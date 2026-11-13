use crate::{Result, require};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Claude,
    Codex,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Anthropic,
    Openai,
    Local,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Harness {
    Codex,
    ClaudeCode,
    OpenaiCompatible,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Agent {
    pub provider: Provider,
    pub harness: Harness,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retired: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}
impl Agent {
    pub fn effort(&self) -> &str {
        self.effort.as_deref().unwrap_or("")
    }
    fn validate(&self) -> Result<()> {
        require(!self.model.trim().is_empty(), "Agent model is required")?;
        let pairs = matches!(
            (self.provider, self.harness),
            (Provider::Openai, Harness::Codex) | (Provider::Anthropic, Harness::ClaudeCode) | (Provider::Local, Harness::OpenaiCompatible)
        );
        require(pairs, "Hosted models run only in their own vendor's harness")?;
        require(self.provider != Provider::Local || self.endpoint.as_deref().is_some_and(local_endpoint), "Local agents need a loopback http endpoint")
    }
    pub fn allowed(&self, family: Option<Family>) -> bool {
        match self.provider {
            Provider::Local => true,
            Provider::Anthropic => family == Some(Family::Claude),
            Provider::Openai => family == Some(Family::Codex),
        }
    }
}
pub fn local_endpoint(url: &str) -> bool {
    ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"].iter().any(|prefix| url.starts_with(prefix))
}
fn hosted(provider: Provider, harness: Harness, model: &str, effort: &str, retired: bool, note: &str) -> Agent {
    Agent { provider, harness, model: model.into(), effort: Some(effort.into()), endpoint: None, retired, note: Some(note.into()) }
}
fn local(model: &str, note: &str) -> Agent {
    Agent { provider: Provider::Local, harness: Harness::OpenaiCompatible, model: model.into(), effort: None, endpoint: Some("http://127.0.0.1:8120/v1".into()), retired: false, note: Some(note.into()) }
}
pub fn catalog() -> BTreeMap<String, Agent> {
    use {Harness::*, Provider::*};
    BTreeMap::from([
        ("sol_light".into(), hosted(Openai, Codex, "gpt-6-sol", "low", true, "Retired")),
        ("luna_6_high".into(), hosted(Openai, Codex, "gpt-6-luna", "high", false, "Hosted, largest, high reasoning; hardest design and debugging")),
        ("sol_6_1_light".into(), hosted(Openai, Codex, "gpt-6.1-sol", "low", false, "Hosted, mid size, low reasoning; routine implementation")),
        ("sol_6_1_medium".into(), hosted(Openai, Codex, "gpt-6.1-sol", "medium", false, "Hosted, mid size, medium reasoning; multi-file features")),
        ("claude_haiku_5_5_low".into(), hosted(Anthropic, ClaudeCode, "claude-haiku-5-5", "low", false, "Hosted, small, fast; routine edits, summaries, checks")),
        ("claude_sonnet_5_5_low".into(), hosted(Anthropic, ClaudeCode, "claude-sonnet-5-5", "low", false, "Hosted, mid size, low effort; routine implementation")),
        ("claude_sonnet_5_5_medium".into(), hosted(Anthropic, ClaudeCode, "claude-sonnet-5-5", "medium", false, "Hosted, mid size, medium effort; multi-file features")),
        ("claude_opus_5_5_medium".into(), hosted(Anthropic, ClaudeCode, "claude-opus-5-5", "medium", false, "Hosted, largest; hardest design and debugging")),
        ("local_granite_3b".into(), local("granite4.2-3b-fp8", "Local 3B, about 4 GB; one-line edits, renames, short summaries")),
        ("local_ministral_3b".into(), local("ministral3-3b", "Local 3B, about 7 GB; short summaries and simple checks")),
        ("local_phi4_mini".into(), local("phi4-mini", "Local 3.8B, about 8 GB; small single-file edits")),
        ("local_qwen3_4b".into(), local("qwen3-4b-2507-fp8", "Local 4B, about 5 GB; small single-file edits and summaries")),
        ("local_qwen3_8b".into(), local("qwen3-8b-fp8", "Local 8B, about 9 GB; small features in one or two files")),
    ])
}
pub fn detect(variable: impl Fn(&str) -> Option<String>) -> Option<Family> {
    if let Some(explicit) = variable("AGENT_FAMILY") {
        return serde_json::from_value(json!(explicit.trim().to_ascii_lowercase())).ok();
    }
    let claude = variable("CLAUDECODE").is_some_and(|v| v == "1") || variable("CLAUDE_CODE_ENTRYPOINT").is_some();
    let codex = ["CODEX_WINDOWS_REGISTERED_CORE", "CODEX_THREAD_ID", "CODEX_SANDBOX", "CODEX_SANDBOX_NETWORK_DISABLED", "CODEX_MANAGED_BY_NPM", "CODEX_MANAGED_BY_BUN"]
        .iter()
        .any(|name| variable(name).is_some());
    match (claude, codex) {
        (true, false) => Some(Family::Claude),
        (false, true) => Some(Family::Codex),
        _ => None,
    }
}
static LAUNCHER: OnceLock<Option<Family>> = OnceLock::new();
#[cfg(not(test))]
pub fn launcher() -> Option<Family> {
    *LAUNCHER.get_or_init(|| detect(|name| std::env::var(name).ok().filter(|v| !v.is_empty())))
}
#[cfg(test)]
pub fn launcher() -> Option<Family> {
    *LAUNCHER.get_or_init(|| Some(Family::Codex))
}
pub fn family(persisted: Option<Family>) -> Option<Family> {
    launcher().or(persisted)
}
pub fn resolve(defined: &BTreeMap<String, Agent>, id: &str, family: Option<Family>) -> Result<Agent> {
    let agent = defined.get(id).cloned().or_else(|| catalog().remove(id)).ok_or_else(|| format!("Unknown agent {id}"))?;
    agent.validate()?;
    require(!agent.retired, format!("Agent {id} is retired"))?;
    require(
        agent.allowed(family),
        format!("Agent {id} is not allowed when the coordinator was started by {}; only {} and local agents are enabled", family.map_or("an undetected launcher".into(), |f| json!(f).as_str().unwrap().to_owned()), family.map_or("no hosted", |f| if f == Family::Claude { "Claude" } else { "Codex" })),
    )?;
    Ok(agent)
}
pub fn enabled(defined: &BTreeMap<String, Agent>, family: Option<Family>) -> BTreeMap<String, Agent> {
    let source = if defined.is_empty() { catalog() } else { defined.clone() };
    source.into_iter().filter(|(_, agent)| !agent.retired && agent.validate().is_ok() && agent.allowed(family)).collect()
}
pub fn served(models: &Value, model: &str) -> bool {
    models["data"].as_array().is_some_and(|items| items.iter().any(|item| item["id"] == model))
}
#[cfg(not(test))]
fn listed_models(endpoint: &str) -> Option<Value> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<BTreeMap<String, (Instant, Option<Value>)>> = Mutex::new(BTreeMap::new());
    if let Some((_, models)) = CACHE.lock().unwrap().get(endpoint).filter(|(at, _)| at.elapsed() < Duration::from_secs(10)) {
        return models.clone();
    }
    let output = crate::process::command("curl.exe").args(["-q", "-sS", "--max-time", "1", "--noproxy", "*", &format!("{}/models", endpoint.trim_end_matches('/'))]).output().ok();
    let models = output.filter(|o| o.status.success()).and_then(|o| serde_json::from_slice(&o.stdout).ok());
    CACHE.lock().unwrap().insert(endpoint.to_owned(), (Instant::now(), models.clone()));
    models
}
#[cfg(test)]
thread_local! {
    pub static SERVED: std::cell::RefCell<BTreeMap<String, Value>> = const { std::cell::RefCell::new(BTreeMap::new()) };
}
#[cfg(test)]
fn listed_models(endpoint: &str) -> Option<Value> {
    SERVED.with(|served| served.borrow().get(endpoint).cloned())
}
#[cfg(test)]
pub fn serve(endpoint: &str, models: &[&str]) {
    SERVED.with(|served| served.borrow_mut().insert(endpoint.to_owned(), json!({"data":models.iter().map(|id| json!({"id":id})).collect::<Vec<_>>()})));
}
pub fn available(agent: &Agent) -> Option<bool> {
    let endpoint = agent.endpoint.as_deref().filter(|_| agent.provider == Provider::Local)?;
    Some(listed_models(endpoint).is_some_and(|models| served(&models, &agent.model)))
}
pub fn require_served(id: &str, agent: &Agent) -> Result<()> {
    require(
        available(agent) != Some(false),
        format!("Model {id} is not being served at {}; GET /api/agents shows which local models are available", agent.endpoint.as_deref().unwrap_or_default()),
    )
}
pub fn listing(defined: &BTreeMap<String, Agent>, family: Option<Family>) -> Value {
    let mut all = catalog();
    all.extend(defined.clone());
    json!({"family":family,"agents":all.into_iter().filter(|(_, a)| !a.retired).map(|(id, a)| json!({"id":id,"allowed":a.allowed(family),"agent":a})).collect::<Vec<_>>()})
}
#[cfg(test)]
#[path = "agents_tests.rs"]
mod tests;
