use crate::{Result, now};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
pub const SCHEMA_VERSION: u32 = 9;
pub const METRIC_DEFINITION_VERSION: u32 = 9;
pub const STRIKES: usize = 2;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Limits {
    pub words: usize,
    pub title_words: usize,
    pub criteria: usize,
    pub summary_words: usize,
    pub findings: usize,
    pub finding_words: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { words: 120, title_words: 12, criteria: 3, summary_words: 40, findings: 3, finding_words: 30 }
    }
}
impl Limits {
    pub fn validate(&self) -> Result<()> {
        crate::require([self.words, self.title_words, self.criteria, self.summary_words, self.findings, self.finding_words].iter().all(|v| *v > 0), "Limits must be positive integers")
    }
    pub fn value(&self) -> Value {
        let mut value = json!(self);
        value["strikes"] = json!(STRIKES);
        value
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Updates {
    #[default]
    Poll,
    Events,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct WorkspaceSettings {
    pub limits: Limits,
    pub updates: Updates,
    pub planner: Option<String>,
    pub worker: Option<String>,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub agents: std::collections::BTreeMap<String, crate::agents::Agent>,
    pub prompts: Option<crate::workspace_prompts::Selection>,
}
impl WorkspaceSettings {
    pub fn validate(&self, family: Option<crate::agents::Family>) -> Result<()> {
        self.limits.validate()?;
        self.agent("planner", family)?;
        self.agent("worker", family).map(drop)
    }
    pub fn agent(&self, role: &str, family: Option<crate::agents::Family>) -> Result<crate::agents::Agent> {
        let id = if role == "planner" { &self.planner } else { &self.worker };
        crate::agents::resolve(&self.agents, id.as_deref().ok_or(format!("Workspace {role} selection is required"))?, family)
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    #[default]
    Code,
    Ui,
    Data,
    Docs,
    Audit,
    Cleanup,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Need {
    Network,
    Browser,
    Process,
    Gpu,
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    #[default]
    Small,
    Medium,
    Large,
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Artifact {
    #[default]
    None,
    Candidate,
    Report,
}
pub const LANGUAGES: &[&str] = &["c", "cpp", "csharp", "css", "go", "html", "java", "javascript", "kotlin", "php", "powershell", "python", "ruby", "rust", "shell", "sql", "swift", "typescript"];
pub const FUNCTIONS: &[&str] = &["api", "build", "deployment", "documentation", "observability", "security", "storage", "testing", "ui"];
fn categories<'de, D: serde::Deserializer<'de>>(input: D, values: &[&str]) -> std::result::Result<std::collections::BTreeSet<String>, D::Error> {
    let selected = std::collections::BTreeSet::<String>::deserialize(input)?;
    if let Some(value) = selected.iter().find(|value| !values.contains(&value.as_str())) {
        return Err(serde::de::Error::custom(format!("Unknown classification {value}; expected one of {}", values.join(", "))));
    }
    Ok(selected)
}
fn languages<'de, D: serde::Deserializer<'de>>(input: D) -> std::result::Result<std::collections::BTreeSet<String>, D::Error> {
    categories(input, LANGUAGES)
}
fn functions<'de, D: serde::Deserializer<'de>>(input: D) -> std::result::Result<std::collections::BTreeSet<String>, D::Error> {
    categories(input, FUNCTIONS)
}
pub fn classification_schema() -> Value {
    json!({"version":1,"languages":{"type":"array","items":{"type":"string","enum":LANGUAGES},"default":[]},
        "functions":{"type":"array","items":{"type":"string","enum":FUNCTIONS},"default":[]},
        "semantics":"Independent unordered sets; duplicates collapse and values serialize lexically. Select every applicable language and function, including mixed-language work. Empty or omitted sets mean uncategorized, never inferred from lane, text, or capabilities. Values are case-sensitive. Attributes replacement resets omitted sets; edits omitting attributes preserve them. Classifications do not affect dispatch or reopen work.",
        "extensibility":"Closed versioned registries: add values to the server registry and increment this version; clients discover supported values here. Unknown values are rejected. Worker requirements remain exclusively in needs."})
}
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Attributes {
    pub lane: Lane,
    pub needs: Vec<Need>,
    #[serde(deserialize_with = "languages")]
    pub languages: std::collections::BTreeSet<String>,
    #[serde(deserialize_with = "functions")]
    pub functions: std::collections::BTreeSet<String>,
    pub priority: Option<u8>,
    pub size: Size,
    pub artifact: Artifact,
    pub deploy: bool,
}
impl Attributes {
    pub const FIELDS: [&'static str; 8] = ["lane", "needs", "languages", "functions", "priority", "size", "artifact", "deploy"];
    pub fn priority(&self) -> u8 {
        self.priority.unwrap_or(2)
    }
    pub fn validate(&self) -> Result<()> {
        crate::require(self.languages.iter().all(|v| LANGUAGES.contains(&v.as_str())) && self.functions.iter().all(|v| FUNCTIONS.contains(&v.as_str())), "Unknown classification")?;
        crate::require(self.priority.is_none_or(|p| p <= 3), "Priority must be 0 (highest) to 3")?;
        crate::require(!(self.deploy && self.artifact != Artifact::Candidate), "deploy requires artifact candidate")
    }
}
pub fn effort(value: &str) -> Result<&str> {
    match value {
        "minimal" | "low" | "medium" | "high" | "xhigh" => Ok(value),
        _ => Err("Effort must be minimal, low, medium, high, or xhigh".into()),
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    Human,
    Planner,
    Scheduler,
    Worker,
}
impl Actor {
    pub fn api(input: &Value) -> Result<Self> {
        match input.get("actor").and_then(Value::as_str) {
            None | Some("planner") => Ok(Self::Planner),
            Some("human") => Ok(Self::Human),
            _ => Err("Task API actor must be human or planner".into()),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    sequence: usize,
    task_id: String,
    workspace_id: Option<PathBuf>,
    pub revision: u64,
    pub attempt_id: Option<String>,
    pub at: u64,
    pub kind: String,
    pub(crate) actor: Actor,
    provenance: String,
    pub evidence: Value,
}
impl Event {
    pub fn new(task: &crate::engine::Task, kind: &str, actor: Actor, source: &str, evidence: Value) -> Self {
        Self {
            sequence: task.history.len() + 1,
            task_id: task.id.clone(),
            workspace_id: task.workspace.clone(),
            revision: task.revision,
            attempt_id: task.active.clone(),
            at: now(),
            kind: kind.into(),
            actor,
            provenance: source.into(),
            evidence,
        }
    }
    pub fn attempt(mut self, id: &str, revision: u64, at: Option<u64>) -> Self {
        self.attempt_id = Some(id.into());
        self.revision = revision;
        self.at = at.unwrap_or(self.at);
        self
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AttemptEvidence {
    pub configuration: Option<Value>,
    pub observed: Option<Observed>,
    pub launch: Option<Launch>,
    pub changes: Option<Value>,
    pub project_size: Option<Value>,
    pub usage: Option<Usage>,
    pub cost: Option<Value>,
    pub overlapping_attempts: Vec<String>,
    pub project_overlapping_attempts: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Value>,
    pub source: String,
}
impl Usage {
    pub fn tokens(&self) -> Option<u64> {
        self.total_tokens.or_else(|| self.components.as_ref().and_then(crate::telemetry::total))
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Launch {
    pub started: u64,
    pub worker: u32,
    pub supervisor: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observed {
    pub worker: String,
    pub cli_version: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub forwarded_model: Option<String>,
    pub forwarded_effort: Option<String>,
    pub source: String,
}
pub fn versions() -> Value {
    json!({"schemaVersion":SCHEMA_VERSION,"dependencyReasons":crate::engine::DEPENDENCY_REASONS,"metricDefinitionVersion":METRIC_DEFINITION_VERSION,
        "classifications":classification_schema(),
        "limits":Limits::default().value(),"workspace":WorkspaceSettings::default()})
}
pub fn clip(text: &str, words: usize) -> String {
    text.split_whitespace().take(words).collect::<Vec<_>>().join(" ")
}
pub fn words(text: &str) -> usize {
    text.split_whitespace().count()
}
pub fn coordination_prose(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("agents.md")
        || text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).any(|token| {
            let parts: Vec<_> = token.split('-').collect();
            (parts.len() >= 3 && parts[0].len() == 13 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))) || (token.len() >= 40 && token.bytes().all(|b| b.is_ascii_hexdigit()))
        })
}
