use crate::agents::Agent;
use crate::history::{Observed, Usage};
use crate::process::{command, prompt, schema};
use crate::{Result, require, write};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

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
    require(status.success(), "Claude worker failed; inspect attempt log")?;
    write(&result, &output)
}
const TOOLS: &str = "You are a coding agent. Reply with exactly one JSON object per turn. To use a tool reply {\"tool\":NAME,\"args\":{...}}. Tools: read_file {path}; list_files {path}; write_file {path, content}; run_command {argv: [program, args...], timeout_seconds}. Relative paths resolve against the working directory. When finished reply {\"final\": RESULT} where RESULT matches the output schema.";
fn inside(path: &Path, roots: &[&Path]) -> bool {
    let parent = path.parent().and_then(|p| p.canonicalize().ok());
    parent.is_some_and(|p| roots.iter().filter_map(|r| r.canonicalize().ok()).any(|r| p.starts_with(r)))
}
fn clip(text: String) -> String {
    if text.len() <= 12000 { text } else { format!("{}\n[truncated {} bytes]", &text[..text.floor_char_boundary(12000)], text.len() - 12000) }
}
pub fn run_tool(call: &Value, cwd: &Path, writable: &[&Path]) -> String {
    let path = |args: &Value| cwd.join(args["path"].as_str().unwrap_or("."));
    let args = &call["args"];
    let outcome: Result<String> = match call["tool"].as_str().unwrap_or_default() {
        "read_file" => std::fs::read_to_string(path(args)).map_err(Into::into),
        "list_files" => std::fs::read_dir(path(args)).map_err(Into::into).map(|entries| {
            entries.filter_map(std::result::Result::ok).map(|e| format!("{}{}", e.file_name().to_string_lossy(), if e.path().is_dir() { "/" } else { "" })).collect::<Vec<_>>().join("\n")
        }),
        "write_file" => {
            let target = path(args);
            if let Some(parent) = target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if !inside(&target, writable) {
                Err("Path is outside the writable directories".into())
            } else {
                std::fs::write(&target, args["content"].as_str().unwrap_or_default()).map(|_| "written".into()).map_err(Into::into)
            }
        }
        "run_command" => run(args, cwd),
        other => Err(format!("Unknown tool {other}").into()),
    };
    clip(outcome.unwrap_or_else(|error| format!("error: {error}")))
}
fn run(args: &Value, cwd: &Path) -> Result<String> {
    let argv: Vec<String> = serde_json::from_value(args["argv"].clone())?;
    require(!argv.is_empty(), "argv is required")?;
    let limit = Duration::from_secs(args["timeout_seconds"].as_u64().unwrap_or(120));
    let output = std::env::temp_dir().join(format!("coordinator-tool-{}.log", crate::engine::id()));
    let log = std::fs::File::create(&output)?;
    let mut child = command(&argv[0]).args(&argv[1..]).current_dir(cwd).stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log).spawn()?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status.code().map_or("killed".into(), |c| c.to_string());
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            break "timeout".into();
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let text = std::fs::read_to_string(&output).unwrap_or_default();
    let _ = std::fs::remove_file(&output);
    Ok(format!("exit {status}\n{text}"))
}
fn chat(url: &str, body: &Value) -> Result<Value> {
    let mut curl = command("curl.exe");
    curl.args(["-q", "-sS", "--max-time", "900", "--noproxy", "*", "-H", "Content-Type: application/json", "--data-binary", "@-", url]);
    let mut child = curl.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    child.stdin.take().unwrap().write_all(body.to_string().as_bytes())?;
    let mut stdout = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut stdout)?;
    let mut stderr = String::new();
    BufReader::new(child.stderr.take().unwrap()).read_line(&mut stderr)?;
    require(child.wait()?.success(), format!("Local model request failed: {stderr}"))?;
    let value: Value = serde_json::from_slice(&stdout)?;
    require(value.get("error").is_none(), format!("Local model error: {}", value["error"]))?;
    Ok(value)
}
pub fn local(request: &Value, agent: &Agent) -> Result<()> {
    let (result, directory, stage) = attempt(request)?;
    let endpoint = agent.endpoint.as_deref().filter(|e| crate::agents::local_endpoint(e)).ok_or("Local agent needs a loopback endpoint")?;
    let url = format!("{}/chat/completions", endpoint.trim_end_matches('/'));
    let cwd = PathBuf::from(request["task"]["cwd"].as_str().ok_or("Missing cwd")?);
    let scratch = working_directory(&json!({"readOnly":true,"scratch":request["scratch"]}))?;
    let writable: Vec<&Path> = if request["readOnly"] == true { vec![&scratch] } else { vec![&cwd, &scratch] };
    let mut messages = vec![
        json!({"role":"system","content":TOOLS}),
        json!({"role":"user","content":format!("{}\nWorking directory: {}\nOutput schema: {}\n{request}", prompt(request, &stage)?, cwd.display(), schema(&stage))}),
    ];
    let transcript = directory.join("events.jsonl");
    let steps = std::env::var("COORDINATOR_LOCAL_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(60u64);
    let (mut input, mut output, mut turns) = (0, 0, 0);
    let finish = |input, output, turns| {
        let components = usage_components("openai_compatible:usage", input, 0, 0, output, turns);
        write(&directory.join("usage.json"), &Usage { total_tokens: crate::telemetry::total(&components), components: Some(components), source: "openai_compatible".into() })
    };
    for _ in 0..steps {
        let reply = chat(&url, &json!({"model":agent.model,"messages":messages,"temperature":0.2,"response_format":{"type":"json_object"}}))?;
        turns += 1;
        input += reply["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
        output += reply["usage"]["completion_tokens"].as_u64().unwrap_or(0);
        let content = reply["choices"][0]["message"]["content"].as_str().unwrap_or_default().to_owned();
        std::fs::OpenOptions::new().create(true).append(true).open(&transcript)?.write_all(format!("{}\n", json!({"type":"turn","content":content,"usage":reply["usage"]})).as_bytes())?;
        messages.push(json!({"role":"assistant","content":content}));
        let call = json_object(&content).unwrap_or_else(|_| json!({"tool":"invalid"}));
        if call["final"].is_object() {
            finish(input, output, turns)?;
            write(&directory.join("launch-settings.json"), &json!({"worker":"openai_compatible","model":agent.model,"endpoint":endpoint,"source":"openai_compatible"}))?;
            return write(&result, &call["final"]);
        }
        let observation = if call["tool"] == "invalid" { "error: reply with one JSON object".into() } else { run_tool(&call, &cwd, &writable) };
        messages.push(json!({"role":"user","content":format!("Tool result:\n{observation}")}));
    }
    finish(input, output, turns)?;
    Err(format!("Local agent did not finish within {steps} turns").into())
}
#[cfg(test)]
#[path = "adapters_tests.rs"]
mod tests;
