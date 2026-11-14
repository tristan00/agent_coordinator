use crate::engine::{Engine, Task, covers, resolve_scope_path};
use crate::{Result, lock, process::local_json, write};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use tiny_http::{Header, Request, Response, Server};
fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name, value).unwrap()
}
fn send(request: Request, status: u16, value: Value) -> Result<()> {
    let response = Response::from_string(value.to_string()).with_status_code(status).with_header(header("Content-Type", "application/json")).with_header(header("Cache-Control", "no-store"));
    let _ = request.respond(response);
    Ok(())
}
pub fn deployment() -> &'static Value {
    static LIVE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    LIVE.get_or_init(|| {
        let executable = std::env::current_exe().ok();
        let digest = executable.as_deref().and_then(|path| crate::evidence::digest(path).ok());
        json!({"status":if digest.is_some() {"verified_live"} else {"identity_unavailable"},
            "pid":std::process::id(),"executable":executable,"digest":digest,
            "protocol":1,"frontendSource":option_env!("COORDINATOR_FRONTEND_SHA"),"source":option_env!("COORDINATOR_SOURCE_SHA"),
            "frontend":crate::redeploy::frontend_digest()})
    })
}
fn authorized(request: &Request, port: u16) -> bool {
    let local = format!("127.0.0.1:{port}");
    let localhost = format!("localhost:{port}");
    let get_header = |name: &str| request.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str());
    get_header("Host").is_some_and(|v| v == local || v == localhost) && !get_header("Origin").is_some_and(|v| v != format!("http://{local}") && v != format!("http://{localhost}"))
}
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = bytes.get(index + 1..index + 3).and_then(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok());
        match (bytes[index], hex) {
            (b'%', Some(value)) => {
                output.push(value);
                index += 3;
            }
            (b'+', _) => {
                output.push(b' ');
                index += 1;
            }
            (byte, _) => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}
struct UpdateQuery {
    workspace: Option<PathBuf>,
    after: Option<u64>,
    wait: u64,
}
fn schema_hint(error: Box<dyn std::error::Error + Send + Sync>) -> Box<dyn std::error::Error + Send + Sync> {
    format!("{error}. Allowed values and this workspace's limits: GET /api/task-schema?workspace=W").into()
}
fn update_query(url: &str) -> Result<UpdateQuery> {
    let mut query = UpdateQuery { workspace: None, after: None, wait: 0 };
    for pair in url.split_once('?').map_or("", |(_, query)| query).split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = decode(value);
        match name {
            "workspace" => query.workspace = Some(crate::engine::workspace(Path::new(&value))?),
            "after" => query.after = Some(value.parse().map_err(|_| "after must be a sequence number")?),
            "wait" => query.wait = value.parse().map_err(|_| "wait must be whole seconds")?,
            _ => return Err(format!("Unknown updates parameter {name}; use workspace, after or wait").into()),
        }
    }
    Ok(query)
}
struct Parked {
    request: Request,
    query: UpdateQuery,
    until: Instant,
}
fn parkable(request: &Request, engine: &Engine, port: u16) -> Option<UpdateQuery> {
    if request.method().as_str() != "GET" || request.url().split('?').next() != Some("/api/updates") || !authorized(request, port) {
        return None;
    }
    update_query(request.url())
        .ok()
        .filter(|query| query.wait > 0 && engine.state.settings(query.workspace.as_deref()).updates == crate::history::Updates::Events && !engine.changed_since(query.workspace.as_deref(), query.after))
}
fn release(parked: Vec<Parked>, engine: &Engine, error: &str, all: bool) -> Vec<Parked> {
    let now = Instant::now();
    let (ready, waiting): (Vec<_>, Vec<_>) = parked.into_iter().partition(|p| all || now >= p.until || engine.changed_since(p.query.workspace.as_deref(), p.query.after));
    for p in ready {
        let _ = send(p.request, 200, engine.updates(p.query.workspace.as_deref(), p.query.after, error));
    }
    waiting
}
fn handle(mut request: Request, engine: &mut Engine, port: u16, error: &str, shutdown: &mut bool) -> Result<()> {
    if !authorized(&request, port) {
        return send(request, 403, json!({"error":"Invalid host or origin"}));
    }
    let full = request.url().to_owned();
    let url = full.split('?').next().unwrap().to_owned();
    let method = request.method().as_str().to_owned();
    let parts: Vec<_> = url.split('/').collect();
    if method == "GET" {
        let asset = match url.as_str() {
            "/" => Some((include_str!("../web/index.html"), "text/html; charset=utf-8")),
            "/app.js" => Some((include_str!("../web/app.js"), "text/javascript")),
            "/style.css" => Some((include_str!("../web/style.css"), "text/css")),
            _ => None,
        };
        if let Some((body, mime)) = asset {
            request.respond(
                Response::from_string(body).with_header(header("Content-Type", mime)).with_header(header("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self'; frame-ancestors 'none'")),
            )?;
            return Ok(());
        }
        if let ["", "api", "attempts", id, "log"] = parts.as_slice()
            && engine.state.tasks.iter().chain(&engine.state.archived).any(|task| task.attempts.iter().any(|a| a.id == *id))
        {
            let file = engine.attempt_path(id).join("log.txt");
            if file.exists() {
                request.respond(Response::from_file(File::open(file)?).with_header(header("Content-Type", "text/plain; charset=utf-8")))?;
                return Ok(());
            }
            return Ok(request.respond(Response::from_string("Worker has not produced a log yet."))?);
        }
        return match get(engine, &parts, &full, error) {
            Ok(Some(value)) => send(request, 200, value),
            Ok(None) => send(request, 404, json!({"error":"Not found"})),
            Err(error) => send(request, 500, json!({"error":error.to_string()})),
        };
    }
    if !request.headers().iter().any(|h| h.field.as_str().as_str().eq_ignore_ascii_case("Content-Type") && h.value.as_str().starts_with("application/json")) {
        return send(request, 415, json!({"error":"Use application/json"}));
    }
    let outcome = (|| -> Result<Value> {
        let mut body = String::new();
        request.as_reader().read_to_string(&mut body)?;
        let input: Value = serde_json::from_str(if body.is_empty() { "{}" } else { &body })?;
        match (method.as_str(), parts.as_slice()) {
            ("POST", ["", "api", "deployment"]) => crate::redeploy::enqueue(input, &engine.directory, port),
            ("POST", ["", "api", "deployment", "quiesce"]) => {
                crate::require(input["expected"] == deployment()["digest"] && input["expected"].is_string(), "Running coordinator identity changed")?;
                crate::redeploy::authorize_handoff(&engine.directory, &input)?;
                *shutdown = true;
                Ok(json!({"ready":true,"paused":engine.state.paused}))
            }
            ("POST", ["", "api", "analytics"]) => crate::analytics::report(&engine.state, input),
            ("POST", ["", "api", "deployment", "handoff"]) => {
                crate::require(input["expected"] == deployment()["digest"] && input["expected"].is_string(), "Running coordinator identity changed")?;
                let event = handoff(engine, input["action"].as_str().ok_or("Missing deployment action")?, port)?;
                *shutdown = true;
                Ok(event)
            }
            ("POST", ["", "api", "wipe"]) => engine.wipe(),
            ("POST", ["", "api", "sessions"]) => crate::sessions::create(engine, input),
            ("POST", ["", "api", "sessions", id, "messages"]) => crate::sessions::message(engine, id, input),
            ("POST", ["", "api", "sessions", id, "deliver"]) => crate::sessions::deliver_now(engine, id),
            ("POST", ["", "api", "sessions", id, "stop"]) => crate::sessions::stop(engine, id),
            ("POST", ["", "api", "tasks"]) => engine.create(input).map_err(schema_hint),
            ("POST", ["", "api", "workspaces"]) => crate::workspace_prompts::create(engine, input, &crate::workspace_prompts::root()),
            ("POST", ["", "api", "workspaces", "generate"]) => crate::workspace_prompts::generate(engine, input, &crate::workspace_prompts::root()),
            ("POST", ["", "api", "tasks", "batch"]) => engine.create_batch(input).map_err(schema_hint),
            ("PATCH", ["", "api", "tasks", id]) => engine.edit(id, input),
            ("POST", ["", "api", "tasks", id, "replan"]) => engine.request_replan(id, input),
            ("DELETE", ["", "api", "tasks", id]) => engine.remove(id).map(|()| json!({"ok":true})),
            ("PUT", ["", "api", "config"]) => engine.configure(input).map(|()| json!({"ok":true})),
            ("POST", ["", "api", action @ ("stop" | "resume")]) => engine.control(*action == "stop", None).map(|()| json!({"ok":true})),
            ("POST", ["", "api", "tasks", id, action @ ("stop" | "resume")]) => engine.control(*action == "stop", Some(id)).map(|()| json!({"ok":true})),
            _ => Err("Unknown route".into()),
        }
    })();
    match outcome {
        Ok(value) => send(request, if method == "POST" && url == "/api/tasks" { 201 } else { 200 }, value),
        Err(error) => send(request, 400, json!({"error":error.to_string()})),
    }
}
fn handoff(engine: &mut Engine, action: &str, port: u16) -> Result<Value> {
    let _redeploy = lock(&engine.directory.join("redeploy.lock"))?;
    let mut target = if action == "rollback" {
        deployment_event(&engine.directory)["from"].clone()
    } else {
        let task = &engine.state.tasks[engine.task(action)?];
        crate::require(deployment_reviewed(task), "Deployment requires an accepted candidate for this revision")?;
        let attempt = task.attempts.iter().rev().find(|a| a.revision == task.revision && a.status() == Some("completed"));
        attempt.and_then(|a| a.evidence.configuration.as_ref()).map(|c| c["artifact"].clone()).filter(Value::is_object).ok_or("Accepted task has no candidate artifact")?
    };
    let path = target["path"].as_str().or(target["executable"].as_str()).ok_or("Deployment target is unavailable")?.to_owned();
    let digest = crate::evidence::digest(Path::new(&path))?;
    crate::require(target["digest"] == digest && target["digest"] != deployment()["digest"], "Deployment target has a mismatched or identical build digest")?;
    let staged = engine.directory.join("deployed").join(&digest["sha256:".len()..]).join("agent-coordinator.exe");
    if action != "rollback" && !staged.exists() {
        fs::create_dir_all(staged.parent().unwrap())?;
        fs::copy(&path, &staged)?;
    }
    target["path"] = if action == "rollback" { json!(path) } else { json!(staged) };
    let mut event = json!({"status":"pending_deployment","action":action,"from":deployment(),"target":target,"at":crate::now()});
    write(&engine.directory.join("deployment.json"), &event)?;
    let helper = server_command(deployment()["executable"].as_str().ok_or("Missing running executable")?, &engine.directory, port).and_then(|mut command| Ok(command.arg("--finish-deploy").spawn()?));
    if let Err(error) = helper {
        event["status"] = json!("deployment_blocked");
        event["error"] = json!(error.to_string());
        write(&engine.directory.join("deployment.json"), &event)?;
        return Err(error);
    }
    Ok(event)
}
fn get(engine: &mut Engine, parts: &[&str], full: &str, error: &str) -> Result<Option<Value>> {
    let value = match parts {
        ["", "api", "updates"] => {
            let query = update_query(full)?;
            engine.updates(query.workspace.as_deref(), query.after, error)
        }
        ["", "api", "agents"] => engine.state.models(update_query(full)?.workspace.as_deref()),
        ["", "api", "tasks"] => engine.scoped_tasks(update_query(full)?.workspace.as_deref())?,
        ["", "api", "task-schema"] => engine.state.task_schema(update_query(full)?.workspace.as_deref()),
        ["", "api", "analytics"] => crate::analytics::report(&engine.state, json!({}))?,
        ["", "api", "prompt-candidates"] => crate::workspace_prompts::catalog(&crate::workspace_prompts::root())?,
        ["", "api", "deployment"] => json!({"live":deployment(),"dataDirectory":engine.directory,"event":deployment_event(&engine.directory),"redeployment":crate::redeploy::event(&engine.directory)}),
        ["", "api", "deployment", "requests", id] => crate::redeploy::receipt(&engine.directory, id)?,
        ["", "api", "sessions"] => crate::sessions::list(engine)?,
        ["", "api", "sessions", id] => crate::sessions::view(engine, id)?,
        ["", "api", "kill-guard"] => json!({"directory":crate::kill_guard::install(&engine.directory)?,"variable":crate::kill_guard::VARIABLE}),
        ["", "api", "state"] => {
            engine.sync_waiting()?;
            let tasks = engine.state.tasks.iter().map(|task| engine.task_value(task, false)).collect::<Result<Vec<_>>>()?;
            let state = &engine.state;
            let workspace_ids: serde_json::Map<String, Value> = state.workspace_ids.iter().map(|(path, id)| (path.to_string_lossy().into_owned(), json!(id.abbreviation))).collect();
            json!({"tasks":tasks,"workspaceIds":workspace_ids,"paused":state.paused,"holdUntil":state.hold_until,"controls":state.controls,"config":state.config,"error":error,
                "dataDirectory":engine.directory,"deployment":deployment(),"deploymentEvent":deployment_event(&engine.directory),"versions":crate::history::versions()})
        }
        ["", "api", "tasks", id] => match engine.task(id) {
            Ok(index) => engine.task_value(&engine.state.tasks[index], true)?,
            Err(_) => return Ok(None),
        },
        _ => return Ok(None),
    };
    Ok(Some(value))
}
pub(crate) fn data_directory() -> Result<PathBuf> {
    let directory =
        std::env::var_os("COORDINATOR_DATA").map(PathBuf::from).or_else(|| std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("AgentCoordinator"))).ok_or("Set LOCALAPPDATA or COORDINATOR_DATA")?;
    let ambiguous = directory.components().any(|part| match part {
        Component::ParentDir => true,
        Component::Normal(name) if cfg!(windows) => name.as_encoded_bytes().ends_with(b".") || name.as_encoded_bytes().ends_with(b" ") || name.as_encoded_bytes().contains(&b':'),
        _ => false,
    });
    crate::require(directory.is_absolute() && !ambiguous, "COORDINATOR_DATA must be an absolute directory without ambiguous components")?;
    let project = resolve_scope_path(Path::new(option_env!("COORDINATOR_SOURCE_ROOT").unwrap_or(env!("CARGO_MANIFEST_DIR"))))?;
    let directory = resolve_scope_path(&directory)?;
    crate::require(!covers(&project, &directory), "COORDINATOR_DATA must be outside the project directory")?;
    fs::create_dir_all(&directory)?;
    Ok(directory.canonicalize()?)
}
fn deployment_event(directory: &Path) -> Value {
    crate::read(&directory.join("deployment.json")).unwrap_or(Value::Null)
}
pub(crate) fn server_command(path: &str, directory: &Path, port: u16) -> Result<std::process::Command> {
    use std::os::windows::io::AsRawHandle;
    for handle in [std::io::stdin().as_raw_handle(), std::io::stdout().as_raw_handle(), std::io::stderr().as_raw_handle()] {
        unsafe {
            windows_sys::Win32::Foundation::SetHandleInformation(handle as _, windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT, 0);
        }
    }
    let log = fs::OpenOptions::new().create(true).append(true).open(directory.join("deployment-server.log"))?;
    let mut command = crate::process::command(path);
    command.env("COORDINATOR_DATA", directory).env("PORT", port.to_string()).stdin(std::process::Stdio::null()).stdout(log.try_clone()?).stderr(log);
    Ok(command)
}
pub fn deploy(action: &str) -> Result<()> {
    let event = deploy_inner(action).unwrap_or_else(|error| json!({"status":"deployment_blocked","action":action,"error":error.to_string(),
        "recovery":"Inspect GET /api/deployment. If absent, request an authorized scoped legacy handoff. If offline, start the retained verified executable with the same COORDINATOR_DATA and PORT, then retry --deploy TASK_ID. Preserve pauses, workers and evidence; do not repeat denied operations or stop the board."}));
    println!("{event}");
    data_directory().and_then(|directory| write(&directory.join("deployment-recovery.json"), &event)).unwrap_or_else(|error| eprintln!("Recovery receipt unavailable: {error}"));
    crate::require(event["status"] == "verified_live", "Deployment incomplete; follow the reported recovery action")
}
fn deploy_inner(action: &str) -> Result<Value> {
    let directory = data_directory()?;
    let _guard = lock(&directory.join("deployment.lock"))?;
    let port: u16 = crate::read(&directory.join("port.json"))?;
    let live = local_json(port, "deployment", None)?["live"].clone();
    let event = local_json(port, "deployment/handoff", Some(&json!({"expected":live["digest"],"action":action})))?;
    let deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < deadline {
        let finished = deployment_event(&directory);
        if finished["at"] == event["at"] && finished["status"] != "pending_deployment" {
            return Ok(finished);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("Deployment helper did not finish; inspect deployment.json and deployment-server.log".into())
}
pub fn finish_deploy() -> Result<()> {
    let directory = data_directory()?;
    let _guard = lock(&directory.join("deployment-helper.lock"))?;
    let port: u16 = crate::read(&directory.join("port.json"))?;
    let mut event = deployment_event(&directory);
    crate::require(!(event["status"] != "pending_deployment" || event["from"]["digest"] != deployment()["digest"]), "No pending handoff owned by this build")?;
    let live = event["from"].clone();
    let launch = |path: &str, digest: &Value| -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(guard) = lock(&directory.join("server.lock")) {
                drop(guard);
                break;
            }
            crate::require(!(Instant::now() >= deadline), "Previous server did not release the board lock")?;
            std::thread::sleep(Duration::from_millis(100));
        }
        crate::require(!(crate::evidence::digest(Path::new(path))? != *digest), "Build changed before launch")?;
        let mut child = server_command(path, &directory, port)?.spawn()?;
        while Instant::now() < deadline {
            if let Some(status) = child.try_wait()? {
                return Err(format!("Target server exited with {status}").into());
            }
            if let Ok(response) = local_json(port, "deployment", None)
                && response["live"]["digest"] == *digest
                && response["live"]["pid"] == child.id()
            {
                return Ok(response["live"].clone());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        child.kill()?;
        child.wait()?;
        Err("Target server did not verify its build and process through the deployment API".into())
    };
    let result = launch(event["target"]["path"].as_str().ok_or("Missing deployment target")?, &event["target"]["digest"]);
    match result {
        Ok(verified) => {
            event["status"] = json!("verified_live");
            event["verified"] = verified;
        }
        Err(error) => {
            event["status"] = json!("deployment_blocked");
            event["error"] = json!(error.to_string());
            event["rollback"] = match launch(live["executable"].as_str().ok_or("Missing previous executable")?, &live["digest"]) {
                Ok(verified) => verified,
                Err(error) => json!({"error":error.to_string()}),
            };
        }
    }
    write(&directory.join("deployments").join(format!("{}.json", event["at"])), &event)?;
    write(&directory.join("deployment.json"), &event)
}
fn notify(engine: &mut Engine) {
    let command = engine.state.config.notify.clone();
    for notice in engine.notices.drain(..) {
        if let Some(program) = command.first() {
            let child = crate::process::command(program).args(&command[1..]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
            if let Ok(mut child) = child {
                use std::io::Write;
                let _ = child.stdin.take().map(|mut pipe| pipe.write_all(notice.to_string().as_bytes()));
            }
        }
    }
}
pub fn serve() -> Result<()> {
    let directory = data_directory()?;
    let _guard = lock(&directory.join("server.lock"))?;
    let mut engine = Engine::new(directory)?;
    let server = Server::http(format!("127.0.0.1:{}", std::env::var("PORT").unwrap_or_else(|_| "4242".into())))?;
    let port = server.server_addr().to_ip().unwrap().port();
    write(&engine.directory.join("port.json"), &port)?;
    println!("Coordinator: http://127.0.0.1:{port}");
    let mut last_tick = Instant::now();
    let mut error = String::new();
    let mut shutdown = false;
    let mut parked = Vec::new();
    while !shutdown {
        if let Some(request) = server.recv_timeout(Duration::from_millis(100))? {
            if let Some(query) = parkable(&request, &engine, port) {
                let until = Instant::now() + Duration::from_secs(query.wait);
                parked.push(Parked { request, query, until });
            } else if let Err(failure) = handle(request, &mut engine, port, &error, &mut shutdown) {
                eprintln!("{failure}");
            }
        }
        if !shutdown && last_tick.elapsed() >= Duration::from_millis(100) {
            error = engine.tick().and_then(|()| crate::sessions::tick(&mut engine)).err().map(|e| e.to_string()).unwrap_or_default();
            last_tick = Instant::now();
            if let Some(id) = engine.pending_deploy.take() {
                match handoff(&mut engine, &id, port) {
                    Ok(_) => shutdown = true,
                    Err(failure) => engine.notices.push(json!({"event":"deploy_failed","taskId":id,"error":failure.to_string()})),
                }
            }
            notify(&mut engine);
        }
        parked = release(parked, &engine, &error, shutdown);
    }
    Ok(())
}
pub(crate) fn deployment_reviewed(task: &Task) -> bool {
    task.stage == "done" && task.attempts.iter().rev().any(|attempt| attempt.revision == task.revision && attempt.stage == "review" && attempt.result.as_ref().is_some_and(|result| result["status"] == "accepted"))
}

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;
