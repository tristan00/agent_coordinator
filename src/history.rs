use crate::{Result, now};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
pub const SCHEMA_VERSION: u32 = 2;
pub const METRIC_DEFINITION_VERSION: u32 = 8;
pub const MAX_WORDS: usize = 120;
pub const MAX_TITLE_WORDS: usize = 12;
pub const MAX_CRITERIA: usize = 3;
pub const SUMMARY_WORDS: usize = 40;
pub const MAX_FINDINGS: usize = 3;
pub const FINDING_WORDS: usize = 30;
pub const STRIKES: usize = 2;
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
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Attributes {
    pub lane: Lane,
    pub needs: Vec<Need>,
    pub priority: Option<u8>,
    pub size: Size,
    pub artifact: Artifact,
    pub deploy: bool,
}
impl Attributes {
    pub const FIELDS: [&'static str; 6] = ["lane", "needs", "priority", "size", "artifact", "deploy"];
    pub fn priority(&self) -> u8 {
        self.priority.unwrap_or(2)
    }
    pub fn validate(&self) -> Result<()> {
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
    json!({"schemaVersion":SCHEMA_VERSION,"metricDefinitionVersion":METRIC_DEFINITION_VERSION,
        "limits":{"words":MAX_WORDS,"titleWords":MAX_TITLE_WORDS,"criteria":MAX_CRITERIA,"summaryWords":SUMMARY_WORDS,"findings":MAX_FINDINGS,"findingWords":FINDING_WORDS,"strikes":STRIKES}})
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
