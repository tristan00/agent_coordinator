use super::*;
use crate::history;
use crate::test_support::Temp;

fn setup(temp: &Temp, planner: &str) -> (Engine, PathBuf) {
    let mut engine = temp.engine();
    let source = temp.dir("config");
    let prompts = crate::workspace_prompts::root();
    let pick = |file: &str| {
        let text = fs::read_to_string(prompts.join(file)).unwrap();
        json!({"candidate":file,"sha256":format!("{:x}", Sha256::digest(text.as_bytes())),"default":true})
    };
    let settings: history::WorkspaceSettings = serde_json::from_value(json!({"planner":"sol_6_1_medium","worker":"sol_6_1_light",
        "prompts":{"project_planner":pick("planner.txt"),"worker_protocol":pick("worker.txt")}})).unwrap();
    let mut state = engine.state.clone();
    state.family = Some(crate::agents::Family::Codex);
    state.config.workspaces.insert(workspace(&source).unwrap(), settings);
    state.config.planner = vec!["cmd.exe".into(), "/d".into(), "/c".into(), planner.into()];
    engine.commit(state).unwrap();
    write(&engine.directory.join("port.json"), &4242).unwrap();
    (engine, source)
}

fn git(directory: &Path, args: &[&str]) -> String {
    let output = command("git").args(args).current_dir(directory).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn names(directory: &Path) -> Vec<String> {
    fs::read_dir(directory).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
}

fn run_planner(engine: &Engine, id: &str) {
    let request = engine.state.sessions[id].requests.last().unwrap().run.clone();
    run(&request).unwrap();
}

#[test]
fn a_session_works_in_its_own_repository_and_waits_when_done() {
    let temp = Temp::new();
    let w = temp.path().join("config").join("case").display().to_string().trim_start_matches(r"\\?\").to_owned();
    let (mut engine, source) = setup(&temp, &format!(r"echo built> {w}\app.txt & cd> {w}\cwd.txt"));
    let case = source.join("case");
    let created = create(&mut engine, json!({"workspace":source,"directory":case,"request":"Build it"})).unwrap();
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["status"], "running");
    assert_eq!(names(&case), [".git"]);
    assert_eq!(git(&case, &["log", "--format=%s"]), "Baseline");
    assert!(!engine.state.config.workspaces.contains_key(&workspace(&case).unwrap()));
    assert_eq!(engine.state.owning_workspace(workspace(&case).unwrap()), workspace(&source).unwrap());
    let request: Value = read(&engine.state.sessions[&id].requests[0].run.join("request.json")).unwrap();
    let prompt = request["prompt"].as_str().unwrap();
    assert!(prompt.contains("Request:\nBuild it") && prompt.contains("Planner instructions:\n") && !prompt.contains("Limits"));
    assert!(!prompt.contains("PROJECT_PLANNER.md") && !prompt.contains("WORKER_PROTOCOL.md"));
    assert_eq!(request["root"].as_str().map(PathBuf::from), Some(workspace(&source).unwrap()));
    let shown = |path: &Path| path.to_string_lossy().trim_start_matches(r"\\?\").to_owned();
    assert!(prompt.contains(&format!("inside the workspace {}, which is your working directory", shown(&workspace(&source).unwrap()))));
    assert!(prompt.contains("The request folder is the result.") && !prompt.contains("/deliver"));
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    let log = fs::read_to_string(engine.state.sessions[&id].requests[0].run.join("log.txt")).unwrap_or_default();
    assert_eq!(engine.state.sessions[&id].requests[0].exit_code, Some(0), "{log}");
    assert_eq!(engine.state.sessions[&id].status, "running");
    tick(&mut engine).unwrap();
    let session = &engine.state.sessions[&id];
    assert!(session.status == "waiting" && session.requests[0].ended.is_some());
    assert_eq!(view(&engine, &id).unwrap()["status"], "waiting");
    assert_eq!(fs::read_to_string(case.join("app.txt")).unwrap().trim(), "built");
    let launched = fs::read_to_string(case.join("cwd.txt")).unwrap();
    assert_eq!(PathBuf::from(launched.trim().trim_start_matches(r"\\?\")).file_name(), source.file_name());
}

#[test]
fn the_session_stays_open_while_tasks_can_still_run_and_follow_ups_resume_it() {
    let temp = Temp::new();
    let w = temp.path().join("config").join("case").display().to_string().trim_start_matches(r"\\?\").to_owned();
    let (mut engine, source) = setup(&temp, &format!(r"mkdir {w}\submission & echo v> {w}\submission\v.txt"));
    let case = source.join("case");
    let id = create(&mut engine, json!({"workspace":source,"directory":case,"request":"First"})).unwrap()["id"].as_str().unwrap().to_owned();
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
fn an_idle_planner_is_resumed_once_with_tasks_that_need_a_decision() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "exit 0");
    let case = source.join("case");
    let id = create(&mut engine, json!({"workspace":source,"directory":case,"request":"First"})).unwrap()["id"].as_str().unwrap().to_owned();
    engine.create(json!({"title":"Needs a call","outcome":"Done","cwd":case,"acceptance":["Works"],"standalone":true})).unwrap();
    engine.change(|state| {
        let task = &mut state.tasks[0];
        task.blocked = true;
        task.failure = "Replanned work failed again; a decision is needed".into();
        Ok(Value::Null)
    }).unwrap();
    assert_eq!(engine.state.tasks[0].status(), "Blocked");
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    let session = &engine.state.sessions[&id];
    assert_eq!((session.status.as_str(), session.requests.len()), ("running", 2));
    let wake: Value = read(&session.requests[1].run.join("request.json")).unwrap();
    let prompt = wake["prompt"].as_str().unwrap();
    assert!(prompt.starts_with("These tasks are blocked and need your decision") && prompt.contains("Needs a call") && prompt.contains("a decision is needed"));
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    assert_eq!((engine.state.sessions[&id].status.as_str(), engine.state.sessions[&id].requests.len()), ("waiting", 2));
}

#[test]
fn existing_folders_are_kept_and_get_their_own_repository_and_stop_ends_the_session() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "exit 0");
    git(&source, &["init", "-q"]);
    let plain = temp.dir("config/plain");
    fs::write(plain.join("notes.txt"), "kept").unwrap();
    let id = create(&mut engine, json!({"workspace":source,"directory":plain,"request":"Work"})).unwrap()["id"].as_str().unwrap().to_owned();
    assert_eq!(PathBuf::from(git(&plain, &["rev-parse", "--show-toplevel"])).canonicalize().unwrap(), plain);
    assert_eq!(git(&plain, &["ls-files"]), "notes.txt");
    stop(&mut engine, &id).unwrap();
    assert_eq!(engine.state.sessions[&id].status, "stopped");
    assert!(engine.state.sessions[&id].requests[0].run.join("stop").exists());
    let existing = temp.dir("config/existing");
    git(&existing, &["init", "-q"]);
    git(&existing, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "Mine"]);
    create(&mut engine, json!({"workspace":source,"directory":existing,"request":"Work"})).unwrap();
    assert_eq!(git(&existing, &["log", "--format=%s"]), "Mine");
}

#[test]
fn requests_are_validated_before_anything_starts() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "exit 0");
    let case = source.join("case");
    assert!(create(&mut engine, json!({"workspace":temp.dir("unknown"),"directory":case,"request":"x"})).is_err());
    assert!(create(&mut engine, json!({"workspace":source,"directory":case,"request":" "})).is_err());
    assert!(create(&mut engine, json!({"workspace":source,"request":"x"})).unwrap_err().to_string().contains("directory is required"));
    assert!(create(&mut engine, json!({"workspace":source,"directory":source,"request":"x"})).unwrap_err().to_string().contains("not the workspace itself"));
    assert!(create(&mut engine, json!({"workspace":source,"directory":temp.path().join("outside"),"request":"x"})).unwrap_err().to_string().contains("inside the workspace"));
    assert!(engine.state.sessions.is_empty() && !source.join(".git").exists());
    create(&mut engine, json!({"workspace":source,"directory":case,"request":"x"})).unwrap();
    assert!(create(&mut engine, json!({"workspace":source,"directory":case,"request":"y"})).unwrap_err().to_string().contains("already running"));
}

fn finish_task(engine: &mut Engine, title: &str, summary: &str) {
    let (title, summary) = (title.to_owned(), summary.to_owned());
    engine.change(move |state| {
        let t = state.tasks.iter_mut().find(|t| t.title == title).unwrap();
        t.attempts.push(crate::engine::Attempt { id: "a".into(), revision: 0, stage: "review".into(), started: 0, ended: Some(1), result: Some(json!({"status":"accepted","summary":summary})), error: None, evidence: Default::default() });
        t.stage = "done".into();
        t.record("phase_changed", "test", json!({"from":"review","to":"done"}));
        Ok(Value::Null)
    }).unwrap();
}

fn age_turn(engine: &mut Engine, id: &str, minutes: u64) {
    engine.change(|state| {
        let request = state.sessions.get_mut(id).unwrap().requests.last_mut().unwrap();
        request.planner_ended = Some(now() - minutes * 60_000);
        Ok(Value::Null)
    }).unwrap();
}

fn wake_text(engine: &Engine, id: &str, number: usize) -> String {
    let request: Value = read(&engine.state.sessions[id].requests[number - 1].run.join("request.json")).unwrap();
    request["prompt"].as_str().unwrap().to_owned()
}

#[test]
fn finished_planner_tasks_are_collected_until_the_timer_or_the_final_check() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "exit 0");
    let case = source.join("case");
    let id = create(&mut engine, json!({"workspace":source,"directory":case,"request":"First"})).unwrap()["id"].as_str().unwrap().to_owned();
    let first: Value = read(&engine.state.sessions[&id].requests[0].run.join("request.json")).unwrap();
    assert!(first["prompt"].as_str().unwrap().contains("every 30 minutes while work runs"));
    engine.actor = Some(format!("planner:{id}"));
    for title in ["Research", "Design", "Later"] {
        engine.create(json!({"title":title,"outcome":"Done","cwd":case,"acceptance":["Works"],"standalone":true})).unwrap();
    }
    engine.actor = None;
    engine.create(json!({"title":"By hand","outcome":"Done","cwd":case,"acceptance":["Works"],"standalone":true})).unwrap();
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    finish_task(&mut engine, "Research", "Sources found for every region");
    finish_task(&mut engine, "By hand", "Not the planner's");
    tick(&mut engine).unwrap();
    assert_eq!(engine.state.sessions[&id].requests.len(), 1);
    finish_task(&mut engine, "Design", "Layout agreed");
    age_turn(&mut engine, &id, 29);
    tick(&mut engine).unwrap();
    assert_eq!(engine.state.sessions[&id].requests.len(), 1);
    age_turn(&mut engine, &id, 30);
    tick(&mut engine).unwrap();
    let timed = wake_text(&engine, &id, 2);
    assert!(timed.starts_with("These tasks finished since your last turn:\n- ") && timed.contains("Sources found for every region") && timed.contains("Layout agreed"), "{timed}");
    assert!(!timed.contains("Not the planner's") && timed.contains("Board: 1 to do, 0 in progress, 0 in review, 0 blocked, 0 stopped, 3 done."), "{timed}");
    assert!(timed.ends_with("every 30 minutes while work runs, and when nothing more can run."), "{timed}");
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    assert_eq!(engine.state.sessions[&id].requests.len(), 2);
    age_turn(&mut engine, &id, 31);
    tick(&mut engine).unwrap();
    let quiet = wake_text(&engine, &id, 3);
    assert!(quiet.starts_with("Nothing has finished since your last turn."), "{quiet}");
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    finish_task(&mut engine, "Later", "Works");
    tick(&mut engine).unwrap();
    let last = wake_text(&engine, &id, 4);
    assert!(last.contains("Later") && last.ends_with("the request finishes when you end this turn unless you add or resume tasks."), "{last}");
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    assert_eq!((engine.state.sessions[&id].status.as_str(), engine.state.sessions[&id].requests.len()), ("waiting", 4));
}

#[test]
fn work_you_stopped_does_not_trigger_the_final_check() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "exit 0");
    let case = source.join("case");
    let id = create(&mut engine, json!({"workspace":source,"directory":case,"request":"First"})).unwrap()["id"].as_str().unwrap().to_owned();
    engine.actor = Some(format!("planner:{id}"));
    engine.create(json!({"title":"Done one","outcome":"Done","cwd":case,"acceptance":["Works"],"standalone":true})).unwrap();
    let halted = engine.create(json!({"title":"Halted","outcome":"Done","cwd":case,"acceptance":["Works"],"standalone":true})).unwrap()["id"].as_str().unwrap().to_owned();
    engine.actor = None;
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    finish_task(&mut engine, "Done one", "Works");
    engine.control(true, Some(&halted)).unwrap();
    tick(&mut engine).unwrap();
    assert_eq!((engine.state.sessions[&id].status.as_str(), engine.state.sessions[&id].requests.len()), ("waiting", 1));
}

#[test]
fn planner_output_lines_get_the_time_they_arrived() {
    assert_eq!(stamped(b"{\"type\":\"result\"}\r", 7), b"{\"coordinatorAt\":7,\"type\":\"result\"}\n");
    assert_eq!(stamped(b"{}", 7), b"{\"coordinatorAt\":7}\n");
    assert_eq!(stamped(b"plain text", 7), b"plain text\n");
}
