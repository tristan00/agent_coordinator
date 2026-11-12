use crate::engine::{Engine, Task, covers, resolve_scope_path};
use crate::{Result, history::Actor, lock, process::local_json, write};
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
            "pid":std::process::id(),"executable":executable,"digest":digest})
    })
}
fn handle(mut request: Request, engine: &mut Engine, port: u16, error: &str, shutdown: &mut bool) -> Result<()> {
    let local = format!("127.0.0.1:{port}");
    let localhost = format!("localhost:{port}");
    let get_header = |name: &str| request.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str());
    if !get_header("Host").is_some_and(|v| v == local || v == localhost) || get_header("Origin").is_some_and(|v| v != format!("http://{local}") && v != format!("http://{localhost}")) {
        return send(request, 403, json!({"error":"Invalid host or origin"}));
    }
    let url = request.url().split('?').next().unwrap().to_owned();
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
        return match get(engine, &parts, error) {
            Ok(Some(value)) => send(request, 200, value),
            Ok(None) => send(request, 404, json!({"error":"Not found"})),
            Err(error) => send(request, 500, json!({"error":error.to_string()})),
        };
    }
    if !get_header("Content-Type").is_some_and(|v| v.starts_with("application/json")) {
        return send(request, 415, json!({"error":"Use application/json"}));
    }
    let outcome = (|| -> Result<Value> {
        let mut body = String::new();
        request.as_reader().read_to_string(&mut body)?;
        let input: Value = serde_json::from_str(if body.is_empty() { "{}" } else { &body })?;
        let reason = || input["reason"].as_str().ok_or("Control requires an explicit reason");
        match (method.as_str(), parts.as_slice()) {
            ("POST", ["", "api", "analytics"]) => crate::analytics::report(&engine.state, input),
            ("POST", ["", "api", "deployment", "handoff"]) => {
                crate::require(input["expected"] == deployment()["digest"] && input["expected"].is_string(), "Running coordinator identity changed")?;
                let event = handoff(engine, input["action"].as_str().ok_or("Missing deployment action")?, port)?;
                *shutdown = true;
                Ok(event)
            }
            ("POST", ["", "api", "tasks"]) => engine.create(input),
            ("POST", ["", "api", "tasks", "batch"]) => engine.create_batch(input),
            ("PATCH", ["", "api", "tasks", id]) => engine.edit(id, input),
            ("POST", ["", "api", "tasks", id, "replan"]) => engine.request_replan(id, input),
            ("DELETE", ["", "api", "tasks", id]) => engine.remove(id, Actor::api(&input)?).map(|()| json!({"ok":true})),
            ("PUT", ["", "api", "config"]) => engine.configure(input).map(|()| json!({"ok":true})),
            ("POST", ["", "api", action @ ("stop" | "resume")]) => engine.control_as(*action == "stop", None, Actor::api(&input)?, reason()?).map(|()| json!({"ok":true})),
            ("POST", ["", "api", "tasks", id, action @ ("stop" | "resume")]) => engine.control_as(*action == "stop", Some(id), Actor::api(&input)?, reason()?).map(|()| json!({"ok":true})),
            _ => Err("Unknown route".into()),
        }
    })();
    match outcome {
        Ok(value) => send(request, if method == "POST" && url == "/api/tasks" { 201 } else { 200 }, value),
        Err(error) => send(request, 400, json!({"error":error.to_string()})),
    }
}
fn handoff(engine: &mut Engine, action: &str, port: u16) -> Result<Value> {
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
fn get(engine: &mut Engine, parts: &[&str], error: &str) -> Result<Option<Value>> {
    let value = match parts {
        ["", "api", "analytics"] => crate::analytics::report(&engine.state, json!({}))?,
        ["", "api", "deployment"] => json!({"live":deployment(),"event":deployment_event(&engine.directory)}),
        ["", "api", "state"] => {
            engine.sync_waiting()?;
            let tasks = engine.state.tasks.iter().map(|task| engine.task_value(task, false)).collect::<Result<Vec<_>>>()?;
            let state = &engine.state;
            json!({"tasks":tasks,"paused":state.paused,"holdUntil":state.hold_until,"controls":state.controls,"config":state.config,"error":error,
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
fn data_directory() -> Result<PathBuf> {
    let directory =
        std::env::var_os("COORDINATOR_DATA").map(PathBuf::from).or_else(|| std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("AgentCoordinator"))).ok_or("Set LOCALAPPDATA or COORDINATOR_DATA")?;
    let ambiguous = directory.components().any(|part| match part {
        Component::ParentDir => true,
        Component::Normal(name) if cfg!(windows) => name.as_encoded_bytes().ends_with(b".") || name.as_encoded_bytes().ends_with(b" ") || name.as_encoded_bytes().contains(&b':'),
        _ => false,
    });
    crate::require(directory.is_absolute() && !ambiguous, "COORDINATOR_DATA must be an absolute directory without ambiguous components")?;
    let project = resolve_scope_path(Path::new(env!("CARGO_MANIFEST_DIR")))?;
    let directory = resolve_scope_path(&directory)?;
    crate::require(!covers(&project, &directory), "COORDINATOR_DATA must be outside the project directory")?;
    fs::create_dir_all(&directory)?;
    Ok(directory.canonicalize()?)
}
fn deployment_event(directory: &Path) -> Value {
    crate::read(&directory.join("deployment.json")).unwrap_or(Value::Null)
}
fn server_command(path: &str, directory: &Path, port: u16) -> Result<std::process::Command> {
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
    while !shutdown {
        if let Some(request) = server.recv_timeout(Duration::from_millis(100))?
            && let Err(failure) = handle(request, &mut engine, port, &error, &mut shutdown)
        {
            eprintln!("{failure}");
        }
        if !shutdown && last_tick.elapsed() >= Duration::from_millis(100) {
            error = engine.tick().err().map(|e| e.to_string()).unwrap_or_default();
            last_tick = Instant::now();
            if let Some(id) = engine.pending_deploy.take() {
                match handoff(&mut engine, &id, port) {
                    Ok(_) => shutdown = true,
                    Err(failure) => engine.notices.push(json!({"event":"deploy_failed","taskId":id,"error":failure.to_string()})),
                }
            }
            notify(&mut engine);
        }
    }
    Ok(())
}
pub(crate) fn deployment_reviewed(task: &Task) -> bool {
    task.stage == "done" && task.attempts.iter().rev().any(|attempt| attempt.revision == task.revision && attempt.stage == "review" && attempt.result.as_ref().is_some_and(|result| result["status"] == "accepted"))
}
