use crate::Result;
use serde_json::{Value, json};
use std::time::Duration;

pub const UNAVAILABLE: &str = "Task service unavailable";

#[derive(Clone)]
pub struct Service {
    pub url: String,
    agent: ureq::Agent,
}

fn encode(text: &str) -> String {
    text.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

impl Service {
    pub fn new(url: &str) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).http_status_as_error(false).build().into();
        Self { url: url.trim_end_matches('/').to_owned(), agent }
    }
    pub fn call(&self, method: &str, path: &str, body: Option<&Value>, actor: &str) -> Result<Value> {
        let url = format!("{}{path}", self.url);
        let response = match method {
            "GET" => self.agent.get(&url).header("X-Client", "coordinator").header("X-Actor", actor).call(),
            "DELETE" => self.agent.delete(&url).header("X-Client", "coordinator").header("X-Actor", actor).call(),
            _ => {
                let request = match method {
                    "POST" => self.agent.post(&url),
                    "PUT" => self.agent.put(&url),
                    _ => self.agent.patch(&url),
                };
                request.header("X-Client", "coordinator").header("X-Actor", actor).send_json(body.cloned().unwrap_or(json!({})))
            }
        };
        let mut response = response.map_err(|e| format!("{UNAVAILABLE} at {}: {e}", self.url))?;
        let status = response.status().as_u16();
        let value: Value = response.body_mut().read_json().unwrap_or(Value::Null);
        if status >= 400 {
            return Err(value["error"].as_str().map_or_else(|| format!("Task service answered {status}"), str::to_owned).into());
        }
        Ok(value)
    }
    pub fn task(&self, id: &str) -> Result<Option<Value>> {
        match self.call("GET", &format!("/api/tasks/{}", encode(id)), None, "coordinator") {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.to_string().starts_with("Task not found") || error.to_string().starts_with("Not found") => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub fn tasks(&self) -> Result<Value> {
        self.call("GET", "/api/tasks?archived=true", None, "coordinator")
    }
    pub fn changes(&self, after: u64) -> Result<Value> {
        self.call("GET", &format!("/api/changes?after={after}"), None, "coordinator")
    }
    pub fn lock(&self, id: &str, reason: &str) -> Result<Value> {
        self.call("PUT", &format!("/api/tasks/{}/lock", encode(id)), Some(&json!({"reason":reason})), "coordinator")
    }
    pub fn unlock(&self, id: &str) -> Result<Value> {
        self.call("DELETE", &format!("/api/tasks/{}/lock", encode(id)), None, "coordinator")
    }
    pub fn deliver(&self, operation: &Value) -> Result<Value> {
        let task = operation["task"].as_str().map(encode).unwrap_or_default();
        match operation["op"].as_str().unwrap_or_default() {
            "note" => self.call("POST", &format!("/api/tasks/{task}/notes"), Some(&json!({"kind":operation["kind"],"text":operation["text"],"link":operation["link"]})), "coordinator"),
            "unlock" => self.unlock(operation["task"].as_str().unwrap_or_default()),
            "result" => self.call("POST", &format!("/api/commands/{}/result", encode(operation["id"].as_str().unwrap_or_default())), Some(&json!({"ok":operation["ok"],"message":operation["message"]})), "coordinator"),
            other => Err(format!("Unknown outbox operation {other}").into()),
        }
    }
    pub fn encode(text: &str) -> String {
        encode(text)
    }
}

pub fn unavailable(error: &(dyn std::error::Error + Send + Sync)) -> bool {
    error.to_string().starts_with(UNAVAILABLE)
}
