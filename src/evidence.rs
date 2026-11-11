use crate::engine::{Attempt, covers};
use crate::history::Artifact;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
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
    for line in BufReader::new(raw).lines() {
        let line = line?;
        if is_tool_result(&line) {
            writeln!(trace, "{line}")?;
        }
    }
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
pub fn artifact(directory: &Path, kind: Artifact) -> Option<Value> {
    let path = safe_file(directory, artifact_name(kind)?)?;
    let digest = digest(&path).ok()?;
    Some(json!({"path":path,"digest":digest}))
}
