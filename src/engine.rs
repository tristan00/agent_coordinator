use crate::history::{self, Actor, Artifact, AttemptEvidence, Attributes, Event, Lane, Need, Size};
use crate::{Result, evidence, lock, metrics, now, process, read, require, write};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
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
    pub acceptance: Vec<String>,
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub related: Vec<String>,
    #[serde(default)]
    pub writable: Vec<PathBuf>,
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
    fn workdir(&self) -> &Path {
        self.workspace.as_deref().unwrap_or(Path::new(&self.cwd))
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
            "dependencies":self.dependencies,"after":self.after,"parent":self.parent,"conflicts":self.conflicts,"related":self.related,
            "writable":self.writable,"model":self.model,"effort":self.effort,"attributes":self.attributes,"definitionWords":self.definition_words()})
    }
    pub fn record(&mut self, kind: &str, actor: Actor, source: &str, evidence: Value) {
        self.history.push(Event::new(self, kind, actor, source, evidence));
    }
    fn record_phase(&mut self, from: &str, actor: Actor, source: &str) {
        if from != self.stage {
            let stage = self.stage.clone();
            self.record("phase_changed", actor, source, json!({"from":from,"to":stage}));
        }
    }
    fn strikes(&self) -> usize {
        let mut failures = vec![0; self.acceptance.len()];
        for attempt in self.attempts.iter().filter(|a| a.revision == self.revision && a.status() == Some("rejected")) {
            for (index, outcome) in attempt.result.as_ref().unwrap()["criteria"].as_array().into_iter().flatten().enumerate() {
                if outcome == "failed" && index < failures.len() {
                    failures[index] += 1;
                }
            }
        }
        failures.into_iter().max().unwrap_or(0)
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
    dependencies: Option<Vec<String>>,
    after: Option<Vec<String>>,
    #[serde(default, deserialize_with = "nullable")]
    parent: Option<Option<String>>,
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
            object.remove("actor");
            object.remove("reason");
        }
        if let Some(attributes) = value.get("attributes").and_then(Value::as_object) {
            let unknown: Vec<_> = attributes.keys().filter(|k| !Attributes::FIELDS.contains(&k.as_str())).collect();
            require(unknown.is_empty(), "Unknown attribute; use lane, needs, priority, size, artifact or deploy")?;
        }
        serde_json::from_value(value).map_err(|e| format!("{e}; coordination belongs in typed fields: dependencies, after, parent, conflicts, related, supersedes, writable, attributes").into())
    }
    fn apply(self, task: &mut Task) -> Result<()> {
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
        for (field, value) in [(&mut task.dependencies, self.dependencies), (&mut task.after, self.after), (&mut task.conflicts, self.conflicts), (&mut task.related, self.related)] {
            if let Some(mut value) = value {
                value.dedup();
                *field = value;
            }
        }
        if let Some(v) = self.parent {
            task.parent = v;
        }
        if let Some(v) = self.model {
            task.model = v.filter(|m| !m.trim().is_empty());
        }
        if let Some(v) = self.effort {
            task.effort = v;
        }
        if let Some(v) = self.attributes {
            task.attributes = serde_json::from_value(v)?;
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
}
#[derive(Clone, Serialize, Deserialize)]
pub struct State {
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
    pub fn check_definition(&self, task: &Task) -> Result<()> {
        require(!(task.title.is_empty() || task.outcome.is_empty() || task.cwd.trim().is_empty()), "Title, outcome and cwd are required")?;
        require(history::words(&task.title) <= history::MAX_TITLE_WORDS, "Title exceeds 12 words")?;
        require(task.acceptance.len() <= history::MAX_CRITERIA, "At most 3 acceptance criteria; split the work into child tasks under a container")?;
        require(!task.acceptance.iter().any(|c| c.is_empty()), "Acceptance criteria must not be blank")?;
        require(task.definition_words() <= history::MAX_WORDS, "Definition exceeds 120 words; split the work into child tasks under a container")?;
        require(![&task.title, &task.outcome].into_iter().chain(&task.acceptance).any(|t| history::coordination_prose(t)), "Task ids, digests and AGENTS.md belong in relations and worker protocol, not task prose")?;
        require(!(task.attributes.size == Size::Large && !task.container()), "Large work must be a container with child tasks")?;
        require(!(task.container() && task.attributes.artifact != Artifact::None), "Containers produce no artifact; children do")?;
        task.attributes.validate()?;
        task.effort.as_deref().map(history::effort).transpose()?;
        let missing: Vec<_> = task.attributes.needs.iter().filter(|need| !self.config.capabilities.contains(need)).collect();
        require(missing.is_empty(), format!("No worker provides {}; choose a verification the workers can run", serde_json::to_string(&missing)?))?;
        validate_workdir(task.workdir())
    }
    fn check_relations(&self, task: &Task) -> Result<()> {
        let related = task.dependencies.iter().chain(&task.after).chain(&task.conflicts).chain(&task.related).chain(&task.parent);
        for id in related {
            require(id != &task.id, "A task cannot relate to itself")?;
            self.get(id).ok_or(format!("Related task not found: {id}"))?;
        }
        if let Some(parent) = task.parent.as_deref().and_then(|id| self.get(id)) {
            require(parent.container(), "Parent must be a container (no acceptance criteria)")?;
            require(parent.stage != "done" || task.stage == "done", "Cannot add work under a completed container")?;
        }
        let lineage: HashSet<&str> = self.ancestors(task).iter().map(|t| t.id.as_str()).collect();
        require(!lineage.contains(task.id.as_str()), "Parent cycle")?;
        require(!task.dependencies.iter().chain(&task.after).any(|id| lineage.contains(id.as_str())), "A child cannot wait on its own container")?;
        let mut pending: Vec<&String> = task.dependencies.iter().chain(&task.after).collect();
        let mut seen = HashSet::new();
        while let Some(id) = pending.pop() {
            require(id != &task.id, "Dependency or ordering cycle")?;
            if seen.insert(id)
                && let Some(other) = self.get(id)
            {
                pending.extend(other.dependencies.iter().chain(&other.after));
            }
        }
        Ok(())
    }
    fn waiting_reason(&self, task: &Task) -> Option<String> {
        if !self.queued(task) {
            return None;
        }
        if self.hold_until > now() {
            return Some(format!("Usage limit; dispatch resumes at {}", self.hold_until));
        }
        if task.container() {
            return Some(if self.children(&task.id).next().is_some() { "Waiting for child tasks" } else { "Container has no child tasks" }.into());
        }
        let inherited = self.ancestors(task).into_iter().flat_map(|a| a.dependencies.iter().chain(&a.after));
        for id in task.dependencies.iter().chain(inherited) {
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
            if !task.read_only()
                && !other.read_only()
                && let Some(path) = task.writable.iter().find(|path| other.writable.iter().any(|held| covers(path, held) || covers(held, path)))
            {
                return Some(format!("Waiting for writable scope {} held by {}", path.display(), other.title));
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
                if task.dependencies.contains(&id) && !found.contains(&index) {
                    found.push(index);
                    pending.push(task.id.clone());
                }
            }
        }
        found
    }
    fn insert(&mut self, value: Value, actor: Actor, parent_override: Option<String>) -> Result<String> {
        let input = Input::parse(value)?;
        let key = input.key.as_ref().map(|key| key.trim().to_owned());
        require(!key.as_ref().is_some_and(String::is_empty), "Task key must not be blank")?;
        let cwd = input.cwd.clone().ok_or("cwd is required")?;
        let directory = workspace(Path::new(&cwd))?;
        if let Some(key) = &key
            && let Some(index) = self.tasks.iter().position(|task| task.key.as_ref() == Some(key) && task.workspace.as_ref() == Some(&directory))
        {
            let id = self.tasks[index].id.clone();
            self.revise(index, input, actor, "keyed_create")?;
            return Ok(id);
        }
        let supersedes = input.supersedes.clone().unwrap_or_default();
        let mut task: Task = serde_json::from_value(
            json!({"id":id(),"title":"","outcome":"","cwd":cwd,"acceptance":[],"dependencies":[],"stage":"implementation","stopped":false,"attempts":[],"active":null,"feedback":[],"failure":"","retryAt":0,"key":key,"workspace":directory}),
        )?;
        input.apply(&mut task)?;
        if parent_override.is_some() {
            task.parent = parent_override;
        }
        self.check_definition(&task)?;
        self.check_relations(&task)?;
        let id = task.id.clone();
        for old in &supersedes {
            let index = self.index(old)?;
            require(self.tasks[index].active.is_none() && self.children(old).next().is_none(), "Superseded tasks must be idle and have no children")?;
            for other in &mut self.tasks {
                for list in [&mut other.dependencies, &mut other.after] {
                    for entry in list.iter_mut().filter(|entry| *entry == old) {
                        *entry = id.clone();
                    }
                }
            }
            let mut retired = self.tasks.remove(index);
            retired.record("superseded", actor, "create", json!({"by":id}));
            self.archived.push(retired);
        }
        let mut snapshot = task.snapshot();
        snapshot["supersedes"] = json!(supersedes);
        task.record("created", actor, "create", snapshot);
        self.tasks.push(task);
        Ok(id)
    }
    fn revise(&mut self, index: usize, input: Input, actor: Actor, source: &str) -> Result<()> {
        require(input.supersedes.is_none(), "supersedes applies only when creating a task")?;
        let previous = self.tasks[index].clone();
        let mut task = previous.clone();
        input.apply(&mut task)?;
        if task.snapshot() == previous.snapshot() {
            return Ok(());
        }
        require(previous.active.is_none(), "Stop the task and wait for its worker to exit before editing")?;
        self.check_definition(&task)?;
        self.check_relations(&task)?;
        let corrective = task.signature() != previous.signature() || (previous.stage == "replan" && !task.refinements.is_empty());
        if corrective {
            task.refinements.clear();
            for dependent in self.dependents(&task.id) {
                let other = &mut self.tasks[dependent];
                require(other.active.is_none(), format!("Stop dependent task \"{}\" before changing its prerequisite", other.title))?;
                if other.stage != "implementation" || other.attempts.iter().any(|a| a.revision == other.revision) {
                    let from = other.stage.clone();
                    other.reset_work();
                    other.record("reopened", actor, source, json!({"causedBy":task.id}));
                    other.record_phase(&from, actor, source);
                }
            }
            task.reset_work();
        }
        task.record(if corrective { "revised" } else { "updated" }, actor, source, task.snapshot());
        task.record_phase(&previous.stage, actor, source);
        self.tasks[index] = task;
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
                task.record(if done { "children_done" } else { "reopened" }, Actor::Scheduler, "containment", json!({}));
                task.record_phase(&from, Actor::Scheduler, "containment");
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
            read(&file)?
        } else {
            let command = vec![std::env::current_exe()?.to_string_lossy().into(), "--codex".into()];
            let config = Config { implementation: command.clone(), review: command.clone(), replan: command, capabilities: default_capabilities(), notify: Vec::new() };
            State { schema_version: Some(history::SCHEMA_VERSION), paused: false, controls: Vec::new(), hold_until: 0, tasks: Vec::new(), archived: Vec::new(), config }
        };
        require(!(state.schema_version > Some(history::SCHEMA_VERSION)), "State schema is newer than this coordinator")?;
        if state.config.replan.is_empty() {
            state.config.replan = state.config.implementation.clone();
        }
        for index in 0..state.tasks.len() {
            let task = &mut state.tasks[index];
            if task.legacy_audit {
                task.attributes.lane = Lane::Audit;
            }
            if task.legacy_evidence.to_string().contains("candidate") && task.attributes.artifact == Artifact::None {
                task.attributes.artifact = Artifact::Candidate;
            }
            let task = &state.tasks[index];
            let invalid = task.blocked || !task.refinements.is_empty() || state.check_definition(task).is_err();
            if task.stage != "done" && task.active.is_none() && invalid && task.stage != "replan" {
                let task = &mut state.tasks[index];
                let from = task.stage.clone();
                task.stage = "replan".into();
                task.blocked = false;
                task.feedback = vec!["Legacy definition violates current limits; rewrite it with typed attributes and relations".into()];
                task.record_phase(&from, Actor::Scheduler, "migration");
            }
        }
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
    pub fn task_value(&self, task: &Task, detailed: bool) -> Result<Value> {
        let mut value = serde_json::to_value(task)?;
        if !detailed {
            value.as_object_mut().unwrap().remove("history");
        }
        let blocks: Vec<_> = self.state.tasks.iter().filter(|t| t.dependencies.contains(&task.id)).map(|t| &t.id).collect();
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
        let actor = Actor::api(&input)?;
        let id = self.change(|state| Ok(json!(state.insert(input, actor, None)?)))?;
        self.task_value(&self.state.tasks[self.task(id.as_str().unwrap())?], false)
    }
    pub fn edit(&mut self, id: &str, input: Value) -> Result<Value> {
        let actor = Actor::api(&input)?;
        let index = self.task(id)?;
        self.change(|state| state.revise(index, Input::parse(input)?, actor, "edit").map(|()| Value::Null))?;
        self.task_value(&self.state.tasks[index], false)
    }
    pub fn request_replan(&mut self, id: &str, input: Value) -> Result<Value> {
        let actor = Actor::api(&input)?;
        let index = self.task(id)?;
        self.change(|state| {
            let task = &mut state.tasks[index];
            require(task.active.is_none() && task.stage != "done" && !task.container(), "Only idle, unfinished, non-container tasks can be replanned")?;
            let from = task.stage.clone();
            task.stage = "replan".into();
            task.blocked = false;
            task.retry_at = 0;
            if let Some(reason) = input["reason"].as_str().filter(|r| !r.trim().is_empty()) {
                task.feedback = vec![history::clip(reason, history::FINDING_WORDS)];
            }
            task.record("replan_requested", actor, "api", json!({"reason":input["reason"]}));
            task.record_phase(&from, actor, "api");
            Ok(Value::Null)
        })?;
        self.task_value(&self.state.tasks[index], false)
    }
    pub fn remove(&mut self, id: &str, actor: Actor) -> Result<()> {
        let index = self.task(id)?;
        let task = &self.state.tasks[index];
        require(task.active.is_none(), "Task has an active worker")?;
        let referenced = self.state.tasks.iter().any(|t| t.parent.as_deref() == Some(id) || t.dependencies.iter().chain(&t.after).any(|d| d == id));
        require(!referenced, "Task has children or dependents; supersede it instead")?;
        self.change(|state| {
            let mut task = state.tasks.remove(index);
            for other in &mut state.tasks {
                other.conflicts.retain(|c| c != id);
                other.related.retain(|c| c != id);
            }
            task.record(if task.stage == "done" { "archived" } else { "abandoned" }, actor, "delete", Value::Null);
            state.archived.push(task);
            Ok(Value::Null)
        })
        .map(drop)
    }
    pub fn configure(&mut self, value: Value) -> Result<()> {
        let mut config: Config = serde_json::from_value(value)?;
        if config.replan.is_empty() {
            config.replan = config.implementation.clone();
        }
        require(![&config.implementation, &config.review, &config.replan].iter().any(|args| args.is_empty() || args.iter().any(|s| s.trim().is_empty())), "Worker commands must be nonempty string arrays")?;
        self.change(|state| {
            state.config = config;
            Ok(Value::Null)
        })
        .map(drop)
    }
    pub fn control_as(&mut self, stop: bool, id: Option<&str>, actor: Actor, reason: &str) -> Result<()> {
        require(id.is_some() || actor == Actor::Human, "Only an explicit user action can control all workspaces")?;
        require(!reason.trim().is_empty(), "Control requires a reason")?;
        if let (false, Some(id)) = (stop, id)
            && actor != Actor::Human
        {
            let task = &self.state.tasks[self.task(id)?];
            let user_stop = task.stopped && task.history.iter().rev().find(|e| matches!(e.kind.as_str(), "stopped" | "resumed")).is_some_and(|e| e.actor == Actor::Human);
            require(!user_stop, "An explicit user task stop requires user resume authority")?;
        }
        let paused = self.state.paused;
        self.change(|state| {
            if id.is_none() {
                if paused != stop {
                    state.controls.push(json!({"at":now(),"actor":actor,"reason":reason,"paused":stop}));
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
                    task.record(if stop { "stopped" } else { "resumed" }, actor, "control", json!({"scope":if id.is_some() {"task"} else {"board"},"reason":reason}));
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
    fn request(&self, task: &Task, attempt_id: &str) -> Value {
        let stage = task.stage.as_str();
        let summary = |a: &Attempt| a.result.as_ref().map(|r| json!({"stage":a.stage,"status":r["status"],"summary":r["summary"],"findings":r["findings"],"criteria":r["criteria"]}));
        let dependencies: Vec<_> = task
            .dependencies
            .iter()
            .filter_map(|id| self.state.get(id))
            .map(|d| {
                let accepted = d.attempts.iter().rev().find(|a| a.revision == d.revision && a.stage == "implementation" && a.status() == Some("completed"));
                json!({"id":d.id,"title":d.title,"summary":accepted.and_then(|a| a.result.as_ref()).map(|r| &r["summary"]),"artifact":accepted.and_then(|a| a.evidence.configuration.as_ref()).map(|c| &c["artifact"])})
            })
            .collect();
        let directory = self.attempt_path(attempt_id);
        let mut request = json!({"stage":stage,"task":{"id":task.id,"revision":task.revision,"title":task.title,"outcome":task.outcome,"acceptance":task.acceptance,
            "cwd":task.workdir(),"model":task.model,"effort":task.effort,"lane":task.attributes.lane,"needs":task.attributes.needs,"artifact":evidence::artifact_name(task.attributes.artifact),
            "writable":task.writable},
            "feedback":task.feedback,"dependencies":dependencies,"limits":history::versions()["limits"],"scratch":directory.join("scratch"),
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
        request
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
        if process::local_codex(registered) {
            command[0] = std::env::current_exe()?.to_string_lossy().into_owned();
        }
        let settings = json!({"scratch":true,"acceptance":task.acceptance,"requested":{"model":task.model,"effort":task.effort},"needs":task.attributes.needs,
            "registeredCommand":registered,"actualCommand":command,"stage":stage,"request":"request.json","revision":task.revision,
            "workspaceId":task.workspace,"cwd":task.workdir(),"writable":task.writable,
            "coordinatorVersion":env!("CARGO_PKG_VERSION"),"coordinatorBuild":crate::web::deployment()["digest"]});
        let attempt =
            Attempt { id: id(), revision: task.revision, stage: stage.clone(), started: now(), ended: None, result: None, error: None, evidence: AttemptEvidence { configuration: Some(settings), ..Default::default() } };
        let mut request = self.request(task, &attempt.id);
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
            task.record("dispatched", Actor::Scheduler, "dispatch", json!({"stage":stage,"started":attempt.started}));
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
            let event = Event::new(task, "started", Actor::Worker, "supervisor", json!("run.json")).attempt(&id, revision, started);
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
        let mut result: Value = read(&self.attempt_path(&attempt.id).join("result.json"))?;
        let status = result["status"].as_str().unwrap_or_default().to_owned();
        require(worker_statuses(&attempt.stage).contains(&status.as_str()), "Invalid worker status")?;
        require(result["summary"].as_str().is_some_and(|s| !s.trim().is_empty()), "Worker summary is required")?;
        let list = |value: &Value, items: usize| -> Vec<String> {
            value.as_array().into_iter().flatten().filter_map(Value::as_str).filter(|s| !s.trim().is_empty()).take(items).map(|s| history::clip(s, history::FINDING_WORDS)).collect()
        };
        result["summary"] = json!(history::clip(result["summary"].as_str().unwrap(), history::SUMMARY_WORDS));
        result["findings"] = json!(list(&result["findings"], history::MAX_FINDINGS));
        result["checks"] = json!(list(&result["checks"], history::MAX_FINDINGS + 2));
        if attempt.stage == "review" {
            let criteria: Vec<String> = serde_json::from_value(result["criteria"].clone()).map_err(|_| "Review must report criteria outcomes")?;
            require(criteria.len() == task.acceptance.len() && criteria.iter().all(|c| matches!(c.as_str(), "passed" | "failed" | "blocked")), "Review must report one outcome per acceptance criterion")?;
            let failed = criteria.iter().any(|c| c == "failed");
            if status == "accepted" && criteria.iter().any(|c| c != "passed") {
                result["status"] = json!(if failed { "rejected" } else { "blocked" });
            }
            require(!(status == "rejected" && !failed), "A rejection must mark at least one criterion failed")?;
        }
        if matches!(result["status"].as_str(), Some("rejected" | "blocked" | "held")) {
            require(result["findings"].as_array().is_some_and(|f| !f.is_empty()), "Rejected, blocked and held results require findings")?;
        }
        Ok(result)
    }
    fn apply_plan(state: &mut State, index: usize, plan: &Value) -> Result<()> {
        let task = state.tasks[index].clone();
        let children = plan["children"].as_array().cloned().unwrap_or_default();
        let base = |item: &Value, parent: bool| {
            json!({"title":item["title"],"outcome":item["outcome"],"acceptance":if parent {json!([])} else {item["acceptance"].clone()},
                "attributes":{"lane":item["lane"],"needs":item["needs"],"size":if parent {json!("large")} else {item["size"].clone()},
                "priority":task.attributes.priority,
                "artifact":if parent {json!("none")} else {item.get("artifact").cloned().unwrap_or(json!(task.attributes.artifact))},"deploy":!parent && item["artifact"] == "candidate" && task.attributes.deploy}})
        };
        if children.is_empty() {
            state.revise(index, Input::parse(base(plan, false))?, Actor::Planner, "replan")?;
            return require(state.tasks[index].stage != "replan", "Plan did not change the task definition");
        }
        state.revise(index, Input::parse(base(plan, true))?, Actor::Planner, "replan")?;
        let mut ids: Vec<String> = Vec::new();
        for child in &children {
            let mut value = base(child, false);
            value["cwd"] = json!(task.cwd);
            value["writable"] = json!(task.writable);
            value["after"] = json!(child["after"].as_array().into_iter().flatten().filter_map(Value::as_u64).filter_map(|i| ids.get(i as usize).cloned()).collect::<Vec<_>>());
            ids.push(state.insert(value, Actor::Planner, Some(task.id.clone()))?);
        }
        Ok(())
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
                                let task = &mut state.tasks[index];
                                task.attempts.last_mut().unwrap().result.as_mut().unwrap()["planError"] = json!(error.to_string());
                                task.feedback = vec![history::clip(&error.to_string(), history::FINDING_WORDS)];
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
        if task.stage == "replan" && task.failed_replans() >= history::STRIKES && !task.blocked {
            task.blocked = true;
            task.failure = "Replanning failed twice; a decision is needed".into();
            notice = Some(json!({"event":"held","taskId":task.id,"title":task.title,"reason":task.failure}));
        }
        let attempt = task.attempts.last().unwrap();
        let kind = attempt.status().unwrap_or(if stopped { "attempt_stopped" } else { "attempt_failed" }).to_owned();
        let event = Event::new(task, &kind, Actor::Worker, "attempt_collection", json!({"currentRevision":current,"error":attempt.error,"ended":attempt.ended})).attempt(&attempt.id, attempt.revision, attempt.ended);
        task.history.push(event);
        task.record_phase(&previous_stage, Actor::Scheduler, "attempt_collection");
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
                task.record("waiting_changed", Actor::Scheduler, "scheduler", json!({"reason":reason}));
                task.recorded_wait = reason;
            }
        }
        self.commit(state)
    }
}
#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
