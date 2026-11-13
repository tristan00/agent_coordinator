use crate::history::{self, Artifact, AttemptEvidence, Attributes, Event, Lane, Need, Size};
use crate::{Result, evidence, lock, metrics, now, process, read, require, write};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::mem::{MaybeUninit, size_of};
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use windows_sys::Win32::Storage::FileSystem::{FILE_ID_INFO, FILE_STANDARD_INFO, FileIdInfo, FileStandardInfo, GetFileInformationByHandleEx};
pub fn id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}-{}", now(), std::process::id(), SEQUENCE.fetch_add(1, Ordering::Relaxed))
}
pub fn next_stage(status: &str) -> Option<&'static str> {
    match status {
        "accepted" => Some("done"),
        "rejected" => Some("implementation"),
        "completed" => Some("review"),
        "blocked" => Some("replan"),
        _ => None,
    }
}
pub fn worker_statuses(stage: &str) -> &'static [&'static str] {
    match stage {
        "review" => &["accepted", "rejected", "blocked"],
        "replan" => &["replanned", "held"],
        _ => &["completed", "blocked"],
    }
}
fn normalized(text: &str) -> String {
    text.trim_matches('\u{feff}').split_whitespace().collect::<Vec<_>>().join(" ")
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyReason {
    #[serde(rename = "uses_artifact")]
    Artifact,
    #[serde(rename = "uses_api")]
    Api,
    #[serde(rename = "uses_schema")]
    Schema,
    #[serde(rename = "uses_data")]
    Data,
}
pub const DEPENDENCY_REASONS: [DependencyReason; 4] = [DependencyReason::Artifact, DependencyReason::Api, DependencyReason::Schema, DependencyReason::Data];
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub target: String,
    pub reason: Option<DependencyReason>,
}
fn dependencies(value: &Value) -> Result<Vec<Dependency>> {
    let links: Vec<Dependency> = serde_json::from_value(value.clone()).map_err(|e| format!("{e}; use after, or nothing"))?;
    require(links.iter().all(|d| d.reason.is_some()), "Dependencies require a typed reason; use after, or nothing")?;
    Ok(links)
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub id: String,
    #[serde(default)]
    pub revision: u64,
    pub stage: String,
    pub started: u64,
    pub ended: Option<u64>,
    pub result: Option<Value>,
    pub error: Option<String>,
    #[serde(default)]
    pub evidence: AttemptEvidence,
}
impl Attempt {
    pub fn status(&self) -> Option<&str> {
        self.result.as_ref()?["status"].as_str()
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    #[serde(default)]
    pub revision: u64,
    pub title: String,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refinements: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub attributes: Attributes,
    #[serde(default)]
    pub history: Vec<Event>,
    #[serde(default)]
    pub recorded_wait: Option<String>,
    #[serde(default)]
    pub changed: u64,
    pub acceptance: Vec<String>,
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub standalone: bool,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub related: Vec<String>,
    #[serde(default)]
    pub writable: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inherited: BTreeMap<String, usize>,
    #[serde(default)]
    pub replanned: bool,
    #[serde(default, rename = "audit", skip_serializing)]
    legacy_audit: bool,
    #[serde(default, rename = "evidenceRequirements", skip_serializing)]
    legacy_evidence: Value,
    pub stage: String,
    pub stopped: bool,
    #[serde(default)]
    pub blocked: bool,
    pub attempts: Vec<Attempt>,
    pub active: Option<String>,
    pub feedback: Vec<String>,
    pub failure: String,
    #[serde(rename = "retryAt")]
    pub retry_at: u64,
}
impl Task {
    pub fn definition_words(&self) -> usize {
        [&self.title, &self.outcome].into_iter().chain(&self.acceptance).map(|text| history::words(text)).sum()
    }
    pub fn container(&self) -> bool {
        self.acceptance.is_empty()
    }
    pub fn read_only(&self) -> bool {
        self.attributes.lane == Lane::Audit
    }
    pub(crate) fn worker_model(&self) -> Option<&str> {
        self.attributes.model.as_deref().filter(|_| self.stage == "implementation")
    }
    fn workdir(&self) -> &Path {
        Path::new(&self.cwd)
    }
    fn reset_work(&mut self) {
        self.revision += 1;
        self.stage = "implementation".into();
        self.blocked = false;
        self.retry_at = 0;
        self.failure.clear();
        self.feedback.clear();
    }
    pub fn status(&self) -> &'static str {
        match self.stage.as_str() {
            "done" => "Done",
            "review" => "Review",
            _ if self.active.is_some() || !self.attempts.is_empty() || self.stage == "replan" => "In progress",
            _ => "To do",
        }
    }
    fn signature(&self) -> Value {
        let mut dependencies = self.dependencies.clone();
        dependencies.sort();
        json!([
            normalized(&self.title),
            normalized(&self.outcome),
            self.acceptance.iter().map(|c| normalized(c)).collect::<Vec<_>>(),
            dependencies,
            self.model,
            self.effort,
            self.cwd,
            self.attributes.lane,
            self.attributes.needs,
            self.attributes.artifact
        ])
    }
    fn snapshot(&self) -> Value {
        json!({"title":self.title,"outcome":self.outcome,"acceptance":self.acceptance,"cwd":self.cwd,"workspaceId":self.workspace,"key":self.key,
            "dependencies":self.dependencies,"after":self.after,"parent":self.parent,"standalone":self.standalone,"conflicts":self.conflicts,"related":self.related,
            "writable":self.writable,"model":self.model,"effort":self.effort,"attributes":self.attributes,"definitionWords":self.definition_words()})
    }
    pub fn record(&mut self, kind: &str, source: &str, evidence: Value) {
        self.history.push(Event::new(self, kind, source, evidence));
    }
    fn record_phase(&mut self, from: &str, source: &str) {
        if from != self.stage {
            let stage = self.stage.clone();
            self.record("phase_changed", source, json!({"from":from,"to":stage}));
        }
    }
    pub fn brief(&self) -> Value {
        let latest = self.attempts.iter().rev().find(|a| a.result.is_some() || a.error.is_some()).map(|a| {
            let result = a.result.as_ref();
            json!({"stage":a.stage,"status":a.status(),"summary":result.map(|r| &r["summary"]),"findings":result.map(|r| &r["findings"]),"criteria":result.map(|r| &r["criteria"]),"error":a.error})
        });
        let tokens: u64 = self.attempts.iter().filter_map(|a| a.evidence.usage.as_ref()?.tokens()).sum();
        json!({"id":self.id,"key":self.key,"title":self.title,"workspace":self.workspace,"parent":self.parent,"stage":self.stage,"status":self.status(),"container":self.container(),
            "active":self.active.is_some(),"stopped":self.stopped,"blocked":self.blocked,"revision":self.revision,"waitingReason":self.recorded_wait,"strikes":self.strikes(),
            "feedback":self.feedback,"failure":self.failure,"attempts":self.attempts.len(),"tokens":tokens,"latest":latest})
    }
    fn failures(&self) -> BTreeMap<String, usize> {
        let mut failures = self.inherited.clone();
        for result in self.attempts.iter().filter(|a| a.revision == self.revision).filter_map(|a| a.result.as_ref()) {
            for (outcome, criterion) in result["criteria"].as_array().into_iter().flatten().zip(&self.acceptance) {
                if outcome == "failed" {
                    *failures.entry(normalized(criterion)).or_default() += 1;
                }
            }
        }
        failures
    }
    fn strikes(&self) -> usize {
        let failures = self.failures();
        self.acceptance.iter().filter_map(|c| failures.get(&normalized(c))).copied().max().unwrap_or(0)
    }
    fn failed_replans(&self) -> usize {
        let resumed = self.history.iter().rev().find(|e| e.kind == "resumed").map_or(0, |e| e.at);
        self.attempts
            .iter()
            .rev()
            .take_while(|a| a.stage == "replan" && a.started >= resumed && a.result.as_ref().is_none_or(|r| r["status"] != "replanned" || r["planError"].is_string()))
            .filter(|a| a.result.is_some())
            .count()
    }
}
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(value: D) -> std::result::Result<Option<Option<T>>, D::Error> {
    Ok(Some(Option::deserialize(value)?))
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Input {
    key: Option<String>,
    title: Option<String>,
    outcome: Option<String>,
    cwd: Option<String>,
    acceptance: Option<Vec<String>>,
    dependencies: Option<Vec<Dependency>>,
    after: Option<Vec<String>>,
    #[serde(default, deserialize_with = "nullable")]
    parent: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    standalone: Option<Option<bool>>,
    conflicts: Option<Vec<String>>,
    related: Option<Vec<String>>,
    supersedes: Option<Vec<String>>,
    writable: Option<Vec<PathBuf>>,
    #[serde(default, deserialize_with = "nullable")]
    model: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    effort: Option<Option<String>>,
    attributes: Option<Value>,
}
impl Input {
    fn parse(mut value: Value) -> Result<Self> {
        if let Some(object) = value.as_object_mut() {
            object.remove("reason");
        }
        if let Some(value) = value.get("dependencies") {
            dependencies(value)?;
        }
        if let Some(attributes) = value.get("attributes").and_then(Value::as_object) {
            let unknown: Vec<_> = attributes.keys().filter(|k| !Attributes::FIELDS.contains(&k.as_str())).collect();
            require(unknown.is_empty(), "Unknown attribute; use lane, needs, languages, functions, priority, size, artifact or deploy")?;
        }
        serde_json::from_value(value).map_err(|e| format!("{e}; coordination belongs in typed fields: dependencies, after, parent, conflicts, related, supersedes, writable, attributes").into())
    }
    fn grouping(&self) -> Result<()> {
        require(self.parent.as_ref().is_some_and(Option::is_some) ^ (self.standalone == Some(Some(true))), "Choose exactly one grouping: parent container or standalone true")?;
        require(self.standalone != Some(None), "standalone must be a boolean")
    }
    fn apply(self, task: &mut Task) -> Result<()> {
        if self.parent.is_some() || self.standalone.is_some() {
            self.grouping()?;
            task.parent = self.parent.clone().flatten();
            task.standalone = self.standalone == Some(Some(true));
        }
        let text = |s: String| s.trim().to_owned();
        if let Some(v) = self.title {
            task.title = text(v);
        }
        if let Some(v) = self.outcome {
            task.outcome = text(v);
        }
        if let Some(v) = self.acceptance {
            task.acceptance = v.into_iter().map(text).collect();
        }
        if let Some(v) = self.cwd {
            task.cwd = v;
            task.workspace = Some(workspace(Path::new(&task.cwd))?);
        }
        if let Some(value) = self.dependencies {
            task.dependencies = value;
        }
        for (field, value) in [(&mut task.after, self.after), (&mut task.conflicts, self.conflicts), (&mut task.related, self.related)] {
            if let Some(mut value) = value {
                value.dedup();
                *field = value;
            }
        }
        if let Some(v) = self.model {
            task.model = v.filter(|m| !m.trim().is_empty());
        }
        if let Some(v) = self.effort {
            task.effort = v;
        }
        if let Some(v) = self.attributes {
            let mut merged = serde_json::to_value(&task.attributes)?;
            match (merged.as_object_mut(), v.as_object()) {
                (Some(base), Some(changes)) => base.extend(changes.clone()),
                _ => merged = v,
            }
            task.attributes = serde_json::from_value(merged)?;
            task.attributes.needs.sort();
            task.attributes.needs.dedup();
        }
        if let Some(paths) = self.writable {
            task.writable = writable_paths(task.workdir(), paths)?;
        }
        Ok(())
    }
}
fn validate_workdir(path: &Path) -> Result<()> {
    require(path.is_absolute() && path.is_dir(), "cwd must be an existing absolute directory")
}
pub(crate) fn workspace(path: &Path) -> Result<PathBuf> {
    validate_workdir(path)?;
    Ok(path.canonicalize()?)
}
fn writable_paths(base: &Path, paths: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let mut resolved = Vec::new();
    for path in paths {
        require(!(path.as_os_str().is_empty() || path.components().any(|part| part == Component::ParentDir)), "Writable paths must be absolute or workspace relative, without '..'")?;
        let path = resolve_scope_path(&base.join(path))?;
        if !resolved.contains(&path) {
            resolved.push(path);
        }
    }
    Ok(resolved)
}
pub(crate) fn resolve_scope_path(path: &Path) -> Result<PathBuf> {
    for ancestor in path.ancestors() {
        match ancestor.canonicalize() {
            Ok(existing) if ancestor == path => return Ok(existing),
            Ok(existing) => return Ok(existing.join(path.strip_prefix(ancestor)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && ancestor.symlink_metadata().is_err() => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err("Missing writable path root".into())
}
pub(crate) fn covers(root: &Path, path: &Path) -> bool {
    let normalized = |part: Component<'_>| part.as_os_str().to_string_lossy().trim_end_matches(['.', ' ']).to_lowercase();
    let mut parts = path.components().map(normalized);
    root.components().map(normalized).all(|root| parts.next() == Some(root))
}
pub fn linked_file(path: &Path) -> bool {
    unsafe fn info<T>(file: &fs::File, class: i32) -> Option<T> {
        let mut info = MaybeUninit::<T>::uninit();
        let success = unsafe { GetFileInformationByHandleEx(file.as_raw_handle(), class, info.as_mut_ptr().cast(), size_of::<T>() as u32) };
        (success != 0).then(|| unsafe { info.assume_init() })
    }
    let Ok(file) = fs::OpenOptions::new().access_mode(0).open(path) else { return false };
    unsafe { info::<FILE_STANDARD_INFO>(&file, FileStandardInfo) }.is_some_and(|i| i.NumberOfLinks >= 2) && unsafe { info::<FILE_ID_INFO>(&file, FileIdInfo) }.is_some()
}
fn default_capabilities() -> Vec<Need> {
    vec![Need::Network]
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub implementation: Vec<String>,
    pub review: Vec<String>,
    #[serde(default)]
    pub replan: Vec<String>,
    #[serde(default = "default_capabilities")]
    pub capabilities: Vec<Need>,
    #[serde(default)]
    pub notify: Vec<String>,
    #[serde(default)]
    pub workspaces: BTreeMap<PathBuf, history::WorkspaceSettings>,
    #[serde(default, skip_serializing)]
    limits: BTreeMap<PathBuf, history::Limits>,
}
impl Config {
    fn adopt_legacy_limits(&mut self) -> Result<()> {
        for (path, limits) in std::mem::take(&mut self.limits) {
            let settings = self.workspaces.entry(path.clone()).or_default();
            require(settings.limits == history::Limits::default() || settings.limits == limits, format!("Workspace {} has conflicting limits and workspaces settings", path.display()))?;
            settings.limits = limits;
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) family: Option<crate::agents::Family>,
    #[serde(default)]
    batches: std::collections::BTreeMap<String, Value>,
    #[serde(default, rename = "schemaVersion")]
    schema_version: Option<u32>,
    pub paused: bool,
    #[serde(default)]
    pub controls: Vec<Value>,
    #[serde(default)]
    pub hold_until: u64,
    pub tasks: Vec<Task>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub archived: Vec<Task>,
    pub config: Config,
    #[serde(default)]
    pub sequence: u64,
    #[serde(default)]
    changes: BTreeMap<PathBuf, u64>,
}
impl State {
    pub fn index(&self, id: &str) -> Result<usize> {
        self.tasks.iter().position(|task| task.id == id).ok_or_else(|| format!("Task not found: {id}").into())
    }
    fn get(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|task| task.id == id)
    }
    pub fn children<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Task> {
        self.tasks.iter().filter(move |task| task.parent.as_deref() == Some(id))
    }
    fn ancestors<'a>(&'a self, task: &'a Task) -> Vec<&'a Task> {
        let mut chain = Vec::new();
        let mut parent = task.parent.as_deref().and_then(|id| self.get(id));
        while let Some(next) = parent.filter(|next| !chain.iter().any(|seen: &&Task| seen.id == next.id)) {
            chain.push(next);
            parent = next.parent.as_deref().and_then(|id| self.get(id));
        }
        chain
    }
    fn owning_workspace(&self, directory: PathBuf) -> PathBuf {
        self.config.workspaces.keys().filter(|path| directory.starts_with(path)).max_by_key(|path| path.components().count()).cloned().unwrap_or(directory)
    }
    pub fn settings(&self, key: Option<&Path>) -> history::WorkspaceSettings {
        key.and_then(|key| self.config.workspaces.get(key)).cloned().unwrap_or_default()
    }
    pub fn models(&self, key: Option<&Path>) -> Value {
        let settings = self.settings(key);
        let agents: Vec<_> = crate::agents::enabled(&settings.agents, self.family)
            .into_iter()
            .map(|(id, agent)| json!({"id":id,"provider":agent.provider,"model":agent.model,"effort":agent.effort,"note":agent.note}))
            .collect();
        json!({"family":self.family,"default":settings.worker,"agents":agents,"usage":"Set attributes.model to an id to choose who implements a task; omit it to use the default. Review and replan always use the default."})
    }
    pub fn limits(&self, task: &Task) -> history::Limits {
        let key = task.workspace.clone().or_else(|| workspace(Path::new(&task.cwd)).ok());
        self.settings(key.as_deref()).limits
    }
    pub fn check_definition(&self, task: &Task) -> Result<()> {
        let limits = self.limits(task);
        require(!(task.title.is_empty() || task.outcome.is_empty() || task.cwd.trim().is_empty()), "Title, outcome and cwd are required")?;
        require(history::words(&task.title) <= limits.title_words, format!("Title exceeds {} words", limits.title_words))?;
        require(task.acceptance.len() <= limits.criteria, format!("At most {} acceptance criteria; split the work into child tasks under a container", limits.criteria))?;
        require(!task.acceptance.iter().any(|c| c.is_empty()), "Acceptance criteria must not be blank")?;
        require(task.definition_words() <= limits.words, format!("Definition exceeds {} words; split the work into child tasks under a container", limits.words))?;
        require(![&task.title, &task.outcome].into_iter().chain(&task.acceptance).any(|t| history::coordination_prose(t)), "Task ids, digests and AGENTS.md belong in relations and worker protocol, not task prose")?;
        require(!(task.attributes.size == Size::Large && !task.container()), "Large work must be a container with child tasks")?;
        require(!(task.container() && task.attributes.artifact != Artifact::None), "Containers produce no artifact; children do")?;
        task.attributes.validate()?;
        if let Some(model) = task.attributes.model.as_deref() {
            require(!task.container(), "Containers run no worker; set model on their children")?;
            let key = task.workspace.clone().or_else(|| workspace(Path::new(&task.cwd)).ok());
            self.settings(key.as_deref()).task_agent(Some(model), self.family)?;
        }
        task.effort.as_deref().map(history::effort).transpose()?;
        let missing: Vec<_> = task.attributes.needs.iter().filter(|need| !self.config.capabilities.contains(need)).collect();
        require(missing.is_empty(), format!("No worker provides {}; choose a verification the workers can run", serde_json::to_string(&missing)?))?;
        validate_workdir(task.workdir())
    }
    fn check_relations(&self, task: &Task) -> Result<()> {
        require(!(task.parent.is_some() && task.standalone), "Choose exactly one grouping: parent container or standalone true")?;
        let mut targets = HashSet::new();
        require(task.dependencies.iter().all(|d| targets.insert(&d.target)), "Duplicate dependency target")?;
        let related = task.dependencies.iter().map(|d| &d.target).chain(&task.after).chain(&task.conflicts).chain(&task.related).chain(&task.parent);
        for id in related {
            require(id != &task.id, "A task cannot relate to itself")?;
            self.get(id).ok_or(format!("Related task not found: {id}"))?;
        }
        if let Some(parent) = task.parent.as_deref().and_then(|id| self.get(id)) {
            require(parent.container(), "Parent must be a container (no acceptance criteria)")?;
        }
        let lineage: HashSet<&str> = self.ancestors(task).iter().map(|t| t.id.as_str()).collect();
        require(!lineage.contains(task.id.as_str()), "Parent cycle")?;
        require(!task.dependencies.iter().map(|d| &d.target).chain(&task.after).any(|id| lineage.contains(id.as_str())), "A child cannot wait on its own container")?;
        let mut pending: Vec<&String> = task.dependencies.iter().map(|d| &d.target).chain(&task.after).collect();
        let mut seen = HashSet::new();
        while let Some(id) = pending.pop() {
            require(id != &task.id, "Dependency or ordering cycle")?;
            if seen.insert(id)
                && let Some(other) = self.get(id)
            {
                pending.extend(other.dependencies.iter().map(|d| &d.target).chain(&other.after));
            }
        }
        Ok(())
    }
    fn waiting_reason(&self, task: &Task) -> Option<String> {
        if !self.queued(task) {
            return None;
        }
        for owner in std::iter::once(task).chain(self.ancestors(task)) {
            if owner.parent.is_none() && !owner.standalone {
                return Some("Unresolved grouping; choose parent container or standalone true".into());
            }
            if owner.dependencies.iter().any(|d| d.reason.is_none()) || self.check_relations(owner).is_err() {
                return Some("Unresolved dependency; classify explicitly, use after, or nothing".into());
            }
        }
        if self.hold_until > now() {
            return Some(format!("Usage limit; dispatch resumes at {}", self.hold_until));
        }
        if task.container() {
            return Some(if self.children(&task.id).next().is_some() { "Waiting for child tasks" } else { "Container has no child tasks" }.into());
        }
        let inherited = self.ancestors(task).into_iter().flat_map(|a| a.dependencies.iter().map(|d| &d.target).chain(&a.after));
        for id in task.dependencies.iter().map(|d| &d.target).chain(inherited) {
            if let Some(other) = self.get(id).filter(|other| other.stage != "done") {
                return Some(format!("Waiting for result from {}", other.title));
            }
        }
        if let Some(other) = task.after.iter().filter_map(|id| self.get(id)).find(|o| o.active.is_some() || (o.stage != "done" && !o.stopped && !o.blocked)) {
            return Some(format!("Waiting for {} to settle", other.title));
        }
        for other in self.tasks.iter().filter(|o| o.active.is_some() && o.id != task.id) {
            if task.conflicts.contains(&other.id) || other.conflicts.contains(&task.id) {
                return Some(format!("Conflicts with running {}", other.title));
            }
        }
        (now() < task.retry_at).then(|| format!("Retry scheduled at {}", task.retry_at))
    }
    fn queued(&self, task: &Task) -> bool {
        !self.paused && task.active.is_none() && !task.stopped && !task.blocked && task.stage != "done"
    }
    fn dependents(&self, id: &str) -> Vec<usize> {
        let mut found: Vec<usize> = Vec::new();
        let mut pending = vec![id.to_owned()];
        while let Some(id) = pending.pop() {
            for (index, task) in self.tasks.iter().enumerate() {
                if task.dependencies.iter().any(|d| d.target == id) && !found.contains(&index) {
                    found.push(index);
                    pending.push(task.id.clone());
                }
            }
        }
        found
    }
    fn insert(&mut self, value: Value, parent_override: Option<String>) -> Result<String> {
        let mut input = Input::parse(value)?;
        if let Some(parent) = parent_override {
            input.parent = Some(Some(parent));
            input.standalone = None;
        }
        input.grouping()?;
        let key = input.key.as_ref().map(|key| key.trim().to_owned());
        require(!key.as_ref().is_some_and(String::is_empty), "Task key must not be blank")?;
        let cwd = input.cwd.clone().ok_or("cwd is required")?;
        let directory = workspace(Path::new(&cwd))?;
        let run_directory = directory.clone();
        let directory = self.owning_workspace(directory);
        if let Some(key) = &key
            && let Some(index) = self.tasks.iter().position(|task| task.key.as_ref() == Some(key) && task.workspace.as_ref() == Some(&directory) && (directory == run_directory || task.cwd == cwd))
        {
            let id = self.tasks[index].id.clone();
            self.revise(index, input, "keyed_create")?;
            return Ok(id);
        }
        let supersedes = input.supersedes.clone().unwrap_or_default();
        let mut task: Task = serde_json::from_value(
            json!({"id":id(),"title":"","outcome":"","cwd":cwd,"acceptance":[],"dependencies":[],"stage":"implementation","stopped":false,"attempts":[],"active":null,"feedback":[],"failure":"","retryAt":0,"key":key,"workspace":directory}),
        )?;
        input.apply(&mut task)?;
        task.workspace = task.workspace.take().map(|directory| self.owning_workspace(directory));
        self.check_definition(&task)?;
        self.check_relations(&task)?;
        if let Some(parent) = task.parent.as_deref().and_then(|id| self.get(id)) {
            require(parent.stage != "done", "Cannot add work under a completed container")?;
        }
        let id = task.id.clone();
        for old in &supersedes {
            let index = self.index(old)?;
            require(self.tasks[index].active.is_none() && self.children(old).next().is_none(), "Superseded tasks must be idle and have no children")?;
            for other in &mut self.tasks {
                let before = other.snapshot();
                for link in other.dependencies.iter_mut().filter(|d| &d.target == old) {
                    link.target = id.clone();
                }
                for list in [&mut other.after] {
                    for entry in list.iter_mut().filter(|entry| *entry == old) {
                        *entry = id.clone();
                    }
                }
                if before != other.snapshot() {
                    other.record("rewired", "supersession", json!({"before":before,"after":other.snapshot()}));
                }
            }
            let mut retired = self.tasks.remove(index);
            retired.record("superseded", "create", json!({"by":id}));
            self.archived.push(retired);
        }
        let mut snapshot = task.snapshot();
        snapshot["supersedes"] = json!(supersedes);
        task.record("created", "create", snapshot);
        self.tasks.push(task);
        for task in &self.tasks {
            self.check_relations(task)?;
        }
        Ok(id)
    }
    fn insert_children(&mut self, parent: &str, children: &[Value]) -> Result<Value> {
        require(!children.is_empty(), "Children are required")?;
        let mut mapping = serde_json::Map::new();
        let mut definitions = Vec::new();
        for child in children {
            let mut value = child.clone();
            let object = value.as_object_mut().ok_or("Child must be an object")?;
            let key = object.remove("localKey").and_then(|v| v.as_str().map(str::to_owned)).ok_or("Child localKey is required")?;
            require(!key.trim().is_empty() && !mapping.contains_key(&key), "Child local keys must be nonblank and unique")?;
            require(!object.contains_key("key") && !object.contains_key("supersedes") && !object.contains_key("parent") && !object.contains_key("standalone"), "Children inherit grouping and cannot replace tasks")?;
            object.entry("cwd").or_insert(json!(self.get(parent).ok_or("Missing container")?.cwd));
            let mut initial = value.clone();
            for field in ["dependencies", "after", "conflicts", "related"] {
                initial.as_object_mut().unwrap().remove(field);
            }
            let id = self.insert(initial, Some(parent.to_owned()))?;
            mapping.insert(key, json!(id));
            definitions.push((id, value));
        }
        let resolve = |target: &mut Value| -> Result<()> {
            if let Some(object) = target.as_object() {
                require(object.len() == 1, "Sibling reference must contain only local")?;
                let key = object.get("local").and_then(Value::as_str).ok_or("Sibling reference requires local key")?;
                *target = mapping.get(key).cloned().ok_or("Unknown sibling local key")?;
            }
            require(target.is_string(), "Reference must be a task ID or local reference")
        };
        for (id, mut value) in definitions {
            if let Some(links) = value.get_mut("dependencies") {
                for link in links.as_array_mut().ok_or("Dependencies must be an array")? {
                    resolve(link.get_mut("target").ok_or("Dependency target is required")?)?;
                }
            }
            for field in ["after", "conflicts", "related"] {
                if let Some(links) = value.get_mut(field) {
                    for target in links.as_array_mut().ok_or("Relations must be arrays")? {
                        resolve(target)?;
                    }
                }
            }
            let index = self.index(&id)?;
            Input::parse(value)?.apply(&mut self.tasks[index])?;
            self.check_definition(&self.tasks[index])?;
            let snapshot = self.tasks[index].snapshot();
            self.tasks[index].history.clear();
            self.tasks[index].record("created", "create", snapshot);
        }
        for task in &self.tasks {
            self.check_relations(task)?;
        }
        Ok(Value::Object(mapping))
    }
    fn revise(&mut self, index: usize, input: Input, source: &str) -> Result<()> {
        require(input.supersedes.is_none(), "supersedes applies only when creating a task")?;
        let previous = self.tasks[index].clone();
        let mut task = previous.clone();
        input.apply(&mut task)?;
        task.workspace = task.workspace.take().map(|directory| self.owning_workspace(directory));
        if task.parent != previous.parent
            && let Some(parent) = task.parent.as_deref().and_then(|id| self.get(id))
        {
            require(parent.stage != "done", "Cannot add work under a completed container")?;
        }
        if task.snapshot() == previous.snapshot() {
            return Ok(());
        }
        require(previous.active.is_none(), "Stop the task and wait for its worker to exit before editing")?;
        self.check_definition(&task)?;
        self.check_relations(&task)?;
        if task.attributes.model != previous.attributes.model {
        }
        let corrective = task.signature() != previous.signature() || (previous.stage == "replan" && !task.refinements.is_empty());
        if corrective {
            task.refinements.clear();
            if source != "replan" {
                task.inherited.clear();
                task.replanned = false;
            }
            for dependent in self.dependents(&task.id) {
                let other = &mut self.tasks[dependent];
                require(other.active.is_none(), format!("Stop dependent task \"{}\" before changing its prerequisite", other.title))?;
                if other.stage != "implementation" || other.attempts.iter().any(|a| a.revision == other.revision) {
                    let from = other.stage.clone();
                    other.reset_work();
                    other.record("reopened", source, json!({"causedBy":task.id}));
                    other.record_phase(&from, source);
                }
            }
            task.reset_work();
        }
        task.record(if corrective { "revised" } else { "updated" }, source, task.snapshot());
        task.record_phase(&previous.stage, source);
        self.tasks[index] = task;
        for task in &self.tasks {
            self.check_relations(task)?;
        }
        Ok(())
    }
    fn settle_containers(&mut self) -> bool {
        let mut changed = false;
        for index in 0..self.tasks.len() {
            let task = &self.tasks[index];
            if !task.container() || task.active.is_some() {
                continue;
            }
            let done = {
                let mut children = self.children(&task.id).peekable();
                children.peek().is_some() && children.all(|child| child.stage == "done")
            };
            let stage = if done { "done" } else { "implementation" };
            let current = task.stage.clone();
            if current != stage && (done || current == "done") {
                let task = &mut self.tasks[index];
                let from = task.stage.clone();
                task.stage = stage.into();
                task.record(if done { "children_done" } else { "reopened" }, "containment", json!({}));
                task.record_phase(&from, "containment");
                changed = true;
            }
        }
        changed
    }
}
pub struct Engine {
    pub directory: PathBuf,
    pub state: State,
    pub pending_deploy: Option<String>,
    pub notices: Vec<Value>,
}
impl Engine {
    pub fn new(directory: PathBuf) -> Result<Self> {
        let file = directory.join("state.json");
        let mut state: State = if file.exists() {
            let mut raw: Value = read(&file)?;
            for role in ["implementation", "review", "replan"] {
                if raw["config"][role][1] == "--codex" {
                    raw["config"][role][1] = json!("--agent");
                }
            }
            if raw["schemaVersion"].as_u64().unwrap_or(0) < 4 {
                for field in ["tasks", "archived"] {
                    for task in raw.get_mut(field).and_then(Value::as_array_mut).into_iter().flatten() {
                        for link in task["dependencies"].as_array_mut().into_iter().flatten() {
                            if let Some(target) = link.as_str() {
                                *link = json!({"target":target,"reason":null});
                            }
                        }
                    }
                }
            }
            for field in ["tasks", "archived"] {
                for task in raw[field].as_array().into_iter().flatten() {
                    for link in task["dependencies"].as_array().into_iter().flatten() {
                        require(link.get("reason").is_some(), "Dependency reason missing; use after, or nothing")?;
                    }
                }
            }
            serde_json::from_value(raw).map_err(|e| format!("{e}; use after, or nothing"))?
        } else {
            let command = vec![std::env::current_exe()?.to_string_lossy().into(), "--external".into()];
            let config =
                Config { implementation: command.clone(), review: command.clone(), replan: command, capabilities: default_capabilities(), notify: Vec::new(), workspaces: BTreeMap::new(), limits: BTreeMap::new() };
            State {
                family: None,
                batches: Default::default(),
                schema_version: Some(history::SCHEMA_VERSION),
                paused: false,
                controls: Vec::new(),
                hold_until: 0,
                tasks: Vec::new(),
                archived: Vec::new(),
                config,
                sequence: 0,
                changes: BTreeMap::new(),
            }
        };
        require(!(state.schema_version > Some(history::SCHEMA_VERSION)), "State schema is newer than this coordinator")?;
        if state.schema_version < Some(4) && directory.join("relationship-plan.json").exists() {
            let plan: Value = read(&directory.join("relationship-plan.json"))?;
            for decision in plan["tasks"].as_array().ok_or("Relationship plan requires tasks")? {
                if let Some(task) = state.tasks.iter_mut().find(|t| decision["id"] == t.id) {
                    let before = task.snapshot();
                    let grouping = Input::parse(json!({"parent":decision.get("parent").cloned().unwrap_or(Value::Null),"standalone":decision.get("standalone").cloned().unwrap_or(json!(false))}))?;
                    grouping.apply(task)?;
                    task.dependencies = dependencies(&decision["dependencies"])?;
                    task.after = serde_json::from_value(decision["after"].clone())?;
                    task.record("relationships_classified", "relationship-plan.json", json!({"before":before,"after":task.snapshot()}));
                }
            }
        }
        for task in &state.tasks {
            if task.dependencies.iter().all(|d| d.reason.is_some()) {
                state.check_relations(task)?;
            }
        }
        if state.config.replan.is_empty() {
            state.config.replan = state.config.implementation.clone();
        }
        state.family = crate::agents::family(state.family);
        state.config.adopt_legacy_limits()?;
        for index in 0..state.tasks.len() {
            let task = &mut state.tasks[index];
            if task.legacy_audit {
                task.attributes.lane = Lane::Audit;
            }
            if task.legacy_evidence.to_string().contains("candidate") && task.attributes.artifact == Artifact::None {
                task.attributes.artifact = Artifact::Candidate;
            }
            let task = &state.tasks[index];
            let invalid = !task.refinements.is_empty() || state.check_definition(task).is_err();
            if !task.stopped && !task.blocked && task.stage != "done" && task.active.is_none() && invalid && task.stage != "replan" {
                let task = &mut state.tasks[index];
                let from = task.stage.clone();
                task.stage = "replan".into();
                task.blocked = false;
                task.feedback = vec!["Legacy definition violates current limits; rewrite it with typed attributes and relations".into()];
                task.record_phase(&from, "migration");
            }
        }
        state.schema_version = Some(history::SCHEMA_VERSION);
        write(&file, &state)?;
        Ok(Self { directory, state, pending_deploy: None, notices: Vec::new() })
    }
    pub fn task(&self, id: &str) -> Result<usize> {
        self.state.index(id)
    }
    pub fn attempt_path(&self, id: &str) -> PathBuf {
        self.directory.join("attempts").join(id)
    }
    pub(crate) fn commit(&mut self, mut state: State) -> Result<()> {
        state.schema_version = Some(history::SCHEMA_VERSION);
        let next = self.state.sequence + 1;
        let mut changed: Vec<PathBuf> = Vec::new();
        for task in &mut state.tasks {
            if self.state.get(&task.id).is_none_or(|old| old.brief() != task.brief()) {
                task.changed = next;
                changed.push(task.workspace.clone().unwrap_or_default());
            }
        }
        for old in self.state.tasks.iter().filter(|old| state.get(&old.id).is_none()) {
            changed.push(old.workspace.clone().unwrap_or_default());
        }
        if (state.paused, state.hold_until, &state.config.workspaces) != (self.state.paused, self.state.hold_until, &self.state.config.workspaces) {
            changed.push(PathBuf::new());
        }
        if !changed.is_empty() {
            state.sequence = next;
            for key in changed {
                state.changes.insert(key, next);
            }
        }
        write(&self.directory.join("state.json"), &state)?;
        self.state = state;
        Ok(())
    }
    fn change(&mut self, mutate: impl FnOnce(&mut State) -> Result<Value>) -> Result<Value> {
        let mut state = self.state.clone();
        let value = mutate(&mut state)?;
        self.commit(state)?;
        Ok(value)
    }
    pub fn changed_since(&self, key: Option<&Path>, after: Option<u64>) -> bool {
        after.is_none_or(|after| key.map_or(self.state.sequence > after, |key| [key, Path::new("")].into_iter().any(|key| self.state.changes.get(key).is_some_and(|at| *at > after))))
    }
    pub fn updates(&self, key: Option<&Path>, after: Option<u64>, error: &str) -> Value {
        let scoped: Vec<&Task> = self.state.tasks.iter().filter(|task| key.is_none_or(|key| task.workspace.as_deref() == Some(key))).collect();
        json!({"sequence":self.state.sequence,"updates":self.state.settings(key).updates,"paused":self.state.paused,"holdUntil":self.state.hold_until,"error":error,
            "ids":scoped.iter().map(|task| &task.id).collect::<Vec<_>>(),
            "tasks":scoped.iter().filter(|task| after.is_none_or(|after| task.changed > after)).map(|task| task.brief()).collect::<Vec<_>>()})
    }
    pub fn task_value(&self, task: &Task, detailed: bool) -> Result<Value> {
        let mut value = serde_json::to_value(task)?;
        if !detailed {
            value.as_object_mut().unwrap().remove("history");
        }
        let blocks: Vec<_> = self.state.tasks.iter().filter(|t| t.dependencies.iter().any(|d| d.target == task.id)).map(|t| &t.id).collect();
        value["status"] = json!(task.status());
        value["container"] = json!(task.container());
        value["children"] = json!(self.state.children(&task.id).map(|t| &t.id).collect::<Vec<_>>());
        value["blocks"] = json!(blocks);
        value["definitionWords"] = json!(task.definition_words());
        value["waitingReason"] = json!(task.recorded_wait);
        value["strikes"] = json!(task.strikes());
        value["analytics"] = metrics::lifecycle(task);
        Ok(value)
    }
    pub fn create(&mut self, input: Value) -> Result<Value> {
        let id = self.change(|state| Ok(json!(state.insert(input, None)?)))?;
        self.task_value(&self.state.tasks[self.task(id.as_str().unwrap())?], false)
    }
    pub fn create_batch(&mut self, input: Value) -> Result<Value> {
        let object = input.as_object().ok_or("Batch must be an object")?;
        require(object.keys().all(|k| ["requestKey", "container", "children"].contains(&k.as_str())), "Unknown batch field")?;
        let key = input["requestKey"].as_str().filter(|s| !s.trim().is_empty()).ok_or("requestKey is required")?.to_owned();
        if let Some(receipt) = self.state.batches.get(&key) {
            require(receipt["input"] == input, "Conflicting request replay")?;
            return Ok(receipt["result"].clone());
        }
        self.change(|state| {
            let container = input["container"].clone();
            let parsed = Input::parse(container.clone())?;
            require(parsed.key.is_none() && parsed.supersedes.is_none(), "Batch container cannot replace existing tasks")?;
            let id = state.insert(container, None)?;
            require(state.get(&id).unwrap().container(), "Container must have no acceptance criteria")?;
            let children = input["children"].as_array().ok_or("Children must be an array")?;
            let mapping = state.insert_children(&id, children)?;
            let result = json!({"container":id,"children":mapping});
            state.batches.insert(key, json!({"input":input,"result":result}));
            Ok(result)
        })
    }
    pub fn edit(&mut self, id: &str, input: Value) -> Result<Value> {
        let index = self.task(id)?;
        self.change(|state| state.revise(index, Input::parse(input)?, "edit").map(|()| Value::Null))?;
        self.task_value(&self.state.tasks[index], false)
    }
    pub fn request_replan(&mut self, id: &str, input: Value) -> Result<Value> {
        let index = self.task(id)?;
        self.change(|state| {
            let limits = state.limits(&state.tasks[index]);
            let task = &mut state.tasks[index];
            require(task.active.is_none() && task.stage != "done" && !task.container(), "Only idle, unfinished, non-container tasks can be replanned")?;
            let from = task.stage.clone();
            task.stage = "replan".into();
            task.blocked = false;
            task.retry_at = 0;
            if let Some(reason) = input["reason"].as_str().filter(|r| !r.trim().is_empty()) {
                task.feedback = vec![history::clip(reason, limits.finding_words)];
            }
            task.record("replan_requested", "api", json!({"reason":input["reason"]}));
            task.record_phase(&from, "api");
            Ok(Value::Null)
        })?;
        self.task_value(&self.state.tasks[index], false)
    }
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let index = self.task(id)?;
        let task = &self.state.tasks[index];
        require(task.active.is_none(), "Task has an active worker")?;
        let referenced = self.state.tasks.iter().any(|t| t.parent.as_deref() == Some(id) || t.dependencies.iter().map(|d| &d.target).chain(&t.after).any(|d| d == id));
        require(!referenced, "Task has children or dependents; supersede it instead")?;
        self.change(|state| {
            let mut task = state.tasks.remove(index);
            for other in &mut state.tasks {
                other.conflicts.retain(|c| c != id);
                other.related.retain(|c| c != id);
            }
            task.record(if task.stage == "done" { "archived" } else { "abandoned" }, "delete", Value::Null);
            state.archived.push(task);
            Ok(Value::Null)
        })
        .map(drop)
    }
    pub fn wipe(&mut self) -> Result<Value> {
        let active: Vec<String> = self.state.tasks.iter().filter_map(|t| t.active.clone()).collect();
        for id in &active {
            self.signal_stop(id)?;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while active.iter().any(|id| self.attempt_path(id).exists() && lock(&self.attempt_path(id).join("lock")).is_err()) {
            require(std::time::Instant::now() < deadline, "Workers are still exiting; send the wipe again")?;
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let source = Path::new(option_env!("COORDINATOR_SOURCE_ROOT").unwrap_or(env!("CARGO_MANIFEST_DIR"))).canonicalize()?;
        let data = self.directory.canonicalize()?;
        let directories: std::collections::BTreeSet<PathBuf> = self.state.config.workspaces.keys().cloned().collect();
        let (mut removed, mut errors) = (Vec::new(), Vec::new());
        for path in &directories {
            let Ok(resolved) = path.canonicalize() else { continue };
            if resolved.parent().is_none() || source.starts_with(&resolved) || resolved.starts_with(&source) || data.starts_with(&resolved) || resolved.starts_with(&data) {
                errors.push(format!("Refused to delete {}", path.display()));
                continue;
            }
            match fs::remove_dir_all(&resolved) {
                Ok(()) => removed.push(path.clone()),
                Err(error) => errors.push(format!("{}: {error}", path.display())),
            }
        }
        let attempts = self.directory.join("attempts");
        if attempts.exists()
            && let Err(error) = fs::remove_dir_all(&attempts)
        {
            errors.push(format!("{}: {error}", attempts.display()));
        }
        let state = &self.state;
        let deleted = json!({"tasks":state.tasks.len(),"archived":state.archived.len(),"workspaces":state.config.workspaces.len(),"batches":state.batches.len(),"stoppedWorkers":active.len()});
        let mut state = self.state.clone();
        state.tasks.clear();
        state.archived.clear();
        state.controls.clear();
        state.batches.clear();
        state.changes.clear();
        state.config.workspaces.clear();
        state.config.limits.clear();
        state.paused = false;
        state.hold_until = 0;
        self.commit(state)?;
        Ok(json!({"deleted":deleted,"removedDirectories":removed,"errors":errors}))
    }
    pub fn configure(&mut self, value: Value) -> Result<()> {
        let mut config: Config = serde_json::from_value(value)?;
        if config.replan.is_empty() {
            config.replan = config.implementation.clone();
        }
        require(![&config.implementation, &config.review, &config.replan].iter().any(|args| args.is_empty() || args.iter().any(|s| s.trim().is_empty())), "Worker commands must be nonempty string arrays")?;
        config.adopt_legacy_limits()?;
        config.workspaces = std::mem::take(&mut config.workspaces)
            .into_iter()
            .map(|(path, settings)| {
                let path = workspace(&path).map_err(|e| format!("Workspace {}: {e}", path.display()))?;
                if self.state.config.workspaces.get(&path) != Some(&settings) {
                    settings.validate(self.state.family)?;
                    crate::workspace_prompts::verify(&path, &settings)?;
                }
                Ok((path, settings))
            })
            .collect::<Result<_>>()?;
        self.change(|state| {
            state.config = config;
            Ok(Value::Null)
        })
        .map(drop)
    }
    pub fn control(&mut self, stop: bool, id: Option<&str>) -> Result<()> {
        if let Some(id) = id {
            self.task(id)?;
        }
        let paused = self.state.paused;
        self.change(|state| {
            if id.is_none() {
                if paused != stop {
                    state.controls.push(json!({"at":now(),"paused":stop}));
                }
                state.paused = stop;
                if !stop {
                    state.hold_until = 0;
                }
            }
            for task in state.tasks.iter_mut().filter(|task| id.is_none_or(|id| id == task.id)) {
                let changed = if id.is_none() { paused != stop } else { task.stopped != stop || (!stop && (task.blocked || task.retry_at != 0)) };
                if id.is_some() {
                    task.stopped = stop;
                    if !stop {
                        task.blocked = false;
                    }
                }
                if !stop {
                    task.retry_at = 0;
                }
                if changed {
                    task.record(if stop { "stopped" } else { "resumed" }, "control", json!({"scope":if id.is_some() {"task"} else {"board"}}));
                }
            }
            Ok(Value::Null)
        })?;
        let active: Vec<_> = self.state.tasks.iter().filter(|t| stop && id.is_none_or(|id| id == t.id)).filter_map(|t| t.active.clone()).collect();
        active.iter().try_for_each(|id| self.signal_stop(id))
    }
    fn signal_stop(&self, id: &str) -> Result<()> {
        Ok(fs::write(self.attempt_path(id).join("stop"), "")?)
    }
    fn launch(&self, id: &str) -> Result<()> {
        let directory = self.attempt_path(id);
        if directory.join("exit.json").exists() {
            return Ok(());
        }
        let Ok(guard) = lock(&directory.join("lock")) else {
            return Ok(());
        };
        drop(guard);
        process::command(std::env::current_exe()?.to_str().ok_or("Invalid executable path")?).arg("--supervise").arg(directory).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
        Ok(())
    }
    fn request(&self, task: &Task, attempt_id: &str) -> Result<Value> {
        let stage = task.stage.as_str();
        let summary = |a: &Attempt| a.result.as_ref().map(|r| json!({"stage":a.stage,"status":r["status"],"summary":r["summary"],"findings":r["findings"],"criteria":r["criteria"]}));
        let dependencies: Vec<_> = task
            .dependencies
            .iter().chain(self.state.ancestors(task).into_iter().flat_map(|a| &a.dependencies))
            .filter_map(|link| self.state.get(&link.target).map(|d| (link, d)))
            .map(|(link, d)| {
                let accepted = d.attempts.iter().rev().find(|a| a.revision == d.revision && a.stage == "implementation" && a.status() == Some("completed"));
                json!({"id":d.id,"target":link.target,"reason":link.reason,"title":d.title,"summary":accepted.and_then(|a| a.result.as_ref()).map(|r| &r["summary"]),"artifact":accepted.and_then(|a| a.evidence.configuration.as_ref()).map(|c| &c["artifact"])})
            })
            .collect();
        let directory = self.attempt_path(attempt_id);
        let produced = task.attempts.iter().rev().find(|a| a.revision == task.revision && a.stage == "implementation" && a.status() == Some("completed"));
        let home = produced.filter(|_| stage == "review").map_or_else(|| directory.clone(), |a| self.attempt_path(&a.id));
        let settings = self.state.settings(task.workspace.as_deref());
        settings.validate(self.state.family)?;
        let (worker, agent) = settings.task_agent(task.worker_model(), self.state.family)?;
        let documents = task.workspace.as_deref().ok_or("Workspace directory is required")?;
        let protocol = crate::workspace_prompts::worker(documents, &settings)?;
        let mut request = json!({"stage":stage,"task":{"id":task.id,"revision":task.revision,"title":task.title,"outcome":task.outcome,"acceptance":task.acceptance,
            "cwd":task.workdir(),"model":agent.model,"effort":agent.effort,"lane":task.attributes.lane,"needs":task.attributes.needs,"artifact":evidence::artifact_name(task.attributes.artifact).map(|name| home.join(name)),
            "languages":task.attributes.languages,"functions":task.attributes.functions,
            "writable":task.writable,"dependencies":task.dependencies,"after":task.after},
            "worker":worker,"agent":agent,"family":self.state.family,"workerProtocol":protocol,"feedback":task.feedback,"dependencies":dependencies,"limits":self.state.limits(task).value(),"scratch":directory.join("scratch"),
            "readOnly":stage != "implementation" || task.read_only()});
        if stage == "review" {
            let implementation = task.attempts.iter().rev().find(|a| a.revision == task.revision && a.stage == "implementation" && a.status() == Some("completed"));
            request["implementation"] = json!(implementation.map(|a| json!({"attemptId":a.id,"result":a.result,"artifact":a.evidence.configuration.as_ref().map(|c| &c["artifact"])})));
        }
        if stage == "replan" {
            request["recentAttempts"] = json!(task.attempts.iter().rev().filter_map(summary).take(4).collect::<Vec<_>>());
            request["legacyNotes"] = json!(task.refinements.iter().map(|r| history::clip(r, 60)).collect::<Vec<_>>());
            request["capabilities"] = json!(self.state.config.capabilities);
        }
        Ok(request)
    }
    fn start(&mut self, index: usize) -> Result<()> {
        let task = &self.state.tasks[index];
        let stage = task.stage.clone();
        let registered = match stage.as_str() {
            "review" => &self.state.config.review,
            "replan" => &self.state.config.replan,
            _ => &self.state.config.implementation,
        };
        let mut command = registered.to_vec();
        if process::local_agent(registered) {
            command[0] = std::env::current_exe()?.to_string_lossy().into_owned();
        }
        let workspace_settings = self.state.settings(task.workspace.as_deref());
        let (worker, agent) = workspace_settings.task_agent(task.worker_model(), self.state.family)?;
        if !process::local_agent(registered) {
            require(command.iter().any(|arg| arg == "{model}") && command.iter().any(|arg| arg.contains("{effort}")), "Worker command requires explicit {model} and {effort} placeholders")?;
        }
        let actual_command: Vec<_> = command.iter().map(|arg| arg.replace("{model}", &agent.model).replace("{effort}", agent.effort())).collect();
        let settings = json!({"scratch":true,"acceptance":task.acceptance,"requested":{"model":task.model,"effort":task.effort},"effective":{"worker":worker,"agent":agent,"family":self.state.family,"model":agent.model,"effort":agent.effort},"needs":task.attributes.needs,
            "registeredCommand":registered,"actualCommand":actual_command,"stage":stage,"request":"request.json","revision":task.revision,
            "workspaceId":task.workspace,"cwd":task.workdir(),"writable":task.writable,
            "coordinatorVersion":env!("CARGO_PKG_VERSION"),"coordinatorBuild":crate::web::deployment()["digest"]});
        let attempt =
            Attempt { id: id(), revision: task.revision, stage: stage.clone(), started: now(), ended: None, result: None, error: None, evidence: AttemptEvidence { configuration: Some(settings), ..Default::default() } };
        let mut request = self.request(task, &attempt.id)?;
        request["command"] = json!(command);
        write(&self.attempt_path(&attempt.id).join("request.json"), &request)?;
        let attempt_id = attempt.id.clone();
        self.change(|state| {
            let (workspace, mut attempt) = (state.tasks[index].workspace.clone(), attempt);
            for other in state.tasks.iter_mut().filter(|other| other.active.is_some() && other.workspace == workspace) {
                attempt.evidence.project_overlapping_attempts.push(other.active.clone().unwrap());
                other.attempts.last_mut().unwrap().evidence.project_overlapping_attempts.push(attempt.id.clone());
            }
            let task = &mut state.tasks[index];
            task.active = Some(attempt.id.clone());
            task.record("dispatched", "dispatch", json!({"stage":stage,"started":attempt.started}));
            task.attempts.push(attempt);
            Ok(Value::Null)
        })?;
        self.launch(&attempt_id)
    }
    fn observe(task: &mut Task, directory: &Path) -> Result<bool> {
        let attempt = task.attempts.last_mut().unwrap();
        let mut changed = false;
        if attempt.evidence.launch.is_none() && directory.join("run.json").exists() {
            attempt.evidence.launch = Some(read(&directory.join("run.json"))?);
            let started = attempt.evidence.launch.as_ref().map(|run| run.started);
            let (id, revision) = (attempt.id.clone(), attempt.revision);
            let event = Event::new(task, "started", "supervisor", json!("run.json")).attempt(&id, revision, started);
            task.history.push(event);
            changed = true;
        }
        let attempt = task.attempts.last_mut().unwrap();
        if attempt.evidence.observed.is_none() && directory.join("launch-settings.json").exists() {
            attempt.evidence.observed = Some(read(&directory.join("launch-settings.json"))?);
            changed = true;
        }
        Ok(changed)
    }
    fn validate_result(&self, task: &Task, attempt: &Attempt, exit: &Value) -> Result<Value> {
        if let Some(error) = exit["error"].as_str() {
            return Err(error.into());
        }
        require(exit["code"] == 0, format!("Worker exited {}; inspect attempt log", exit["code"]))?;
        let directory = self.attempt_path(&attempt.id);
        let mut result: Value = read(&directory.join("result.json"))?;
        let status = result["status"].as_str().unwrap_or_default().to_owned();
        if attempt.stage == "implementation" && status == "completed" && let Some(name) = evidence::artifact_name(task.attributes.artifact) {
            evidence::recover_artifact(&directory, name);
            require(evidence::safe_file(&directory, name).is_some(), format!("Declared artifact missing; write it to {}", directory.join(name).display()))?;
        }
        require(worker_statuses(&attempt.stage).contains(&status.as_str()), "Invalid worker status")?;
        require(result["summary"].as_str().is_some_and(|s| !s.trim().is_empty()), "Worker summary is required")?;
        let limits = self.state.limits(task);
        let list = |value: &Value, items: usize| -> Vec<String> {
            value.as_array().into_iter().flatten().filter_map(Value::as_str).filter(|s| !s.trim().is_empty()).take(items).map(|s| history::clip(s, limits.finding_words)).collect()
        };
        result["summary"] = json!(history::clip(result["summary"].as_str().unwrap(), limits.summary_words));
        result["findings"] = json!(list(&result["findings"], limits.findings));
        result["checks"] = json!(list(&result["checks"], limits.findings + 2));
        if attempt.stage != "replan" {
            let criteria: Vec<String> = serde_json::from_value(result["criteria"].clone()).map_err(|_| "Worker must report criteria outcomes")?;
            require(criteria.len() == task.acceptance.len() && criteria.iter().all(|c| matches!(c.as_str(), "passed" | "failed" | "blocked")), "Worker must report one outcome per acceptance criterion")?;
            let failed = criteria.iter().any(|c| c == "failed");
            if status == "accepted" && criteria.iter().any(|c| c != "passed") {
                result["status"] = json!(if failed { "rejected" } else { "blocked" });
            }
            require(!(status == "rejected" && !failed), "A rejection must mark at least one criterion failed")?;
            if status == "completed" && criteria.iter().any(|c| c != "passed") {
                let unmet = criteria.iter().zip(&task.acceptance).filter(|(o, _)| *o != "passed").map(|(o, c)| history::clip(&format!("Unmet criterion ({o}): {c}"), limits.finding_words));
                let findings: Vec<Value> = unmet.map(Value::from).chain(result["findings"].as_array().cloned().unwrap_or_default()).take(limits.findings).collect();
                result["status"] = json!("blocked");
                result["findings"] = json!(findings);
            }
        }
        if matches!(result["status"].as_str(), Some("rejected" | "blocked" | "held")) {
            require(result["findings"].as_array().is_some_and(|f| !f.is_empty()), "Rejected, blocked and held results require findings")?;
        }
        Ok(result)
    }
    fn apply_plan(state: &mut State, index: usize, plan: &Value) -> Result<()> {
        let mut next = state.clone();
        Self::plan(&mut next, index, plan)?;
        *state = next;
        Ok(())
    }
    fn plan(state: &mut State, index: usize, plan: &Value) -> Result<()> {
        let task = state.tasks[index].clone();
        let children = plan["children"].as_array().cloned().unwrap_or_default();
        let failures = task.failures();
        let criteria = |item: &Value| item["acceptance"].as_array().into_iter().flatten().filter_map(Value::as_str).map(normalized).collect::<Vec<_>>();
        let proposed: Vec<String> = if children.is_empty() { criteria(plan) } else { children.iter().flat_map(criteria).collect() };
        require(!proposed.iter().any(|c| failures.get(c).is_some_and(|n| *n >= history::STRIKES)), "Plan repeats a criterion that already failed twice; change the approach or hold for a decision")?;
        let inherit = |state: &mut State, id: &str| -> Result<()> {
            let index = state.index(id)?;
            let child = &mut state.tasks[index];
            child.inherited = child.acceptance.iter().map(|c| normalized(c)).filter_map(|c| failures.get(&c).map(|n| (c, *n))).collect();
            child.replanned = true;
            Ok(())
        };
        let base = |item: &Value, parent: bool, existing: bool| {
            json!({"title":item["title"],"outcome":item["outcome"],"acceptance":if parent {json!([])} else {item["acceptance"].clone()},
                "attributes":{"lane":item["lane"],"needs":item["needs"],"size":if parent {json!("large")} else {item["size"].clone()},
                "priority":task.attributes.priority,
                "languages":item.get("languages").cloned().unwrap_or_else(|| if existing {json!(task.attributes.languages)} else {json!([])}),
                "functions":item.get("functions").cloned().unwrap_or_else(|| if existing {json!(task.attributes.functions)} else {json!([])}),
                "artifact":if parent {json!("none")} else {item.get("artifact").cloned().unwrap_or(json!(task.attributes.artifact))},"deploy":!parent && item["artifact"] == "candidate" && task.attributes.deploy,"model":null}})
        };
        if children.is_empty() {
            let mut value = base(plan, false, true);
            if let Some(links) = plan.get("dependencies") {
                value["dependencies"] = links.clone();
            }
            state.revise(index, Input::parse(value)?, "replan")?;
            require(state.tasks[index].stage != "replan", "Plan did not change the task definition")?;
            return inherit(state, &task.id);
        }
        let mut value = base(plan, true, true);
        if let Some(links) = plan.get("dependencies") {
            value["dependencies"] = links.clone();
        }
        state.revise(index, Input::parse(value)?, "replan")?;
        let mut definitions = Vec::new();
        for (index, child) in children.iter().enumerate() {
            let mut value = base(child, false, false);
            value["cwd"] = json!(task.cwd);
            value["writable"] = json!(task.writable);
            value["localKey"] = json!(index.to_string());
            let after: Vec<usize> = serde_json::from_value(child.get("after").cloned().unwrap_or(json!([])))?;
            value["after"] = json!(after.iter().map(|i| json!({"local":i.to_string()})).collect::<Vec<_>>());
            let mut links = child.get("dependencies").cloned().unwrap_or(json!([]));
            for link in links.as_array_mut().ok_or("Dependencies must be records; use after, or nothing")? {
                if let Some(index) = link["target"].as_u64() {
                    link["target"] = json!({"local":index.to_string()});
                }
            }
            value["dependencies"] = links;
            definitions.push(value);
        }
        let mapping = state.insert_children(&task.id, &definitions)?;
        mapping.as_object().into_iter().flat_map(|m| m.values()).filter_map(Value::as_str).try_for_each(|id| inherit(state, id))
    }
    fn collect(&mut self, index: usize, exit: Value) -> Result<()> {
        let task = &self.state.tasks[index];
        let Some(attempt) = task.attempts.last().filter(|_| task.active.is_some()) else {
            return Ok(());
        };
        let stopped = exit["stopped"] == true;
        let directory = self.attempt_path(&attempt.id);
        let result = self.validate_result(task, attempt, &exit);
        let log = fs::read_to_string(directory.join("log.txt")).unwrap_or_default();
        let usage_reset = result.is_err().then(|| crate::telemetry::usage_reset(&log)).flatten();
        let mut state = self.state.clone();
        let task = &mut state.tasks[index];
        Self::observe(task, &directory)?;
        let previous_stage = task.stage.clone();
        task.active = None;
        let current = task.attempts.last().unwrap().revision == task.revision;
        let attempt = task.attempts.last_mut().unwrap();
        attempt.ended = exit["ended"].as_u64();
        attempt.evidence.usage = read(&directory.join("usage.json")).ok();
        Self::measure(attempt, &directory)?;
        let mut deploy = false;
        let mut notice = None;
        match (stopped, result) {
            (true, _) => attempt.error = Some("Stopped by user".into()),
            (false, Err(error)) => {
                attempt.error = Some(error.to_string());
                if let Some(reset) = usage_reset {
                    state.hold_until = reset.unwrap_or_else(|| now() + 3_600_000);
                    notice = Some(json!({"event":"usage_limit","until":state.hold_until}));
                } else if current {
                    let failures = task.attempts.iter().rev().take_while(|a| a.error.as_deref().is_some_and(|e| e != "Stopped by user")).count() as u64;
                    let attempt = task.attempts.last().unwrap();
                    task.failure = error.to_string();
                    task.retry_at = now() + attempt.ended.unwrap_or_else(now).saturating_sub(attempt.started).max(1000) * failures;
                }
            }
            (false, Ok(mut result)) => {
                let status = result["status"].as_str().unwrap().to_owned();
                if attempt.stage == "implementation" && status == "completed" {
                    attempt.evidence.configuration.get_or_insert_with(|| json!({}))["artifact"] = json!(evidence::artifact(&directory, task.attributes.artifact));
                }
                if attempt.stage == "review" && status == "accepted" {
                    let recorded = task.attempts.iter().rev().find(|a| a.revision == task.revision && a.status() == Some("completed")).and_then(|a| a.evidence.configuration.as_ref()).map(|c| c["artifact"].clone());
                    let path = recorded.as_ref().and_then(|a| a["path"].as_str()).map(PathBuf::from);
                    if path.as_deref().is_some_and(|p| evidence::digest(p).ok().as_ref() != recorded.as_ref().and_then(|a| a["digest"].as_str()).map(str::to_owned).as_ref()) {
                        result["status"] = json!("rejected");
                        result["findings"] = json!(["Artifact changed after implementation; rebuild it"]);
                    }
                }
                let attempt = task.attempts.last_mut().unwrap();
                attempt.result = Some(result.clone());
                if current {
                    task.failure.clear();
                    task.retry_at = 0;
                    task.feedback = serde_json::from_value(result["findings"].clone())?;
                    match result["status"].as_str().unwrap() {
                        "replanned" => {
                            if let Err(error) = Self::apply_plan(&mut state, index, &result["plan"]) {
                                let limits = state.limits(&state.tasks[index]);
                                let task = &mut state.tasks[index];
                                task.attempts.last_mut().unwrap().result.as_mut().unwrap()["planError"] = json!(error.to_string());
                                task.feedback = vec![history::clip(&error.to_string(), limits.finding_words)];
                            }
                        }
                        "held" => {
                            task.blocked = true;
                            task.failure = result["summary"].as_str().unwrap().into();
                            notice = Some(json!({"event":"held","taskId":task.id,"title":task.title,"reason":task.failure}));
                        }
                        "rejected" if task.strikes() >= history::STRIKES => task.stage = "replan".into(),
                        "rejected" => {
                            let failed: Vec<_> = result["criteria"].as_array().unwrap().iter().zip(&task.acceptance).filter(|(o, _)| *o == "failed").map(|(_, c)| format!("Failed criterion: {c}")).collect();
                            task.feedback.splice(0..0, failed);
                            task.stage = "implementation".into();
                        }
                        status => {
                            task.stage = next_stage(status).unwrap().into();
                            deploy = status == "accepted" && task.attributes.deploy;
                        }
                    }
                }
            }
        }
        let task = &mut state.tasks[index];
        if task.stage == "replan" && previous_stage != "replan" && task.replanned && !task.blocked {
            task.blocked = true;
            task.failure = "Replanned work failed again; a decision is needed".into();
            notice = Some(json!({"event":"held","taskId":task.id,"title":task.title,"reason":task.failure}));
        }
        if task.stage == "replan" && task.failed_replans() >= history::STRIKES && !task.blocked {
            task.blocked = true;
            task.failure = "Replanning failed twice; a decision is needed".into();
            notice = Some(json!({"event":"held","taskId":task.id,"title":task.title,"reason":task.failure}));
        }
        let attempt = task.attempts.last().unwrap();
        let kind = attempt.status().unwrap_or(if stopped { "attempt_stopped" } else { "attempt_failed" }).to_owned();
        let event = Event::new(task, &kind, "attempt_collection", json!({"currentRevision":current,"error":attempt.error,"ended":attempt.ended})).attempt(&attempt.id, attempt.revision, attempt.ended);
        task.history.push(event);
        task.record_phase(&previous_stage, "attempt_collection");
        evidence::clean_scratch(&directory, task.attempts.last().unwrap());
        let task_id = task.id.clone();
        self.commit(state)?;
        metrics::clean(&directory);
        if deploy {
            self.pending_deploy = Some(task_id);
        }
        self.notices.extend(notice);
        Ok(())
    }
    fn measure(attempt: &mut Attempt, directory: &Path) -> Result<()> {
        let before = read::<Value>(&directory.join("project-before.json")).ok();
        let after = read::<Value>(&directory.join("project-after.json")).ok();
        if before.is_some() || after.is_some() {
            attempt.evidence.project_size = Some(json!({"delta":metrics::size_delta(before.as_ref().unwrap_or(&Value::Null),after.as_ref().unwrap_or(&Value::Null)),"before":before,"after":after}));
        }
        if let Some(configuration) = &attempt.evidence.configuration
            && directory.join("baseline.json").exists()
            && directory.join("final-snapshot.json").exists()
        {
            let base: PathBuf = serde_json::from_value(configuration["cwd"].clone())?;
            let changes = directory.join("changes.json");
            let mut changes = if changes.exists() { read::<Value>(&changes)? } else { metrics::compare(read(&directory.join("baseline.json"))?, read(&directory.join("final-snapshot.json"))?, &base, &[]) };
            changes["attribution"] = json!(if attempt.evidence.project_overlapping_attempts.is_empty() { "scoped_observation" } else { "ambiguous" });
            attempt.evidence.changes = Some(changes);
        }
        Ok(())
    }
    fn poll_attempt(&mut self, index: usize) -> Result<()> {
        let Some(id) = self.state.tasks[index].active.clone() else {
            return Ok(());
        };
        let directory = self.attempt_path(&id);
        let exit = directory.join("exit.json");
        if exit.exists() {
            return self.collect(index, read(&exit)?);
        }
        if std::iter::once(&self.state.tasks[index]).chain(self.state.ancestors(&self.state.tasks[index])).any(|t| t.parent.is_none() && !t.standalone) {
            return self.signal_stop(&id);
        }
        if self.state.paused || self.state.tasks[index].stopped {
            self.signal_stop(&id)?;
        }
        let mut task = self.state.tasks[index].clone();
        if Self::observe(&mut task, &directory)? {
            let mut state = self.state.clone();
            state.tasks[index] = task;
            self.commit(state)?;
        }
        self.launch(&id).or_else(|error| write(&exit, &json!({"error":error.to_string(),"ended":now()})))
    }
    pub fn tick(&mut self) -> Result<()> {
        let mut outcome = Ok(());
        for index in 0..self.state.tasks.len() {
            outcome = self.poll_attempt(index).and(outcome);
        }
        let mut state = self.state.clone();
        if state.settle_containers() {
            self.commit(state)?;
        }
        let mut order: Vec<usize> = (0..self.state.tasks.len()).collect();
        order.sort_by_key(|&i| self.state.tasks[i].attributes.priority());
        for index in order {
            let ready = { self.state.queued(&self.state.tasks[index]) && self.state.waiting_reason(&self.state.tasks[index]).is_none() };
            outcome = if ready { self.start(index) } else { Ok(()) }.and(outcome);
        }
        self.sync_waiting()?;
        outcome
    }
    pub fn sync_waiting(&mut self) -> Result<()> {
        let reasons: Vec<_> = self.state.tasks.iter().map(|task| self.state.waiting_reason(task)).collect();
        if reasons.iter().zip(&self.state.tasks).all(|(reason, task)| reason == &task.recorded_wait) {
            return Ok(());
        }
        let mut state = self.state.clone();
        for (task, reason) in state.tasks.iter_mut().zip(reasons) {
            if reason != task.recorded_wait {
                task.record("waiting_changed", "scheduler", json!({"reason":reason}));
                task.recorded_wait = reason;
            }
        }
        self.commit(state)
    }
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
