use crate::engine::{Attempt, Engine, Task, lock_reason};
use crate::task_service::unavailable;
use crate::{Result, now};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DEFINITION_EVENTS: [&str; 6] = ["created", "revised", "rewired", "archived", "superseded", "unlinked"];

pub fn stage_label(stage: &str) -> &'static str {
    match stage {
        "review" => "Review",
        "replan" => "Replan",
        _ => "Implementation",
    }
}

fn compact(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

fn minutes(ms: u64) -> String {
    let seconds = ms / 1000;
    if seconds < 60 { format!("{seconds}s") } else if seconds < 3600 { format!("{}m", seconds / 60) } else { format!("{}h {}m", seconds / 3600, seconds % 3600 / 60) }
}

pub fn attempt_note(task: &Task, attempt: &Attempt) -> String {
    let stage = stage_label(&attempt.stage);
    let mut parts = vec![match (&attempt.error, attempt.result.as_ref()) {
        (Some(error), _) => format!("{stage} failed: {error}"),
        (None, Some(result)) => format!("{stage} {}: {}", result["status"].as_str().unwrap_or("finished"), result["summary"].as_str().unwrap_or_default()),
        _ => format!("{stage} ended"),
    }];
    let failed: Vec<&str> =
        attempt.result.as_ref().and_then(|r| r["criteria"].as_array()).map(|outcomes| outcomes.iter().zip(&task.acceptance).filter(|(o, _)| *o == "failed").map(|(_, c)| c.as_str()).collect()).unwrap_or_default();
    if !failed.is_empty() {
        parts.push(format!("Failed: {}", failed.join("; ")));
    }
    if let Some(ended) = attempt.ended {
        parts.push(minutes(ended.saturating_sub(attempt.started)));
    }
    if let Some(tokens) = attempt.evidence.usage.as_ref().and_then(|u| u.tokens()) {
        parts.push(format!("{} tokens", compact(tokens)));
    }
    if let Some(usd) = attempt.evidence.cost.as_ref().and_then(|c| c["usd"].as_f64()) {
        parts.push(format!("${usd:.2}"));
    }
    if let Some(model) = attempt.evidence.configuration.as_ref().and_then(|c| c["effective"]["model"].as_str()) {
        parts.push(model.to_owned());
    }
    parts.join(" · ")
}

impl Engine {
    pub fn connect(&mut self) -> Result<()> {
        if !self.state.tasks_migrated {
            crate::migrate::run(self)?;
        }
        self.register_workspaces()?;
        let listing = self.service.tasks()?;
        let remote: Vec<Value> = listing["tasks"].as_array().cloned().unwrap_or_default();
        let known: HashSet<&str> = remote.iter().filter_map(|t| t["id"].as_str()).collect();
        self.pushed = remote.iter().filter(|t| t["archived"] != true).filter_map(|t| Some((t["id"].as_str()?.to_owned(), t["status"].as_str()?.to_owned()))).collect();
        let missing: Vec<String> = self.state.tasks.iter().filter(|t| !known.contains(t.id.as_str())).map(|t| t.id.clone()).collect();
        self.change(|state| {
            for definition in &remote {
                state.adopt(definition, "sync", None)?;
            }
            for id in &missing {
                let index = state.index(id)?;
                let mut task = state.tasks.remove(index);
                task.record("abandoned", "sync", json!({"reason":"The task service no longer has this task"}));
                state.archived.push(task);
            }
            Ok(Value::Null)
        })?;
        for task in self.state.tasks.iter().filter(|t| t.active.is_some()) {
            self.service.lock(&task.id, lock_reason(&task.stage))?;
        }
        self.sync()
    }
    pub fn register_workspaces(&mut self) -> Result<()> {
        let paths: BTreeSet<PathBuf> = self.state.config.workspaces.keys().cloned().chain(self.state.tasks.iter().filter_map(|t| t.workspace.clone())).collect();
        for path in paths {
            self.register_workspace(&path)?;
        }
        Ok(())
    }
    pub fn register_workspace(&mut self, path: &Path) -> Result<()> {
        let schema = self.state.schema(path);
        self.service.call("PUT", "/api/workspaces", Some(&json!({"path":path,"schema":schema})), "coordinator")?;
        self.registered.insert(path.to_path_buf());
        Ok(())
    }
    pub fn sync(&mut self) -> Result<()> {
        let feed = self.service.changes(self.state.cursor)?;
        let sequence = feed["sequence"].as_u64().unwrap_or(self.state.cursor);
        if sequence == self.state.cursor {
            return Ok(());
        }
        let mut causes: Vec<(String, String, Option<String>, Value)> = Vec::new();
        let mut commands = Vec::new();
        for event in feed["events"].as_array().into_iter().flatten() {
            let kind = event["kind"].as_str().unwrap_or_default();
            if let Some(task) = event["task"].as_str().filter(|_| DEFINITION_EVENTS.contains(&kind)) {
                let actor = event["actor"].as_str().map(str::to_owned);
                let position = causes.iter().position(|(id, _, _, _)| id == task).unwrap_or_else(|| {
                    causes.push((task.to_owned(), "rewired".into(), None, Value::Null));
                    causes.len() - 1
                });
                let entry = &mut causes[position];
                match kind {
                    "superseded" => (entry.1, entry.3) = ("superseded".into(), event["data"].clone()),
                    "rewired" => {}
                    "created" if entry.1 != "superseded" => (entry.1, entry.3) = ("feed".into(), event["data"].clone()),
                    _ if entry.1 == "rewired" => entry.1 = "feed".into(),
                    _ => {}
                }
                entry.2 = actor;
            }
            if kind == "command" && event["data"]["client"] == "coordinator" && event["data"]["state"] == "pending" {
                commands.push(event["data"].clone());
            }
        }
        let mut definitions = Vec::new();
        for (id, cause, actor, evidence) in causes {
            if let Some(definition) = self.service.task(&id)? {
                definitions.push((definition, cause, actor, evidence));
            }
        }
        self.change(|state| {
            for (definition, cause, actor, evidence) in &definitions {
                state.adopt_with(definition, cause, actor.as_deref(), evidence)?;
            }
            state.cursor = sequence;
            Ok(Value::Null)
        })?;
        for command in commands {
            self.execute(&command)?;
        }
        Ok(())
    }
    fn execute(&mut self, command: &Value) -> Result<()> {
        let task = command["task"].as_str();
        let previous = self.actor.replace(command["actor"].as_str().unwrap_or("ui").to_owned());
        let outcome = match (command["command"].as_str().unwrap_or_default(), task) {
            ("stop", Some(id)) => self.control(true, Some(id)).map(|()| "Stopped"),
            ("resume", Some(id)) => self.control(false, Some(id)).map(|()| "Resumed"),
            ("replan", Some(id)) => self.request_replan(id, json!({"reason":command["input"]})).map(|_| "Queued for replanning"),
            ("pause", None) => self.control(true, None).map(|()| "Paused all work"),
            ("resume-all", None) => self.control(false, None).map(|()| "Resumed all work"),
            (other, _) => Err(format!("Unknown command {other}").into()),
        };
        self.actor = previous;
        let (ok, message) = match outcome {
            Ok(message) => (true, message.to_owned()),
            Err(error) => (false, error.to_string()),
        };
        let id = command["id"].clone();
        self.change(|state| {
            state.outbox.push(json!({"op":"result","id":id,"ok":ok,"message":message}));
            Ok(Value::Null)
        })
        .map(drop)
    }
    pub fn flush(&mut self) -> Result<()> {
        let mut delivered = 0;
        let mut failure = None;
        for operation in &self.state.outbox {
            match self.service.deliver(operation) {
                Ok(_) => delivered += 1,
                Err(error) if unavailable(error.as_ref()) => {
                    failure = Some(error);
                    break;
                }
                Err(_) => delivered += 1,
            }
        }
        if delivered > 0 {
            let mut state = self.state.clone();
            state.outbox.drain(..delivered);
            self.commit(state)?;
        }
        failure.map_or(Ok(()), Err)
    }
    pub fn publish(&mut self) -> Result<()> {
        let current: HashMap<String, String> = self.state.tasks.iter().map(|t| (t.id.clone(), if self.state.halted(t) { "Stopped" } else { t.status() }.to_owned())).collect();
        let changed: Vec<Value> = current.iter().filter(|(id, status)| self.pushed.get(*id) != Some(*status)).map(|(id, status)| json!({"id":id,"status":status})).collect();
        if changed.is_empty() {
            return Ok(());
        }
        self.service.call("PUT", "/api/statuses", Some(&json!(changed)), "coordinator")?;
        self.pushed = current;
        Ok(())
    }
    pub fn announce(&mut self, error: &str) -> Result<()> {
        let at = now();
        let (text, tone) = if self.state.paused {
            ("Paused".to_owned(), "warn")
        } else if self.state.hold_until > at {
            (format!("Usage hold, resumes in {} min", (self.state.hold_until - at).div_ceil(60_000)), "warn")
        } else if !error.is_empty() {
            (format!("Scheduler error: {error}"), "bad")
        } else {
            ("Running".to_owned(), "good")
        };
        let board = if self.state.paused { json!([{"id":"resume-all","label":"Resume all work"}]) } else { json!([{"id":"pause","label":"Pause all work","confirm":"Pause all work? Running workers are stopped."}]) };
        let open = ["To do", "In progress", "Review", "Blocked"];
        let registration = json!({"label":"Coordinator","url":format!("http://127.0.0.1:{}", self.port),"state":{"text":text,"tone":tone},
            "taskCommands":[{"id":"stop","label":"Stop","statuses":open},{"id":"resume","label":"Resume","statuses":["Stopped","Blocked"]},
                {"id":"replan","label":"Replan","statuses":["To do","In progress","Blocked","Stopped"],"containers":false,"input":"Why should this task be replanned?"}],
            "boardCommands":board});
        let due = self.announced.as_ref().is_none_or(|(sent, when)| sent != &registration || when.elapsed() >= Duration::from_secs(5));
        if due {
            self.service.call("PUT", "/api/clients/coordinator", Some(&registration), "coordinator")?;
            self.announced = Some((registration, Instant::now()));
        }
        Ok(())
    }
}
