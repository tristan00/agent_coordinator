use crate::client::{attempt_note, stage_label};
use crate::engine::{Engine, State, Task, lock_reason};
use crate::{Result, now, require};
use serde_json::{Value, json};
use std::collections::HashSet;

fn status_of(stage: &str, stopped: bool, blocked: bool, active: bool, attempted: bool) -> &'static str {
    match stage {
        "done" => "Done",
        _ if stopped && !active => "Stopped",
        "review" => "Review",
        _ if blocked && !active => "Blocked",
        _ if active || attempted || stage == "replan" => "In progress",
        _ => "To do",
    }
}

fn events(task: &Task, current: &str, port: u16) -> Vec<Value> {
    let mut events: Vec<Value> = Vec::new();
    let (mut stage, mut stopped, mut blocked, mut active, mut attempted) = ("implementation".to_owned(), false, false, false, false);
    let mut status = "To do";
    for e in &task.history {
        let event = |kind: &str, data: Value| json!({"at":e.at,"kind":kind,"actor":e.actor.clone().unwrap_or_else(|| "coordinator".into()),"data":data});
        match e.kind.as_str() {
            "created" => events.push(event("created", json!({"definition":e.evidence}))),
            "revised" | "updated" | "relationships_classified" => events.push(event("revised", json!({"fields":[],"after":e.evidence}))),
            "rewired" | "superseded" => events.push(event(&e.kind, e.evidence.clone())),
            "archived" | "abandoned" => events.push(event("archived", json!({}))),
            "merged" => {
                let text = format!("Merged {} into {} at {}", e.evidence["branch"].as_str().unwrap_or_default(), e.evidence["into"].as_str().unwrap_or_default(), e.evidence["commit"].as_str().unwrap_or_default());
                events.push(event("note", json!({"kind":"merged","text":text})));
            }
            _ => {}
        }
        match e.kind.as_str() {
            "dispatched" => (active, attempted) = (true, true),
            "phase_changed" => stage = e.evidence["to"].as_str().unwrap_or(&stage).to_owned(),
            "stopped" if e.evidence["scope"] == "task" => stopped = true,
            "resumed" if e.evidence["scope"] == "task" => (stopped, blocked) = (false, false),
            "held" => (active, blocked) = (false, true),
            "accepted" | "rejected" | "completed" | "blocked" | "replanned" | "attempt_stopped" | "attempt_failed" => active = false,
            "reopened" | "revised" => blocked = false,
            _ => {}
        }
        let next = status_of(&stage, stopped, blocked, active, attempted);
        if next != status {
            events.push(json!({"at":e.at,"kind":"status","actor":"coordinator","data":{"from":status,"to":next}}));
            status = next;
        }
    }
    if status != current {
        let at = task.history.last().map_or_else(now, |e| e.at);
        events.push(json!({"at":at,"kind":"status","actor":"coordinator","data":{"from":status,"to":current}}));
    }
    let mut counts = std::collections::HashMap::new();
    for attempt in &task.attempts {
        let number = counts.entry(attempt.stage.clone()).and_modify(|n| *n += 1).or_insert(1);
        events.push(json!({"at":attempt.started,"kind":"note","actor":"coordinator","data":{"kind":"started","text":format!("{} attempt {number} started", stage_label(&attempt.stage))}}));
        if let Some(ended) = attempt.ended {
            let link = format!("http://127.0.0.1:{port}/api/attempts/{}/log", attempt.id);
            events.push(json!({"at":ended,"kind":"note","actor":"coordinator","data":{"kind":"finished","text":attempt_note(task, attempt),"link":link}}));
            if attempt.status() == Some("replanned") && attempt.result.as_ref().is_some_and(|r| r["planError"].is_null()) {
                let text = format!("Replanned: {}", attempt.result.as_ref().and_then(|r| r["summary"].as_str()).unwrap_or_default());
                events.push(json!({"at":ended,"kind":"note","actor":"coordinator","data":{"kind":"replanned","text":text}}));
            }
        }
    }
    events.sort_by_key(|e| e["at"].as_u64().unwrap_or(0));
    events
}

fn record(state: &State, task: &Task, archived: bool, port: u16) -> Result<Value> {
    let status = if archived { status_of(&task.stage, task.stopped, task.blocked, false, !task.attempts.is_empty()) } else if state.halted(task) { "Stopped" } else { task.status() };
    let created = task.history.iter().find(|e| e.kind == "created").map(|e| e.at).or_else(|| task.id.split('-').next()?.parse().ok()).unwrap_or(0);
    let events = events(task, status, port);
    let updated = events.last().and_then(|e| e["at"].as_u64()).unwrap_or(created).max(created);
    let done = (status == "Done").then(|| events.iter().rev().find(|e| e["kind"] == "status" && e["data"]["to"] == "Done").and_then(|e| e["at"].as_u64()).unwrap_or(updated));
    let lock = task.active.as_ref().map(|_| json!({"client":"coordinator","reason":lock_reason(&task.stage),"since":task.attempts.last().map_or(updated, |a| a.started)}));
    let mut attributes = serde_json::to_value(&task.attributes)?;
    attributes["priority"] = json!(task.attributes.priority());
    attributes["model"] = json!(task.attributes.model);
    attributes["effort"] = json!(task.attributes.effort);
    let revisions = 1 + task.history.iter().filter(|e| matches!(e.kind.as_str(), "revised" | "updated" | "rewired" | "relationships_classified")).count();
    let workspace = home(state, task)?;
    Ok(json!({"id":task.id,"shortId":task.short_id.clone().unwrap_or_default(),"workspace":workspace,"key":task.key,
        "title":task.title,"outcome":task.outcome,"acceptance":task.acceptance,"cwd":task.cwd,"attributes":attributes,"parent":task.parent,"standalone":task.standalone,
        "dependencies":task.dependencies,"after":task.after,"conflicts":task.conflicts,"related":task.related,"status":status,"revision":revisions,
        "createdAt":created,"updatedAt":updated,"doneAt":done,"lock":lock,"archived":archived,"events":events}))
}

fn home(state: &State, task: &Task) -> Result<std::path::PathBuf> {
    match &task.workspace {
        Some(path) => Ok(path.clone()),
        None => Ok(state.owning_workspace(crate::engine::workspace(std::path::Path::new(&task.cwd))?)),
    }
}

pub fn payload(state: &State, port: u16) -> Result<Value> {
    let mut paths: std::collections::BTreeSet<std::path::PathBuf> = state.workspace_ids.keys().cloned().chain(state.config.workspaces.keys().cloned()).collect();
    for task in state.tasks.iter().chain(&state.archived) {
        paths.insert(home(state, task)?);
    }
    let workspaces: Vec<Value> = paths
        .iter()
        .map(|path| {
            let id = state.workspace_ids.get(path);
            json!({"path":path,"name":id.map(|w| &w.abbreviation),"next":id.map_or(0, |w| w.next),"schema":state.schema(path)})
        })
        .collect();
    let mut tasks = Vec::new();
    for task in &state.tasks {
        tasks.push(record(state, task, false, port)?);
    }
    for task in &state.archived {
        tasks.push(record(state, task, true, port)?);
    }
    Ok(json!({"workspaces":workspaces,"tasks":tasks}))
}

pub fn run(engine: &mut Engine) -> Result<()> {
    let archive = engine.directory.join("state.before-agent-tasks.json");
    if !archive.exists() && engine.directory.join("state.json").exists() {
        std::fs::copy(engine.directory.join("state.json"), &archive)?;
    }
    let payload = payload(&engine.state, engine.port)?;
    let ids: HashSet<String> = payload["tasks"].as_array().into_iter().flatten().filter_map(|t| t["id"].as_str().map(str::to_owned)).collect();
    let existing = engine.service.tasks()?;
    let present: HashSet<String> = existing["tasks"].as_array().into_iter().flatten().filter_map(|t| t["id"].as_str().map(str::to_owned)).collect();
    let sequence = if !ids.is_empty() && ids.is_subset(&present) {
        existing["sequence"].as_u64().unwrap_or(0)
    } else {
        require(ids.is_disjoint(&present), "The task service already holds some of these tasks but not all; wipe it and restart")?;
        let names: HashSet<String> = engine.service.call("GET", "/api/workspaces", None, "coordinator")?.as_array().into_iter().flatten().filter_map(|w| w["path"].as_str().map(crate::engine::path_key)).collect();
        let mut payload = payload;
        payload["workspaces"].as_array_mut().unwrap().retain(|w| !names.contains(&crate::engine::path_key(w["path"].as_str().unwrap_or_default())));
        engine.service.call("POST", "/api/import", Some(&payload), "coordinator")?["sequence"].as_u64().unwrap_or(0)
    };
    let after = engine.service.tasks()?;
    let imported: HashSet<String> = after["tasks"].as_array().into_iter().flatten().filter_map(|t| t["id"].as_str().map(str::to_owned)).collect();
    require(ids.is_subset(&imported), "The task service is missing imported tasks")?;
    engine.change(|state| {
        state.tasks_migrated = true;
        state.cursor = sequence;
        Ok(Value::Null)
    })
    .map(drop)
}
