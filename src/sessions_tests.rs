use super::*;
use crate::history;
use crate::test_support::Temp;

fn setup(temp: &Temp, planner: &str) -> (Engine, PathBuf) {
    let mut engine = temp.engine();
    let source = temp.dir("config");
    let candidates = crate::workspace_prompts::root();
    let pick = |role: &str| {
        let name = fs::read_dir(candidates.join(role)).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".md")).min().unwrap();
        let text = fs::read_to_string(candidates.join(role).join(&name)).unwrap();
        json!({"candidate":name,"sha256":format!("{:x}", Sha256::digest(text.as_bytes()))})
    };
    let settings: history::WorkspaceSettings = serde_json::from_value(json!({"planner":"sol_6_1_medium","worker":"sol_6_1_light",
        "prompts":{"project_planner":pick("project_planner"),"worker_protocol":pick("worker_protocol")}})).unwrap();
    let mut state = engine.state.clone();
    state.family = Some(crate::agents::Family::Codex);
    state.config.workspaces.insert(workspace(&source).unwrap(), settings);
    state.config.planner = vec!["cmd.exe".into(), "/d".into(), "/c".into(), planner.into()];
    engine.commit(state).unwrap();
    write(&engine.directory.join("port.json"), &4242).unwrap();
    (engine, source)
}

fn run_planner(engine: &Engine, id: &str) {
    let request = engine.state.sessions[id].requests.last().unwrap().run.clone();
    run(&request).unwrap();
}

#[test]
fn a_session_copies_the_workspace_runs_the_planner_and_delivers_on_completion() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "mkdir submission & echo built> submission\\app.txt & echo raw> data.csv");
    let case = temp.path().join("case");
    let output = temp.path().join("out");
    let created = create(&mut engine, json!({"workspace":source,"directory":case,"request":"Build it","output":output,"deliverable":"submission"})).unwrap();
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["status"], "running");
    assert_eq!(fs::read_dir(&case).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect::<Vec<_>>(), ["LIMITS.json"]);
    assert!(engine.state.config.workspaces.contains_key(&workspace(&case).unwrap()));
    let request: Value = read(&engine.state.sessions[&id].requests[0].run.join("request.json")).unwrap();
    let prompt = request["prompt"].as_str().unwrap();
    assert!(prompt.contains("Request:\nBuild it") && prompt.contains("Planner instructions:\n") && !prompt.contains("Limits"));
    assert!(!prompt.contains("PROJECT_PLANNER.md") && !prompt.contains("WORKER_PROTOCOL.md"));
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    assert_eq!(engine.state.sessions[&id].requests[0].exit_code, Some(0));
    assert_eq!(engine.state.sessions[&id].status, "running");
    tick(&mut engine).unwrap();
    let session = &engine.state.sessions[&id];
    assert_eq!((session.status.as_str(), session.deliveries, session.requests[0].delivery), ("waiting", 1, Some(1)));
    assert_eq!(fs::read_to_string(output.join("1").join("app.txt")).unwrap().trim(), "built");
    assert!(!output.join("1").join("data.csv").exists());
}

#[test]
fn the_session_stays_open_while_tasks_can_still_run_and_follow_ups_resume_it() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "mkdir submission & echo v> submission\\v.txt");
    let case = temp.path().join("case");
    let id = create(&mut engine, json!({"workspace":source,"directory":case,"request":"First","output":temp.path().join("out"),"deliverable":"submission"})).unwrap()["id"].as_str().unwrap().to_owned();
    engine.create(json!({"title":"Pending work","outcome":"Done","cwd":case,"acceptance":["Works"],"standalone":true})).unwrap();
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    assert_eq!(engine.state.sessions[&id].status, "running");
    assert!(message(&mut engine, &id, json!({"request":"Too early"})).unwrap_err().to_string().contains("still working"));
    let task = engine.state.tasks[0].id.clone();
    engine.control(true, Some(&task)).unwrap();
    tick(&mut engine).unwrap();
    assert_eq!(engine.state.sessions[&id].status, "waiting");
    message(&mut engine, &id, json!({"request":"Second"})).unwrap();
    let second: Value = read(&engine.state.sessions[&id].requests[1].run.join("request.json")).unwrap();
    assert!(second["prompt"].as_str().unwrap().starts_with("New request from the same user"));
    assert_eq!(engine.state.sessions[&id].status, "running");
}

#[test]
fn deliveries_copy_snapshots_and_stop_ends_the_session() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "exit 0");
    let case = temp.path().join("case");
    let output = temp.path().join("out");
    let id = create(&mut engine, json!({"workspace":source,"directory":case,"request":"Work","output":output,"deliverable":"submission"})).unwrap()["id"].as_str().unwrap().to_owned();
    assert!(deliver_now(&mut engine, &id).unwrap_err().to_string().contains("does not exist yet"));
    fs::create_dir_all(case.join("submission").join("node_modules")).unwrap();
    fs::write(case.join("submission").join("main.py"), "print(1)").unwrap();
    fs::write(case.join("submission").join("node_modules").join("big.js"), "x").unwrap();
    assert_eq!(deliver_now(&mut engine, &id).unwrap()["delivery"], 1);
    assert!(output.join("1").join("main.py").is_file() && !output.join("1").join("node_modules").exists());
    assert_eq!(engine.state.sessions[&id].outbox[0]["event"], "delivered");
    stop(&mut engine, &id).unwrap();
    assert_eq!(engine.state.sessions[&id].status, "stopped");
    assert!(engine.state.sessions[&id].requests[0].run.join("stop").exists());
}

#[test]
fn requests_are_validated_before_anything_starts() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "exit 0");
    let out = temp.path().join("out");
    assert!(create(&mut engine, json!({"workspace":temp.dir("unknown"),"request":"x","output":out})).is_err());
    assert!(create(&mut engine, json!({"workspace":source,"request":" ","output":out})).is_err());
    assert!(create(&mut engine, json!({"workspace":source,"request":"x","output":out,"deliverable":"../up"})).is_err());
    assert!(create(&mut engine, json!({"workspace":source,"request":"x","output":out,"callback":"http://example.com/x"})).is_err());
    let busy = temp.dir("busy");
    fs::write(busy.join("file.txt"), "x").unwrap();
    assert!(create(&mut engine, json!({"workspace":source,"directory":busy,"request":"x","output":out})).unwrap_err().to_string().contains("new or empty"));
    assert!(engine.state.sessions.is_empty());
}

#[test]
fn a_silent_callback_receiver_never_blocks_the_tick() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "exit 0");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let callback = format!("http://127.0.0.1:{}/hook", listener.local_addr().unwrap().port());
    let id = create(&mut engine, json!({"workspace":source,"directory":temp.path().join("case"),"request":"Work","output":temp.path().join("out"),"callback":callback})).unwrap()["id"].as_str().unwrap().to_owned();
    fs::create_dir_all(temp.path().join("case").join("submission")).unwrap();
    engine.change(|state| {
        state.sessions.get_mut(&id).unwrap().deliverable = "submission".into();
        Ok(Value::Null)
    }).unwrap();
    deliver_now(&mut engine, &id).unwrap();
    let started = std::time::Instant::now();
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    assert!(SENDING.lock().unwrap().contains(&id));
    assert_eq!(engine.state.sessions[&id].outbox.len(), 1);
    drop(listener);
}
