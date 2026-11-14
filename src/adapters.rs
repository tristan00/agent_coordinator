use crate::agents::Agent;
use crate::history::{Observed, Usage};
use crate::process::{command, prompt, schema};
use crate::{Result, require, write};
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

fn attempt(request: &Value) -> Result<(PathBuf, PathBuf, String)> {
    let result = PathBuf::from(std::env::var("COORDINATOR_RESULT")?);
    let directory = result.parent().ok_or("Missing attempt directory")?.to_path_buf();
    Ok((result, directory, request["stage"].as_str().unwrap_or("implementation").to_owned()))
}
fn working_directory(request: &Value) -> Result<PathBuf> {
    let read_only = request["readOnly"] == true;
    let cwd = if read_only { request["scratch"].as_str() } else { request["task"]["cwd"].as_str() };
    let cwd = PathBuf::from(cwd.ok_or("Missing working directory")?);
    std::fs::create_dir_all(&cwd)?;
    Ok(cwd)
}
fn usage_components(source: &str, input: u64, cached: u64, written: u64, output: u64, turns: u64) -> Value {
    let field = |value: u64| json!({"value":value,"measuredTurns":turns,"turns":turns});
    json!({"source":source,"fields":{"input_tokens":field(input),"cached_input_tokens":field(cached),"cache_write_input_tokens":field(written),"output_tokens":field(output),"reasoning_output_tokens":field(0)}})
}
pub fn hosted_claude_environment(name: &str) -> bool {
    name == "CLAUDECODE" || name == "CLAUDE_PID" || name == "CLAUDE_EFFORT" || name.starts_with("CLAUDE_CODE_") || name.starts_with("CLAUDE_AGENT_SDK") || name.starts_with("CLAUDE_PREVIEW")
}
pub fn claude_result(events: &str) -> Result<(Value, Value, Option<Value>)> {
    let lines: Vec<Value> = events.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
    let init = lines.iter().find(|e| e["type"] == "system" && e["subtype"] == "init").cloned();
    let done = lines.iter().rev().find(|e| e["type"] == "result").ok_or("Claude produced no result event")?;
    require(done["is_error"] != true, format!("Claude worker failed: {}", done["result"]))?;
    let output = match &done["structured_output"] {
        Value::Object(_) => done["structured_output"].clone(),
        _ => json_object(done["result"].as_str().unwrap_or_default())?,
    };
    let u = &done["usage"];
    let n = |key: &str| u[key].as_u64().unwrap_or(0);
    let (cached, written) = (n("cache_read_input_tokens"), n("cache_creation_input_tokens"));
    let components = usage_components("claude_stream_json:result", n("input_tokens") + cached + written, cached, written, n("output_tokens"), done["num_turns"].as_u64().unwrap_or(1));
    Ok((output, components, init))
}
fn json_object(text: &str) -> Result<Value> {
    let start = text.find('{').ok_or("Reply contains no JSON object")?;
    let end = text.rfind('}').ok_or("Reply contains no JSON object")?;
    require(end > start, "Reply contains no JSON object")?;
    Ok(serde_json::from_str(&text[start..=end])?)
}
pub fn executable(name: &str, path: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let found = std::env::split_paths(&path?).flat_map(|dir| ["exe", "cmd"].map(|ext| dir.join(format!("{name}.{ext}")))).find(|candidate| candidate.is_file())?;
    let native = found.parent()?.join("node_modules/@anthropic-ai/claude-code/bin").join(format!("{name}.exe"));
    Some(if found.extension().is_some_and(|e| e == "cmd") && native.is_file() { native } else { found })
}
pub fn claude(request: &Value, agent: &Agent) -> Result<()> {
    let (result, directory, stage) = attempt(request)?;
    let program = std::env::var("CLAUDE_BIN").ok().or_else(|| executable("claude", std::env::var_os("PATH")).map(|p| p.to_string_lossy().into_owned())).ok_or("Claude Code executable not found on PATH")?;
    let mut cli = command(&program);
    for (name, _) in std::env::vars_os() {
        if hosted_claude_environment(&name.to_string_lossy()) {
            cli.env_remove(&name);
        }
    }
    cli.current_dir(working_directory(request)?);
    cli.args(["-p", "--model", &agent.model, "--output-format", "stream-json", "--verbose", "--no-session-persistence", "--dangerously-skip-permissions", "--json-schema"]);
    cli.arg(schema(&stage).to_string());
    if let Some(effort) = &agent.effort {
        cli.args(["--effort", effort]);
    }
    if request["readOnly"] == true {
        cli.args(["--disallowedTools", "Edit", "Write", "NotebookEdit"]);
    }
    let events = directory.join("events.jsonl");
    let mut child = cli.stdin(Stdio::piped()).stdout(std::fs::File::create(&events)?).stderr(Stdio::inherit()).spawn()?;
    let input = child.stdin.take().unwrap().write_all(format!("{}\nReturn only the JSON object required by the output schema.\n{request}", prompt(request, &stage)?).as_bytes());
    let status = child.wait()?;
    input?;
    let (output, components, init) = claude_result(&std::fs::read_to_string(&events)?)?;
    let tokens = crate::telemetry::total(&components);
    write(&directory.join("usage.json"), &Usage { total_tokens: tokens, components: Some(components), source: "claude_stream_json".into() })?;
    let observed = Observed {
        worker: Path::new(&program).file_name().unwrap_or_default().to_string_lossy().into(),
        cli_version: init.as_ref().and_then(|i| i["claude_code_version"].as_str().map(str::to_owned)),
        model: init.as_ref().and_then(|i| i["model"].as_str().map(str::to_owned)),
        effort: None,
        forwarded_model: Some(agent.model.clone()),
        forwarded_effort: agent.effort.clone(),
        source: "claude_stream_json".into(),
    };
    write(&directory.join("launch-settings.json"), &observed)?;
    crate::evidence::capture_trace(&directory)?;
    if let Err(error) = crate::evidence::capture_agent_activity(&directory, request) {
        eprintln!("Could not record agent activity: {error}");
    }
    require(status.success(), "Claude worker failed; inspect attempt log")?;
    write(&result, &output)
}
#[cfg(test)]
#[path = "adapters_tests.rs"]
mod tests;
