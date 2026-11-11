use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const STAGES: [&str; 5] = ["backlog", "planning", "implementation", "review", "done"];
pub const EXECUTABLE_STAGES: [&str; 3] = ["planning", "implementation", "review"];
pub fn canonical_stage(stage: &str) -> &str {
    if stage == "verification" {
        "review"
    } else {
        stage
    }
}
pub fn normalize_stages(stages: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    for stage in stages {
        let stage = canonical_stage(&stage).to_owned();
        if !result.contains(&stage) {
            result.push(stage);
        }
    }
    result
}
pub fn normalize_routes(routes: &mut BTreeMap<String, String>) {
    if let Some(agent) = routes.remove("verification") {
        routes.entry("review".into()).or_insert(agent);
    }
}
pub fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn stage_index(stage: &str) -> Option<usize> {
    STAGES.iter().position(|s| *s == canonical_stage(stage))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Estimate {
    pub points: u8,
    pub confidence: String,
    pub reason: String,
    pub at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub key: String,
    pub title: String,
    pub description: String,
    pub acceptance: String,
    pub constraints: String,
    pub kind: String,
    pub task_type: String,
    pub parent_id: Option<String>,
    pub dependencies: Vec<String>,
    pub tags: Vec<String>,
    pub priority: u8,
    pub complexity: u8,
    pub estimates: Vec<Estimate>,
    pub stage: String,
    pub status: String,
    pub revision: u64,
    pub visit: u32,
    pub sendbacks: u32,
    pub max_attempts: u32,
    pub max_sendbacks: u32,
    pub timeout_seconds: u64,
    pub token_budget: Option<u64>,
    pub assignments: BTreeMap<String, String>,
    pub workspace: String,
    pub current_attempt: Option<String>,
    pub blocked_reason: String,
    pub created_at: String,
    pub updated_at: String,
    pub archived: bool,
    #[serde(default)]
    pub human_approval: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<Approval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported: Option<ImportedTask>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_of: Option<RecoveryIncident>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_id: Option<String>,
}

/// A provider-neutral receiver belonging to the planner that originated work.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlannerRegistration {
    pub id: String,
    pub source: String,
    pub agent_id: String,
    pub name: String,
    pub callback_url: Option<String>,
    pub context: Value,
    pub workspace_default: bool,
    pub enabled: bool,
    pub created_at: String,
}

/// An outbox item is committed in the same transaction as the worker failure.
/// Acknowledgement records receipt, never resolution or task completion.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlannerNotification {
    pub id: String,
    pub planner_id: String,
    pub task_id: String,
    pub origin_task_id: String,
    pub attempt_id: String,
    pub reason: String,
    pub payload: Value,
    pub created_at: String,
    pub last_delivery_at: Option<String>,
    pub delivery_attempts: u64,
    pub next_delivery_at: String,
    pub acknowledged_at: Option<String>,
    pub last_error: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryIncident {
    pub task_id: String,
    pub attempt_id: String,
    pub stage: String,
    #[serde(default)]
    pub resumed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Approval {
    pub reason: String,
    pub at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportedTask {
    pub plan_id: String,
    pub code: String,
    pub spec: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub adapter: String,
    pub model: String,
    pub stages: Vec<String>,
    pub instructions: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub unset_env: Vec<String>,
    pub endpoint: String,
    pub enabled: bool,
    pub capacity: usize,
    pub timeout_seconds: u64,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence: Option<Presence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerRegistration>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerRegistration {
    pub source: String,
    pub seen_at: String,
    pub lease_seconds: u64,
    pub assignment: Option<Assignment>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Assignment {
    pub id: String,
    pub task_id: String,
    pub task_revision: u64,
    pub stage: String,
    pub visit: u32,
    pub workspace: String,
    pub offered_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Presence {
    pub source: String,
    pub status: String,
    pub seen_at: String,
    pub lease_seconds: u64,
    pub run_id: Option<String>,
    pub task_id: Option<String>,
    pub usage: Usage,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub source: String,
}
impl Default for Usage {
    fn default() -> Self {
        Self {
            input_tokens: None,
            output_tokens: None,
            cost_usd: None,
            source: "unavailable".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attempt {
    pub id: String,
    pub task_id: String,
    pub stage: String,
    pub visit: u32,
    pub number: u32,
    pub agent_id: String,
    pub agent_name: String,
    pub adapter: String,
    pub model: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: Option<u64>,
    pub summary: String,
    pub error: String,
    pub usage: Usage,
    pub input: Value,
    pub output: Option<Value>,
    pub log: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<ExternalLease>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExternalLease {
    pub source: String,
    pub request_id: String,
    pub heartbeat_at: String,
    pub lease_seconds: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub task_id: String,
    pub attempt_id: Option<String>,
    pub stage: String,
    pub visit: u32,
    pub name: String,
    pub content: String,
    pub media_type: String,
    pub stale: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub task_id: Option<String>,
    pub kind: String,
    pub actor: String,
    pub message: String,
    pub at: String,
    pub data: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub paused: bool,
    pub concurrency: usize,
    pub stage_agents: BTreeMap<String, String>,
    pub independent_review: bool,
    pub project_name: String,
    #[serde(default = "yes")]
    pub auto_advance: bool,
    #[serde(default = "retry_delay")]
    pub retry_delay_seconds: u64,
}
fn yes() -> bool {
    true
}
fn retry_delay() -> u64 {
    5
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            paused: false,
            concurrency: 2,
            stage_agents: BTreeMap::new(),
            independent_review: true,
            project_name: "My workspace".into(),
            auto_advance: true,
            retry_delay_seconds: retry_delay(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Snapshot {
    #[serde(skip)]
    pub occupied_workspaces: Vec<std::path::PathBuf>,
    pub tasks: Vec<Task>,
    pub agents: Vec<Agent>,
    pub attempts: Vec<Attempt>,
    pub artifacts: Vec<Artifact>,
    pub events: Vec<Event>,
    #[serde(default)]
    pub planners: Vec<PlannerRegistration>,
    #[serde(default)]
    pub planner_notifications: Vec<PlannerNotification>,
    pub settings: Settings,
    pub revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResultArtifact {
    pub name: String,
    pub content: String,
    #[serde(default = "markdown")]
    pub media_type: String,
}
fn markdown() -> String {
    "text/markdown".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EstimateProposal {
    pub points: u8,
    pub confidence: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentResult {
    pub outcome: String,
    pub summary: String,
    #[serde(default)]
    pub send_back_to: Option<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub artifacts: Vec<ResultArtifact>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub estimate: Option<EstimateProposal>,
}
