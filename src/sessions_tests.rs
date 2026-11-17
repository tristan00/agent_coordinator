use super::*;
use crate::history;
use crate::test_support::Temp;

fn setup(temp: &Temp, planner: &str) -> (Engine, PathBuf) {
    let mut engine = temp.engine();
    let source = temp.dir("config");
    let prompts = crate::workspace_prompts::root();
    let pick = |file: &str| {
        let text = fs::read_to_string(prompts.join(file)).unwrap();
        json!({"candidate":file,"sha256":format!("{:x}", Sha256::digest(text.replace("\r\n", "\n").as_bytes())),"default":true})
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
    assert!(prompt.contains("Build it") && prompt.contains(fs::read_to_string(crate::workspace_prompts::root().join("planner.txt")).unwrap().trim()));
    assert_eq!(request["root"].as_str().map(PathBuf::from), Some(workspace(&source).unwrap()));
    let shown = |path: &Path| path.to_string_lossy().trim_start_matches(r"\\?\").to_owned();
    assert!(prompt.contains(&shown(&workspace(&source).unwrap())));
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    let log = fs::read_to_string(engine.state.sessions[&id].requests[0].run.join("log.txt")).unwrap_or_default();
    assert_eq!(engine.state.sessions[&id].requests[0].exit_code, Some(0), "{log}");
    assert_eq!(engine.state.sessions[&id].status, "running");
    tick(&mut engine).unwrap();
    let session = &engine.state.sessions[&id];
    assert!(session.status == "finished" && session.requests[0].ended.is_some());
    assert_eq!(view(&engine, &id).unwrap()["status"], "finished");
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
    assert_eq!(engine.state.sessions[&id].status, "finished");
    message(&mut engine, &id, json!({"request":"Second"})).unwrap();
    let second: Value = read(&engine.state.sessions[&id].requests[1].run.join("request.json")).unwrap();
    assert!(second["prompt"].as_str().unwrap().contains("Second"));
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
    assert!(prompt.contains("Needs a call") && prompt.contains("a decision is needed"));
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    assert_eq!((engine.state.sessions[&id].status.as_str(), engine.state.sessions[&id].requests.len()), ("finished", 2));
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
    assert!(timed.contains("Sources found for every region") && timed.contains("Layout agreed"), "{timed}");
    assert!(!timed.contains("Not the planner's"), "{timed}");
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    assert_eq!(engine.state.sessions[&id].requests.len(), 2);
    age_turn(&mut engine, &id, 31);
    tick(&mut engine).unwrap();
    let quiet = wake_text(&engine, &id, 3);
    assert!(!quiet.contains("Sources found for every region") && !quiet.contains("Layout agreed"), "{quiet}");
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    finish_task(&mut engine, "Later", "Works");
    tick(&mut engine).unwrap();
    let last = wake_text(&engine, &id, 4);
    assert!(last.contains("Later"), "{last}");
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    assert_eq!((engine.state.sessions[&id].status.as_str(), engine.state.sessions[&id].requests.len()), ("finished", 4));
}

#[test]
fn a_wake_that_fails_is_reported_and_retried_instead_of_swallowed() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "exit 0");
    let case = source.join("case");
    let id = create(&mut engine, json!({"workspace":source,"directory":case,"request":"First"})).unwrap()["id"].as_str().unwrap().to_owned();
    engine.actor = Some(format!("planner:{id}"));
    engine.create(json!({"title":"Only","outcome":"Done","cwd":case,"acceptance":["Works"],"standalone":true})).unwrap();
    engine.actor = None;
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    let key = workspace(&source).unwrap();
    let original = engine.state.config.workspaces[&key].prompts.clone().unwrap();
    engine.change(|state| {
        state.config.workspaces.get_mut(&key).unwrap().prompts.as_mut().unwrap().project_planner.sha256 = "0".repeat(64);
        Ok(Value::Null)
    }).unwrap();
    finish_task(&mut engine, "Only", "Works");
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    let session = &engine.state.sessions[&id];
    assert_eq!((session.status.as_str(), session.requests.len(), session.notified.len()), ("running", 1, 0));
    assert!(session.wake_error.as_deref().unwrap().contains("changed since the workspace selected it"));
    engine.change(|state| {
        state.config.workspaces.get_mut(&key).unwrap().prompts = Some(original);
        Ok(Value::Null)
    }).unwrap();
    tick(&mut engine).unwrap();
    let session = &engine.state.sessions[&id];
    assert_eq!((session.requests.len(), session.notified.len(), session.wake_error.is_none()), (2, 1, true));
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
    assert_eq!((engine.state.sessions[&id].status.as_str(), engine.state.sessions[&id].requests.len()), ("finished", 1));
}

#[test]
fn planner_output_lines_get_the_time_they_arrived() {
    assert_eq!(stamped(b"{\"type\":\"result\"}\r", 7), b"{\"coordinatorAt\":7,\"type\":\"result\"}\n");
    assert_eq!(stamped(b"{}", 7), b"{\"coordinatorAt\":7}\n");
    assert_eq!(stamped(b"plain text", 7), b"plain text\n");
}

#[test]
fn a_task_using_a_lot_of_compute_wakes_the_planner_once() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "exit 0");
    let case = source.join("case");
    let id = create(&mut engine, json!({"workspace":source,"directory":case,"request":"First"})).unwrap()["id"].as_str().unwrap().to_owned();
    engine.actor = Some(format!("planner:{id}"));
    engine.create(json!({"title":"Train the policy","outcome":"Done","cwd":case,"acceptance":["Works"],"standalone":true})).unwrap();
    engine.create(json!({"title":"Light work","outcome":"Done","cwd":case,"acceptance":["Works"],"standalone":true})).unwrap();
    engine.actor = None;
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    let busy = |engine: &mut Engine, title: &str, attempt: &str, seconds: f64| {
        let (title, attempt) = (title.to_owned(), attempt.to_owned());
        write(&engine.attempt_path(&attempt).join("cpu.json"), &json!({"cpuSeconds":seconds,"at":1})).unwrap();
        engine.change(move |state| {
            let t = state.tasks.iter_mut().find(|t| t.title == title).unwrap();
            t.attempts.push(crate::engine::Attempt { id: attempt.clone(), revision: 0, stage: "implementation".into(), started: now(), ended: None, result: None, error: None, evidence: Default::default() });
            t.active = Some(attempt);
            Ok(Value::Null)
        }).unwrap();
    };
    busy(&mut engine, "Light work", "light", 600.0);
    tick(&mut engine).unwrap();
    assert_eq!(engine.state.sessions[&id].requests.len(), 1);
    busy(&mut engine, "Train the policy", "heavy", 10.0 * 3600.0);
    tick(&mut engine).unwrap();
    let session = &engine.state.sessions[&id];
    assert_eq!(session.requests.len(), 2);
    assert!(session.notified.contains("cpu:heavy") && !session.notified.contains("cpu:light"));
    let text = wake_text(&engine, &id, 2);
    assert!(text.contains("Train the policy") && text.contains("10.0") && text.contains("0.2"), "{text}");
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    assert_eq!(engine.state.sessions[&id].requests.len(), 2);
}

#[test]
fn sessions_saved_as_waiting_load_as_finished() {
    let temp = crate::test_support::Temp::new();
    let directory = temp.dir("board-finished");
    let mut state: Value = serde_json::to_value(&temp.engine().state).unwrap();
    state["sessions"] = json!({"s1":{"id":"s1","workspace":temp.dir("ws"),"status":"waiting","requests":[],"created":1},"s2":{"id":"s2","workspace":temp.dir("ws"),"status":"stopped","requests":[],"created":1}});
    crate::write(&directory.join("state.json"), &state).unwrap();
    let engine = Engine::new(directory).unwrap();
    assert_eq!((engine.state.sessions["s1"].status.as_str(), engine.state.sessions["s2"].status.as_str()), ("finished", "stopped"));
}

fn enable_evaluator(engine: &mut Engine, source: &Path) {
    let key = workspace(source).unwrap();
    let text = fs::read_to_string(crate::workspace_prompts::root().join("evaluator.txt")).unwrap();
    let document: crate::workspace_prompts::Document = serde_json::from_value(json!({"candidate":"evaluator.txt","sha256":format!("{:x}", Sha256::digest(text.replace("\r\n", "\n").as_bytes())),"default":true})).unwrap();
    engine.change(move |state| {
        let settings = state.config.workspaces.get_mut(&key).unwrap();
        settings.evaluator = Some("sol_6_1_medium".into());
        settings.prompts.as_mut().unwrap().roles.insert("evaluator".into(), document);
        Ok(Value::Null)
    }).unwrap();
}

fn finish_evaluation_run(engine: &Engine, id: &str, verdicts: Value) {
    let run = engine.state.sessions[id].evaluations.last().unwrap().run.clone();
    write(&run.join("output.json"), &json!({"verdicts":verdicts})).unwrap();
    write(&run.join("exit.json"), &json!({"code":0})).unwrap();
}

#[test]
fn proposed_tasks_are_evaluated_after_each_planner_turn_and_kick_backs_wake_the_planner() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "echo planned");
    enable_evaluator(&mut engine, &source);
    let case = source.join("case");
    let id = create(&mut engine, json!({"workspace":source,"directory":case,"request":"Build the sandbox"})).unwrap()["id"].as_str().unwrap().to_owned();
    assert!(wake_text(&engine, &id, 1).contains("starts as proposed"));
    engine.actor = Some(format!("planner:{id}"));
    let spec = |title: &str| json!({"title":title,"outcome":"It works","cwd":case,"acceptance":["A short check passes"],"standalone":true});
    let good = engine.create(spec("Route graph")).unwrap()["id"].as_str().unwrap().to_owned();
    let costly = engine.create(spec("Twenty minute soak")).unwrap()["id"].as_str().unwrap().to_owned();
    engine.actor = None;
    tick(&mut engine).unwrap();
    assert!(engine.state.sessions[&id].evaluations.is_empty());
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    let evaluation = engine.state.sessions[&id].evaluations[0].clone();
    assert_eq!(evaluation.submitted.len(), 2);
    let request: Value = read(&evaluation.run.join("request.json")).unwrap();
    let prompt = request["prompt"].as_str().unwrap();
    assert!(prompt.contains("Build the sandbox") && prompt.contains("Twenty minute soak") && prompt.contains("Proposed tasks"));
    assert!(request["schema"].is_object() && request["conversation"].is_null());
    let short = engine.state.tasks[engine.task(&good).unwrap()].short_id.clone().unwrap();
    finish_evaluation_run(&engine, &id, json!([{"task":short,"accept":true,"findings":[]},{"task":costly,"accept":false,"findings":["A 20 min run proves no more than a 60 s one"]}]));
    tick(&mut engine).unwrap();
    assert_eq!(engine.state.tasks[engine.task(&good).unwrap()].stage, "implementation");
    assert_eq!(engine.state.tasks[engine.task(&costly).unwrap()].stage, "proposed");
    assert!(engine.state.sessions[&id].evaluations[0].ended.is_some());
    tick(&mut engine).unwrap();
    assert_eq!(engine.state.sessions[&id].requests.len(), 2);
    let wake = wake_text(&engine, &id, 2);
    assert!(wake.contains("kicked these proposed tasks back") && wake.contains("A 20 min run proves no more") && wake.contains("1 of 2 submissions left"));
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    let second = engine.state.sessions[&id].evaluations.last().unwrap();
    assert_eq!(engine.state.sessions[&id].evaluations.len(), 2);
    assert_eq!(second.submitted.iter().map(|s| s.task.as_str()).collect::<Vec<_>>(), [costly.as_str()]);
}

#[test]
fn a_failed_evaluation_is_recorded_and_not_retried_until_the_planner_acts() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "echo planned");
    enable_evaluator(&mut engine, &source);
    let case = source.join("case");
    let id = create(&mut engine, json!({"workspace":source,"directory":case,"request":"Build it"})).unwrap()["id"].as_str().unwrap().to_owned();
    engine.actor = Some(format!("planner:{id}"));
    engine.create(json!({"title":"Task","outcome":"It works","cwd":case,"acceptance":["A check passes"],"standalone":true})).unwrap();
    engine.actor = None;
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    let run = engine.state.sessions[&id].evaluations[0].run.clone();
    write(&run.join("exit.json"), &json!({"code":1,"error":"Claude produced no result event"})).unwrap();
    tick(&mut engine).unwrap();
    for _ in 0..3 {
        tick(&mut engine).unwrap();
    }
    let session = &engine.state.sessions[&id];
    assert_eq!(session.evaluations.len(), 1);
    assert!(session.evaluations[0].error.as_deref().is_some_and(|e| e.contains("no result event")));
    assert_eq!(session.requests.len(), 1);
}

#[test]
fn dependents_wait_for_evaluation_and_a_rejection_does_not_wake_the_planner() {
    let temp = Temp::new();
    let (mut engine, source) = setup(&temp, "echo planned");
    enable_evaluator(&mut engine, &source);
    let case = source.join("case");
    let id = create(&mut engine, json!({"workspace":source,"directory":case,"request":"Build it"})).unwrap()["id"].as_str().unwrap().to_owned();
    engine.actor = Some(format!("planner:{id}"));
    let base = engine.create(json!({"title":"Base","outcome":"It works","cwd":case,"acceptance":["A check passes"],"standalone":true})).unwrap()["id"].as_str().unwrap().to_owned();
    let dependent = engine.create(json!({"title":"Dependent","outcome":"It works","cwd":case,"acceptance":["A check passes"],"standalone":true,"dependencies":[{"target":base,"reason":"uses_api"}]})).unwrap()["id"].as_str().unwrap().to_owned();
    engine.actor = None;
    let index = engine.task(&base).unwrap();
    engine.change(|state| {
        let revision = state.tasks[index].revision;
        for _ in 0..2 {
            state.tasks[index].plan_reviews.push(crate::engine::PlanReview { at: 0, revision, accepted: false, findings: vec!["Too costly".into()] });
        }
        Ok(Value::Null)
    }).unwrap();
    run_planner(&engine, &id);
    tick(&mut engine).unwrap();
    tick(&mut engine).unwrap();
    let submitted: Vec<String> = engine.state.sessions[&id].evaluations[0].submitted.iter().map(|s| s.task.clone()).collect();
    assert_eq!(submitted, [base.clone()]);
    let prompt = read::<Value>(&engine.state.sessions[&id].evaluations[0].run.join("request.json")).unwrap()["prompt"].as_str().unwrap().to_owned();
    assert!(prompt.contains("Dependent [Proposed]"));
    finish_evaluation_run(&engine, &id, json!([{"task":base,"accept":false,"findings":["Still too costly"]}]));
    for _ in 0..4 {
        tick(&mut engine).unwrap();
    }
    assert!(engine.task(&base).is_err() && engine.task(&dependent).is_err());
    let session = &engine.state.sessions[&id];
    assert_eq!((session.requests.len(), session.evaluations.len()), (1, 1));
}
