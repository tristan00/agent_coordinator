use crate::agents::{Agent, Harness};
use crate::engine::{Engine, Task, id, workspace};
use crate::process::{Job, command};
use crate::{Result, lock, now, read, require, write};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

const SKIPPED: [&str; 8] = [".git", "target", "node_modules", ".venv", "venv", "__pycache__", ".harness", ".env"];

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub workspace: PathBuf,
    pub output: PathBuf,
    pub deliverable: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback: Option<String>,
    pub status: String,
    pub requests: Vec<Request>,
    #[serde(default)]
    pub deliveries: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outbox: Vec<Value>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub retry_at: u64,
    pub created: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Request {
    pub text: String,
    pub run: PathBuf,
    pub started: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_ended: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn session<'a>(engine: &'a Engine, id: &str) -> Result<&'a Session> {
    engine.state.sessions.get(id).ok_or_else(|| format!("Unknown session {id}").into())
}

fn relative(text: &str) -> Result<PathBuf> {
    let path = PathBuf::from(if text.is_empty() { "." } else { text });
    require(!path.is_absolute() && path.components().all(|part| matches!(part, Component::Normal(_) | Component::CurDir)), "Deliverable must be a folder inside the workspace, without '..'")?;
    Ok(path)
}

fn local_url(url: &str) -> bool {
    url.strip_prefix("http://127.0.0.1:").or_else(|| url.strip_prefix("http://localhost:")).is_some_and(|rest| rest.split('/').next().is_some_and(|port| port.parse::<u16>().is_ok()))
}

pub fn can_still_run(task: &Task) -> bool {
    !task.container() && (task.active.is_some() || !(task.stage == "done" || task.stopped || task.blocked))
}

pub fn create(engine: &mut Engine, input: Value) -> Result<Value> {
    let source = workspace(Path::new(input["workspace"].as_str().ok_or("workspace is required")?))?;
    let settings = engine.state.config.workspaces.get(&source).cloned().ok_or_else(|| format!("{} is not a configured workspace", source.display()))?;
    let text = input["request"].as_str().filter(|text| !text.trim().is_empty()).ok_or("request text is required")?.to_owned();
    let output = PathBuf::from(input["output"].as_str().ok_or("output folder is required")?);
    require(output.is_absolute(), "output must be an absolute folder")?;
    let deliverable = relative(input["deliverable"].as_str().unwrap_or("."))?;
    let callback = input["callback"].as_str().map(str::to_owned);
    require(callback.as_deref().is_none_or(local_url), "callback must be an http URL on 127.0.0.1 or localhost")?;
    settings.agent("planner", engine.state.family)?;
    let directory = match input["directory"].as_str() {
        None => source.clone(),
        Some(path) => {
            let path = PathBuf::from(path);
            require(path.is_absolute(), "directory must be absolute")?;
            require(!path.exists() || fs::read_dir(&path)?.next().is_none(), "directory must be new or empty")?;
            fs::create_dir_all(&path)?;
            fs::write(path.join(crate::workspace_prompts::LIMITS_FILE), serde_json::to_vec_pretty(&settings.limits)?)?;
            workspace(&path)?
        }
    };
    require(!engine.state.sessions.values().any(|s| s.workspace == directory && s.status == "running"), "A session is already running in this workspace")?;
    fs::create_dir_all(&output)?;
    let id = id();
    let session = Session { id: id.clone(), workspace: directory.clone(), output, deliverable: deliverable.to_string_lossy().replace('\\', "/"), callback, status: "running".into(), requests: Vec::new(), deliveries: 0, conversation: None, outbox: Vec::new(), retry_at: 0, created: now() };
    engine.change(|state| {
        state.config.workspaces.entry(directory.clone()).or_insert(settings);
        state.sessions.insert(id.clone(), session);
        Ok(Value::Null)
    })?;
    start(engine, &id, text)?;
    view(engine, &id)
}

pub fn message(engine: &mut Engine, id: &str, input: Value) -> Result<Value> {
    let text = input["request"].as_str().filter(|text| !text.trim().is_empty()).ok_or("request text is required")?.to_owned();
    require(session(engine, id)?.status == "waiting", "The session is still working on its current request")?;
    start(engine, id, text)?;
    view(engine, id)
}

fn prompt(session: &Session, text: &str, first: bool, instructions: &str) -> String {
    if !first {
        return format!("New request from the same user. The workspace, deliverable folder and delivery rules are unchanged.\n\n{text}");
    }
    let w = session.workspace.to_string_lossy().trim_start_matches(r"\\?\").to_owned();
    format!(
        "You are the planner of the agent coordinator for the workspace {w}.\n\
         Plan the request as coordinator tasks; workers implement and review them. The coordinator is at $COORDINATOR_URL and your workspace is $COORDINATOR_WORKSPACE (W). GET $COORDINATOR_URL/api/task-schema?workspace=W lists the task fields and limits, GET $COORDINATOR_URL/api/tasks?workspace=W lists your tasks, and POST $COORDINATOR_URL/api/tasks creates one.\n\
         The deliverable is the folder {} inside the workspace. Whenever it holds a complete version, POST an empty JSON object to $COORDINATOR_URL/api/sessions/$COORDINATOR_SESSION/deliver; each delivery replaces the previous one. When you end your turn the request finishes once no task can still run, and a final delivery is made then.\n\
         No one is available to answer questions during the work; make decisions yourself.\n\nPlanner instructions:\n{instructions}\n\nRequest:\n{text}",
        session.deliverable
    )
}

fn start(engine: &mut Engine, id: &str, text: String) -> Result<()> {
    let session = session(engine, id)?.clone();
    let settings = engine.state.settings(Some(&session.workspace));
    let agent = settings.agent("planner", engine.state.family)?;
    let instructions = crate::workspace_prompts::planner(&crate::workspace_prompts::root(), &settings)?;
    let run = engine.directory.join("sessions").join(id).join((session.requests.len() + 1).to_string());
    fs::create_dir_all(&run)?;
    let port: u16 = read(&engine.directory.join("port.json")).unwrap_or(4242);
    write(&run.join("request.json"), &json!({"session":id,"workspace":session.workspace,"prompt":prompt(&session, &text, session.requests.is_empty(), &instructions),"agent":agent,
        "conversation":session.conversation,"command":engine.state.config.planner,"url":format!("http://127.0.0.1:{port}")}))?;
    write(&run.join("launch-intent.json"), &json!({"at":now()}))?;
    let request = Request { text, run: run.clone(), started: now(), planner_ended: None, exit_code: None, usage: None, ended: None, delivery: None, error: None };
    engine.change(|state| {
        let s = state.sessions.get_mut(id).ok_or("Unknown session")?;
        s.status = "running".into();
        s.requests.push(request);
        Ok(Value::Null)
    })?;
    command(std::env::current_exe()?.to_str().ok_or("Invalid executable path")?).arg("--planner").arg(&run).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    Ok(())
}

fn copy_tree(from: &Path, to: &Path, root: bool) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = entry.file_type()?;
        if kind.is_symlink() || SKIPPED.contains(&name.as_str()) || name.ends_with(".pyc") || (root && name == crate::workspace_prompts::LIMITS_FILE) {
            continue;
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &to.join(&name), false)?;
        } else {
            fs::copy(entry.path(), to.join(&name))?;
        }
    }
    Ok(())
}

fn deliver(engine: &mut Engine, id: &str) -> Result<u64> {
    let session = session(engine, id)?.clone();
    let source = session.workspace.join(&session.deliverable);
    require(source.is_dir(), format!("The deliverable folder {} does not exist yet", session.deliverable))?;
    let number = session.deliveries + 1;
    let partial = session.output.join(format!(".{number}.partial"));
    if partial.exists() {
        fs::remove_dir_all(&partial)?;
    }
    copy_tree(&source, &partial, session.deliverable == ".")?;
    fs::rename(&partial, session.output.join(number.to_string()))?;
    engine.change(|state| {
        state.sessions.get_mut(id).ok_or("Unknown session")?.deliveries = number;
        Ok(Value::Null)
    })?;
    Ok(number)
}

fn event(session: &Session, kind: &str, delivery: Option<u64>, extra: Value) -> Value {
    let mut value = json!({"event":kind,"session":session.id,"request":session.requests.len(),"delivery":delivery,
        "path":delivery.map(|n| session.output.join(n.to_string())),"at":now()});
    if let (Some(target), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
        target.extend(extra.clone());
    }
    value
}

pub fn deliver_now(engine: &mut Engine, id: &str) -> Result<Value> {
    require(session(engine, id)?.status == "running", "Deliveries are only accepted while the session is working")?;
    let number = deliver(engine, id)?;
    engine.change(|state| {
        let s = state.sessions.get_mut(id).ok_or("Unknown session")?;
        let notice = event(s, "delivered", Some(number), Value::Null);
        s.outbox.push(notice);
        Ok(Value::Null)
    })?;
    Ok(json!({"delivery":number}))
}

pub fn stop(engine: &mut Engine, id: &str) -> Result<Value> {
    let session = session(engine, id)?.clone();
    if let Some(request) = session.requests.last().filter(|r| r.planner_ended.is_none()) {
        fs::write(request.run.join("stop"), b"")?;
    }
    let tasks: Vec<String> = engine.state.tasks.iter().filter(|t| engine.state.in_scope(t, Some(&session.workspace)) && can_still_run(t) && !t.stopped).map(|t| t.id.clone()).collect();
    for task in tasks {
        engine.control(true, Some(&task))?;
    }
    engine.change(|state| {
        let s = state.sessions.get_mut(id).ok_or("Unknown session")?;
        s.status = "stopped".into();
        if let Some(request) = s.requests.last_mut() {
            request.ended.get_or_insert(now());
        }
        Ok(Value::Null)
    })?;
    view(engine, id)
}

pub fn view(engine: &Engine, id: &str) -> Result<Value> {
    let s = session(engine, id)?;
    let tasks: Vec<&str> = engine.state.tasks.iter().filter(|t| engine.state.in_scope(t, Some(&s.workspace))).map(|t| t.id.as_str()).collect();
    let mut value = serde_json::to_value(s)?;
    value["tasks"] = json!(tasks);
    value["pendingCallbacks"] = json!(s.outbox.len());
    if let Some(object) = value.as_object_mut() {
        object.remove("outbox");
    }
    Ok(value)
}

pub fn list(engine: &Engine) -> Result<Value> {
    Ok(json!(engine.state.sessions.keys().map(|id| view(engine, id)).collect::<Result<Vec<_>>>()?))
}

fn planner_finished(request: &Request) -> Option<Value> {
    if let Ok(exit) = read::<Value>(&request.run.join("exit.json")) {
        return Some(exit);
    }
    let started = request.run.join("run.json").exists();
    let stale = now().saturating_sub(request.started) > 60_000;
    (lock(&request.run.join("lock")).is_ok() && (started || stale)).then(|| json!({"code":-1,"error":"Planner process ended without recording an exit"}))
}

pub fn tick(engine: &mut Engine) -> Result<()> {
    let running: Vec<String> = engine.state.sessions.values().filter(|s| s.status == "running").map(|s| s.id.clone()).collect();
    for id in running {
        let session = session(engine, &id)?.clone();
        let Some(request) = session.requests.last() else { continue };
        if request.planner_ended.is_none() {
            let Some(exit) = planner_finished(request) else { continue };
            engine.change(|state| {
                let s = state.sessions.get_mut(&id).ok_or("Unknown session")?;
                if let Some(conversation) = exit["conversation"].as_str() {
                    s.conversation = Some(conversation.to_owned());
                }
                let r = s.requests.last_mut().ok_or("Missing request")?;
                r.planner_ended = Some(now());
                r.exit_code = exit["code"].as_i64();
                r.usage = exit.get("usage").cloned();
                r.error = exit["error"].as_str().map(str::to_owned);
                Ok(Value::Null)
            })?;
            continue;
        }
        if engine.state.tasks.iter().any(|t| engine.state.in_scope(t, Some(&session.workspace)) && can_still_run(t)) {
            continue;
        }
        let delivery = deliver(engine, &id);
        engine.change(|state| {
            let s = state.sessions.get_mut(&id).ok_or("Unknown session")?;
            s.status = "waiting".into();
            let r = s.requests.last_mut().ok_or("Missing request")?;
            r.ended = Some(now());
            r.delivery = delivery.as_ref().ok().copied();
            let status = if r.exit_code == Some(0) { "completed" } else { "failed" };
            let extra = json!({"status":status,"exitCode":r.exit_code,"error":delivery.as_ref().err().map(|e| e.to_string()).or(r.error.clone())});
            let delivered = r.delivery;
            let notice = event(s, "done", delivered, extra);
            s.outbox.push(notice);
            Ok(Value::Null)
        })?;
    }
    flush(engine)
}

static SENDING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
static SENT: Mutex<Vec<(String, usize, bool)>> = Mutex::new(Vec::new());

fn post(url: &str, notice: &Value) -> bool {
    command("curl.exe").args(["-q", "-sS", "-f", "--max-time", "10", "--noproxy", "*", "-H", "Content-Type: application/json", "-d", &notice.to_string(), url]).output().is_ok_and(|o| o.status.success())
}

fn flush(engine: &mut Engine) -> Result<()> {
    let finished: Vec<(String, usize, bool)> = std::mem::take(&mut *SENT.lock().unwrap());
    for (id, sent, failed) in finished {
        engine.change(|state| {
            if let Some(s) = state.sessions.get_mut(&id) {
                s.outbox.drain(..sent.min(s.outbox.len()));
                s.retry_at = if failed { now() + 5_000 } else { 0 };
            }
            Ok(Value::Null)
        })?;
        SENDING.lock().unwrap().remove(&id);
    }
    let due: Vec<String> = engine.state.sessions.values().filter(|s| !s.outbox.is_empty() && s.retry_at <= now() && !SENDING.lock().unwrap().contains(&s.id)).map(|s| s.id.clone()).collect();
    for id in due {
        let session = session(engine, &id)?.clone();
        let Some(url) = session.callback.clone() else {
            let count = session.outbox.len();
            engine.change(|state| {
                if let Some(s) = state.sessions.get_mut(&id) {
                    s.outbox.drain(..count.min(s.outbox.len()));
                }
                Ok(Value::Null)
            })?;
            continue;
        };
        SENDING.lock().unwrap().insert(id.clone());
        thread::spawn(move || {
            let sent = session.outbox.iter().take_while(|notice| post(&url, notice)).count();
            SENT.lock().unwrap().push((id, sent, sent < session.outbox.len()));
        });
    }
    Ok(())
}

pub fn stop_all(engine: &Engine) {
    for session in engine.state.sessions.values().filter(|s| s.status == "running") {
        if let Some(request) = session.requests.last() {
            let _ = fs::write(request.run.join("stop"), b"");
        }
    }
}

fn uuid(seed: &str) -> String {
    let hex = format!("{:x}", Sha256::digest(format!("{seed}:{}", now())));
    format!("{}-{}-4{}-a{}-{}", &hex[0..8], &hex[8..12], &hex[13..16], &hex[17..20], &hex[20..32])
}

fn arguments(request: &Value, agent: &Agent) -> Result<(Vec<String>, Option<String>)> {
    let conversation = request["conversation"].as_str().map(str::to_owned);
    let custom: Vec<String> = serde_json::from_value(request["command"].clone()).unwrap_or_default();
    if !custom.is_empty() {
        let effort = agent.effort().to_owned();
        return Ok((custom.into_iter().map(|a| a.replace("{model}", &agent.model).replace("{effort}", &effort)).collect(), conversation));
    }
    let effort = agent.effort().to_owned();
    match agent.harness {
        Harness::ClaudeCode => {
            let program = std::env::var("CLAUDE_BIN").ok().or_else(|| crate::adapters::executable("claude", std::env::var_os("PATH")).map(|p| p.to_string_lossy().into_owned())).ok_or("Claude Code executable not found on PATH")?;
            let mut argv: Vec<String> = [program.as_str(), "-p", "--model", &agent.model, "--output-format", "stream-json", "--verbose", "--dangerously-skip-permissions"].map(str::to_owned).into();
            if !effort.is_empty() {
                argv.extend(["--effort".into(), effort]);
            }
            let id = conversation.clone().unwrap_or_else(|| uuid(request["session"].as_str().unwrap_or_default()));
            argv.extend([if conversation.is_some() { "--resume" } else { "--session-id" }.to_owned(), id.clone()]);
            Ok((argv, Some(id)))
        }
        Harness::Codex => {
            let program = std::env::var("CODEX_BIN").unwrap_or_else(|_| "codex".into());
            let mut argv = vec![program, "exec".into()];
            if conversation.is_some() {
                argv.push("resume".into());
            }
            argv.extend(["--json", "--skip-git-repo-check", "--dangerously-bypass-approvals-and-sandbox", "--model", &agent.model, "-c"].map(str::to_owned));
            argv.push(format!("model_reasoning_effort=\"{effort}\""));
            argv.extend(conversation.clone());
            argv.push("-".into());
            Ok((argv, conversation))
        }
    }
}

fn usage(events: &Path) -> (Option<Value>, Option<String>) {
    let (mut input, mut output, mut cached, mut written, mut thread) = (0u64, 0u64, 0u64, 0u64, None);
    let mut seen = false;
    if let Ok(file) = File::open(events) {
        for line in BufReader::new(file).lines().map_while(std::result::Result::ok) {
            let Ok(e) = serde_json::from_str::<Value>(&line) else { continue };
            let n = |u: &Value, key: &str| u[key].as_u64().unwrap_or(0);
            if e["type"] == "result" {
                let u = &e["usage"];
                (cached, written) = (cached + n(u, "cache_read_input_tokens"), written + n(u, "cache_creation_input_tokens"));
                input += n(u, "input_tokens") + n(u, "cache_read_input_tokens") + n(u, "cache_creation_input_tokens");
                output += n(u, "output_tokens");
                seen = true;
            } else if e["type"] == "turn.completed" {
                let u = &e["usage"];
                input += n(u, "input_tokens");
                cached += n(u, "cached_input_tokens");
                output += n(u, "output_tokens");
                seen = true;
            } else if e["type"] == "thread.started" {
                thread = e["thread_id"].as_str().map(str::to_owned);
            }
        }
    }
    (seen.then(|| json!({"input_tokens":input,"cached_input_tokens":cached,"cache_write_input_tokens":written,"output_tokens":output,"totalTokens":input + output})), thread)
}

pub fn run(directory: &Path) -> Result<()> {
    let _guard = lock(&directory.join("lock"))?;
    if directory.join("exit.json").exists() {
        return Ok(());
    }
    let request: Value = read(&directory.join("request.json"))?;
    let agent: Agent = serde_json::from_value(request["agent"].clone())?;
    let (argv, conversation) = arguments(&request, &agent)?;
    let workspace = PathBuf::from(request["workspace"].as_str().ok_or("Missing workspace")?);
    let mut cli = command(&argv[0]);
    cli.args(&argv[1..]).current_dir(&workspace).stdin(Stdio::piped()).stdout(File::create(directory.join("events.jsonl"))?).stderr(File::create(directory.join("log.txt"))?);
    cli.env("COORDINATOR_URL", request["url"].as_str().unwrap_or_default()).env("COORDINATOR_SESSION", request["session"].as_str().unwrap_or_default()).env("COORDINATOR_WORKSPACE", &workspace);
    for (name, _) in std::env::vars_os() {
        if crate::adapters::hosted_claude_environment(&name.to_string_lossy()) {
            cli.env_remove(name);
        }
    }
    if agent.harness == Harness::Codex {
        let profile = std::env::var_os("USERPROFILE").map(PathBuf::from).filter(|path| path.is_absolute());
        cli.envs(profile.filter(|_| std::env::var_os("CODEX_HOME").is_none_or(|home| home.is_empty())).map(|path| ("CODEX_HOME", path.join(".codex"))));
        cli.env_remove("CODEX_WINDOWS_REGISTERED_CORE");
    }
    if let Some(data) = directory.ancestors().nth(3) {
        match crate::kill_guard::install(data).and_then(|dir| crate::kill_guard::environment(&dir)) {
            Ok(vars) => drop(cli.envs(vars)),
            Err(error) => fs::write(directory.join("kill-guard-error.txt"), error.to_string())?,
        }
    }
    let job = Job::new()?;
    let mut child = cli.spawn()?;
    job.contain(&child)?;
    write(&directory.join("run.json"), &json!({"planner":child.id(),"supervisor":std::process::id(),"started":now(),"argv":argv}))?;
    let prompt = request["prompt"].as_str().unwrap_or_default().as_bytes().to_vec();
    let mut pipe = child.stdin.take().ok_or("Missing planner stdin")?;
    thread::spawn(move || {
        let _ = pipe.write_all(&prompt);
    });
    let mut stopped = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if !stopped && directory.join("stop").exists() {
            job.stop()?;
            stopped = true;
        }
        thread::sleep(Duration::from_millis(200));
    };
    job.drain()?;
    let (used, thread) = usage(&directory.join("events.jsonl"));
    write(&directory.join("exit.json"), &json!({"code":status.code(),"stopped":stopped,"conversation":thread.or(conversation),"usage":used,"ended":now()}))
}

#[cfg(test)]
#[path = "sessions_tests.rs"]
mod tests;
