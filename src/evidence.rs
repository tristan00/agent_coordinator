use crate::engine::{Attempt, covers};
use crate::history::Artifact;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
pub fn clean_scratch(directory: &Path, attempt: &Attempt) {
    if attempt.evidence.configuration.as_ref().is_some_and(|c| c["scratch"] == true)
        && let Some(path) = safe_path(directory, "scratch")
        && let Err(error) = fs::remove_dir_all(&path)
    {
        let _ = crate::write(&directory.join("cleanup.json"), &json!({"status":"cleanup_debt","path":path,"error":error.to_string()}));
    }
}
fn is_tool_result(line: &str) -> bool {
    let Ok(event) = serde_json::from_str::<Value>(line) else { return false };
    let item = &event["item"];
    event["type"] == "item.completed"
        && match item["type"].as_str() {
            Some("command_execution") => item["exit_code"].is_i64() && item["aggregated_output"].is_string(),
            Some("mcp_tool_call") => item["result"].is_object() || item["error"].is_object(),
            _ => false,
        }
}
pub fn capture_trace(directory: &Path) -> crate::Result<()> {
    let raw = File::open(directory.join("events.jsonl"))?;
    let mut trace = File::create(directory.join("review-trace.jsonl"))?;
    let lines: Vec<String> = BufReader::new(raw).lines().collect::<io::Result<_>>()?;
    for line in &lines {
        if is_tool_result(&line) {
            writeln!(trace, "{line}")?;
        }
    }
    Ok(())
}

fn csv(value: &str) -> String {
    let value = value.replace('\r', "\\r").replace('\n', "\\n");
    if value.contains([',', '"']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

fn redact(value: String) -> String {
    let mut output = value;
    for marker in ["Bearer ", "HARNESS_RUN_TOKEN=", "HARNESS_API_TOKEN=", "HARNESS_JUDGMENT_TOKEN="] {
        while let Some(start) = output.find(marker) {
            let value_start = start + marker.len();
            let end = output[value_start..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                .map_or(output.len(), |offset| value_start + offset);
            output.replace_range(value_start..end, "[REDACTED]");
        }
    }
    output
}

fn event_record(event: &Value) -> Option<(&'static str, String, String, String)> {
    let item = &event["item"];
    if event["type"] == "item.completed" {
        match item["type"].as_str() {
            Some("command_execution") => {
                let command = item["command"].as_str()
                    .or_else(|| item["command"].as_array().and_then(|args| args.first()).and_then(Value::as_str))
                    .unwrap_or("command_execution");
                let status = item["exit_code"].to_string();
                return Some(("command", command.to_owned(), status, item.to_string()));
            }
            Some("mcp_tool_call") => {
                let operation = item["name"].as_str().or_else(|| item["tool_name"].as_str()).unwrap_or("mcp_tool_call");
                let status = if item["error"].is_object() { "error" } else { "completed" };
                let kind = if operation.to_ascii_lowercase().contains("file")
                    || operation.to_ascii_lowercase().contains("read")
                    || operation.to_ascii_lowercase().contains("write")
                {
                    "file"
                } else {
                    "api"
                };
                return Some((kind, operation.to_owned(), status.to_owned(), item.to_string()));
            }
            _ => {}
        }
    }
    if event["type"] == "assistant" {
        for block in event["message"]["content"].as_array().into_iter().flatten() {
            if block["type"] == "tool_use" {
                let operation = block["name"].as_str().unwrap_or("tool_use");
                let kind = if operation.to_ascii_lowercase().contains("file")
                    || operation.to_ascii_lowercase().contains("read")
                    || operation.to_ascii_lowercase().contains("write")
                {
                    "file"
                } else {
                    "tool"
                };
                return Some((kind, operation.to_owned(), "completed".into(), block.to_string()));
            }
        }
    }
    None
}

pub fn capture_agent_activity(directory: &Path, request: &Value) -> crate::Result<()> {
    let workspace = PathBuf::from(request["task"]["workspace"].as_str().ok_or("Missing task workspace")?);
    let task_id = request["task"]["id"].as_str().unwrap_or_default();
    let task = request["task"]["title"].as_str().unwrap_or_default();
    let run = request["run_id"].as_str().unwrap_or_default();
    let agent = request["agent"]["model"].as_str().or_else(|| request["agent"]["harness"].as_str()).unwrap_or_default();
    let events = fs::read_to_string(directory.join("events.jsonl"))?;
    let path = workspace.join("agent_activity.csv");
    let lock_path = workspace.join(".agent_activity.lock");
    let mut guard = None;
    for _ in 0..400 {
        if let Ok(file) = crate::lock(&lock_path) {
            guard = Some(file);
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let _guard = guard.ok_or("Could not acquire agent activity CSV lock")?;
    let new_file = !path.exists() || path.metadata()?.len() == 0;
    let mut output = OpenOptions::new().create(true).append(true).open(&path)?;
    if new_file {
        writeln!(output, "timestamp,task,task_id,run_id,agent,kind,operation,status,target,details")?;
    }
    for line in events.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else { continue };
        let Some((kind, operation, status, details)) = event_record(&event) else { continue };
        let timestamp = event["timestamp"].as_str().map(str::to_owned).unwrap_or_else(|| crate::now().to_string());
        let target = event["item"]["command"].as_str().unwrap_or_default();
        writeln!(output, "{},{},{},{},{},{},{},{},{},{}",
            csv(&timestamp), csv(task), csv(task_id), csv(run), csv(agent), csv(kind),
            csv(&operation), csv(&status), csv(target), csv(&redact(details)))?;
    }
    output.sync_data()?;
    Ok(())
}
pub fn digest(path: &Path) -> crate::Result<String> {
    let mut hash = Sha256::new();
    io::copy(&mut File::open(path)?, &mut hash)?;
    Ok(format!("sha256:{:x}", hash.finalize()))
}
pub fn safe_path(root: &Path, name: &str) -> Option<PathBuf> {
    let path = root.join(name);
    let resolved = path.canonicalize().ok()?;
    (covers(&path, &resolved) && covers(&resolved, &path)).then_some(path)
}
pub fn safe_file(root: &Path, name: &str) -> Option<PathBuf> {
    let path = safe_path(root, name)?;
    (path.is_file() && !crate::engine::linked_file(&path)).then_some(path)
}
pub fn artifact_name(kind: Artifact) -> Option<&'static str> {
    match kind {
        Artifact::None => None,
        Artifact::Candidate => Some("deliverables/candidate.exe"),
        Artifact::Report => Some("deliverables/report.md"),
    }
}
pub fn recover_artifact(directory: &Path, name: &str) {
    let target = directory.join(name);
    let stray = directory.join("scratch").join(name);
    if !target.exists() && stray.is_file() && !crate::engine::linked_file(&stray) && target.parent().is_some_and(|p| fs::create_dir_all(p).is_ok()) {
        let _ = fs::rename(&stray, &target).or_else(|_| fs::copy(&stray, &target).map(|_| ()));
    }
}
pub fn artifact(directory: &Path, kind: Artifact) -> Option<Value> {
    let path = safe_file(directory, artifact_name(kind)?)?;
    let digest = digest(&path).ok()?;
    Some(json!({"path":path,"digest":digest}))
}

#[cfg(test)]
#[path = "evidence_tests.rs"]
mod tests;
