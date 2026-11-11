use super::*;

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn board() -> (Fixture, Engine) {
    let root = std::env::current_exe().unwrap().parent().unwrap().join(format!("board-{}", id()));
    fs::create_dir_all(&root).unwrap();
    let engine = Engine::new(root.clone()).unwrap();
    (Fixture(root), engine)
}

fn spec(engine: &Engine, title: &str) -> Value {
    json!({"title":title,"outcome":"Make the change","cwd":engine.directory,"acceptance":["cargo test passes"]})
}

fn add(engine: &mut Engine, title: &str, extra: Value) -> String {
    let mut input = spec(engine, title);
    for (key, value) in extra.as_object().unwrap() {
        input[key] = value.clone();
    }
    engine.create(input).unwrap()["id"].as_str().unwrap().to_owned()
}

fn task<'a>(engine: &'a Engine, id: &str) -> &'a Task {
    &engine.state.tasks[engine.task(id).unwrap()]
}

fn run(engine: &mut Engine, id: &str, result: Value) {
    run_exit(engine, id, Some(result), json!({"code":0}));
}

fn run_exit(engine: &mut Engine, id: &str, result: Option<Value>, exit: Value) {
    let index = engine.task(id).unwrap();
    let attempt = super::id();
    let task = &mut engine.state.tasks[index];
    task.active = Some(attempt.clone());
    task.attempts.push(Attempt { id: attempt.clone(), revision: task.revision, stage: task.stage.clone(), started: now(), ended: None, result: None, error: None, evidence: Default::default() });
    let directory = engine.attempt_path(&attempt);
    fs::create_dir_all(&directory).unwrap();
    if let Some(result) = result {
        write(&directory.join("result.json"), &result).unwrap();
    }
    engine.collect(index, exit).unwrap();
}

fn outcome(status: &str, findings: &[&str]) -> Value {
    json!({"status":status,"summary":"Done","checks":["cargo test"],"findings":findings})
}

fn review(status: &str, criteria: &[&str], findings: &[&str]) -> Value {
    let mut value = outcome(status, findings);
    value["criteria"] = json!(criteria);
    value
}

#[test]
fn definitions_are_bounded_and_coordination_is_typed() {
    let (_fixture, mut engine) = board();
    let base = spec(&engine, "Bounded task");
    let reject = |engine: &mut Engine, change: Value| {
        let mut input = base.clone();
        for (key, value) in change.as_object().unwrap() {
            input[key] = value.clone();
        }
        engine.create(input).unwrap_err().to_string()
    };
    assert!(reject(&mut engine, json!({"acceptance":["a","b","c","d"]})).contains("At most 3"));
    assert!(reject(&mut engine, json!({"outcome":"word ".repeat(120)})).contains("120 words"));
    assert!(reject(&mut engine, json!({"title":"t ".repeat(13)})).contains("12 words"));
    assert!(reject(&mut engine, json!({"outcome":"Reuse 1794382095482-30692-58 output"})).contains("relations"));
    assert!(reject(&mut engine, json!({"outcome":format!("Use {}", "ab".repeat(32))})).contains("relations"));
    assert!(reject(&mut engine, json!({"outcome":"Follow AGENTS.md limits"})).contains("relations"));
    assert!(reject(&mut engine, json!({"refinements":["more prose"]})).contains("typed fields"));
    assert!(reject(&mut engine, json!({"attributes":{"tags":["x"]}})).contains("Unknown attribute"));
    assert!(reject(&mut engine, json!({"attributes":{"lane":"design"}})).contains("unknown variant"));
    assert!(reject(&mut engine, json!({"attributes":{"needs":["browser"]}})).contains("No worker provides"));
    assert!(reject(&mut engine, json!({"attributes":{"size":"large"}})).contains("container"));
    assert!(reject(&mut engine, json!({"attributes":{"deploy":true}})).contains("candidate"));
    assert!(reject(&mut engine, json!({"dependencies":["missing"]})).contains("not found"));
    let id = engine.create(base).unwrap()["id"].as_str().unwrap().to_owned();
    assert_eq!(task(&engine, &id).attributes.lane, Lane::Code);
}

#[test]
fn lifecycle_runs_implementation_review_done() {
    let (_fixture, mut engine) = board();
    let id = add(&mut engine, "Simple", json!({}));
    run(&mut engine, &id, outcome("completed", &[]));
    assert_eq!(task(&engine, &id).stage, "review");
    run(&mut engine, &id, review("accepted", &["passed"], &[]));
    assert_eq!(task(&engine, &id).status(), "Done");
}

#[test]
fn accepted_review_with_unpassed_criterion_is_corrected() {
    let (_fixture, mut engine) = board();
    let id = add(&mut engine, "Strict", json!({}));
    run(&mut engine, &id, outcome("completed", &[]));
    run(&mut engine, &id, review("accepted", &["failed"], &["Edge case fails"]));
    assert_eq!(task(&engine, &id).stage, "implementation");
    assert!(task(&engine, &id).feedback[0].starts_with("Failed criterion"));
}

#[test]
fn repeated_failure_of_one_criterion_triggers_replan() {
    let (_fixture, mut engine) = board();
    let id = add(&mut engine, "Strikes", json!({"acceptance":["first check","second check"]}));
    run(&mut engine, &id, outcome("completed", &[]));
    run(&mut engine, &id, review("rejected", &["failed", "passed"], &["Fails for unicode"]));
    assert_eq!(task(&engine, &id).stage, "implementation");
    run(&mut engine, &id, outcome("completed", &[]));
    run(&mut engine, &id, review("rejected", &["passed", "failed"], &["Second fails"]));
    assert_eq!(task(&engine, &id).stage, "implementation");
    run(&mut engine, &id, outcome("completed", &[]));
    run(&mut engine, &id, review("rejected", &["failed", "passed"], &["Fails for emoji"]));
    assert_eq!(task(&engine, &id).stage, "replan");
}

#[test]
fn blocked_work_replans_into_ordered_children_under_a_container() {
    let (_fixture, mut engine) = board();
    let id = add(&mut engine, "Too broad", json!({}));
    let dependent = add(&mut engine, "Downstream", json!({"dependencies":[id]}));
    run(&mut engine, &id, outcome("blocked", &["Needs two separate changes"]));
    assert_eq!(task(&engine, &id).stage, "replan");
    let child = |title: &str, after: Value| json!({"title":title,"outcome":"Do one part","acceptance":["test passes"],"lane":"code","needs":[],"size":"small","artifact":"none","after":after});
    let plan = json!({"title":"Too broad","outcome":"Both parts","acceptance":[],"lane":"code","needs":[],"size":"large","children":[child("Part one", json!([])), child("Part two", json!([0]))]});
    let mut replanned = outcome("replanned", &[]);
    replanned["plan"] = plan;
    run(&mut engine, &id, replanned);
    assert!(task(&engine, &id).container());
    let children: Vec<String> = engine.state.children(&id).map(|t| t.id.clone()).collect();
    assert_eq!(children.len(), 2);
    assert_eq!(task(&engine, &children[1]).after, vec![children[0].clone()]);
    assert!(engine.state.waiting_reason(task(&engine, &children[0])).is_none());
    assert!(engine.state.waiting_reason(task(&engine, &children[1])).unwrap().contains("settle"));
    assert!(engine.state.waiting_reason(task(&engine, &dependent)).unwrap().contains("Too broad"));
    for child in &children {
        run(&mut engine, child, outcome("completed", &[]));
        run(&mut engine, child, review("accepted", &["passed"], &[]));
    }
    let mut state = engine.state.clone();
    assert!(state.settle_containers());
    engine.commit(state).unwrap();
    assert_eq!(task(&engine, &id).stage, "done");
    assert!(engine.state.waiting_reason(task(&engine, &dependent)).is_none());
}

#[test]
fn failed_replans_hold_for_a_decision_until_resumed() {
    let (_fixture, mut engine) = board();
    let id = add(&mut engine, "Stuck", json!({}));
    run(&mut engine, &id, outcome("blocked", &["Unclear"]));
    let unchanged = json!({"title":"Stuck","outcome":"Make the change","acceptance":["cargo test passes"],"lane":"code","needs":[],"size":"small","children":[]});
    let mut replanned = outcome("replanned", &[]);
    replanned["plan"] = unchanged;
    run(&mut engine, &id, replanned.clone());
    assert!(engine.state.tasks[0].attempts.last().unwrap().result.as_ref().unwrap()["planError"].is_string());
    assert!(!task(&engine, &id).blocked);
    run(&mut engine, &id, replanned);
    assert!(task(&engine, &id).blocked);
    assert_eq!(engine.notices.last().unwrap()["event"], "held");
    engine.control_as(false, Some(&id), Actor::Human, "Decision made").unwrap();
    assert!(!task(&engine, &id).blocked);
    assert!(engine.state.queued(task(&engine, &id)));
}

#[test]
fn held_result_needs_a_finding_and_stops_dispatch() {
    let (_fixture, mut engine) = board();
    let id = add(&mut engine, "Decision", json!({}));
    run(&mut engine, &id, outcome("blocked", &["Choose a storage engine"]));
    run(&mut engine, &id, outcome("held", &[]));
    assert!(task(&engine, &id).attempts.last().unwrap().error.as_deref().unwrap().contains("findings"));
    run(&mut engine, &id, outcome("held", &["Pick SQLite or Postgres"]));
    assert!(task(&engine, &id).blocked);
}

#[test]
fn usage_limit_holds_the_whole_board_without_retries() {
    let (_fixture, mut engine) = board();
    let id = add(&mut engine, "Limited", json!({}));
    let other = add(&mut engine, "Other", json!({}));
    let index = engine.task(&id).unwrap();
    let attempt = super::id();
    engine.state.tasks[index].active = Some(attempt.clone());
    engine.state.tasks[index].attempts.push(Attempt { id: attempt.clone(), revision: 0, stage: "implementation".into(), started: now(), ended: None, result: None, error: None, evidence: Default::default() });
    let directory = engine.attempt_path(&attempt);
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("log.txt"), "ERROR: You've hit your usage limit. Visit x or try again at Nov 16th, 2099 3:06 PM.").unwrap();
    engine.collect(index, json!({"code":1})).unwrap();
    assert!(engine.state.hold_until > now());
    assert_eq!(task(&engine, &id).retry_at, 0);
    assert!(engine.state.waiting_reason(task(&engine, &other)).unwrap().starts_with("Usage limit"));
}

#[test]
fn worker_errors_back_off_and_outputs_are_clipped() {
    let (_fixture, mut engine) = board();
    let id = add(&mut engine, "Flaky", json!({}));
    run_exit(&mut engine, &id, None, json!({"code":2}));
    assert!(task(&engine, &id).retry_at > 0);
    let mut long = outcome("completed", &["a ".repeat(80).as_str(), "b", "c", "d"]);
    long["summary"] = json!("w ".repeat(90));
    run(&mut engine, &id, long);
    let result = task(&engine, &id).attempts.last().unwrap().result.clone().unwrap();
    assert_eq!(history::words(result["summary"].as_str().unwrap()), history::SUMMARY_WORDS);
    assert_eq!(result["findings"].as_array().unwrap().len(), history::MAX_FINDINGS);
    assert_eq!(history::words(result["findings"][0].as_str().unwrap()), history::FINDING_WORDS);
}

#[test]
fn relations_gate_dispatch_in_code() {
    let (_fixture, mut engine) = board();
    let first = add(&mut engine, "First", json!({"writable":["shared"]}));
    let ordered = add(&mut engine, "Ordered", json!({"after":[first]}));
    let conflicting = add(&mut engine, "Conflicting", json!({"conflicts":[first]}));
    let overlapping = add(&mut engine, "Overlapping", json!({"writable":["shared/file.txt"]}));
    let audit = add(&mut engine, "Audit", json!({"writable":["shared"],"attributes":{"lane":"audit"}}));
    let index = engine.task(&first).unwrap();
    engine.state.tasks[index].active = Some("running".into());
    let reason = |engine: &Engine, id: &str| engine.state.waiting_reason(task(engine, id)).unwrap_or_default();
    assert!(reason(&engine, &ordered).contains("settle"));
    assert!(reason(&engine, &conflicting).contains("Conflicts"));
    assert!(reason(&engine, &overlapping).contains("writable scope"));
    assert_eq!(reason(&engine, &audit), "");
    add(&mut engine, "Later", json!({"after":[ordered]}));
    let mut edit = json!({"after":[ordered]});
    edit["actor"] = json!("planner");
    assert!(engine.edit(&first, edit).unwrap_err().to_string().contains("Stop the task"));
}

#[test]
fn priority_orders_dispatch() {
    let (_fixture, mut engine) = board();
    add(&mut engine, "Low", json!({"attributes":{"priority":3}}));
    add(&mut engine, "High", json!({"attributes":{"priority":0}}));
    let mut order: Vec<_> = engine.state.tasks.iter().collect();
    order.sort_by_key(|t| t.attributes.priority());
    assert_eq!(order[0].title, "High");
}

#[test]
fn children_require_container_and_cannot_wait_on_it() {
    let (_fixture, mut engine) = board();
    let leaf = add(&mut engine, "Leaf", json!({}));
    let directory = engine.directory.clone();
    assert!(engine.create(json!({"title":"Child","outcome":"x","cwd":directory,"acceptance":["y"],"parent":leaf})).unwrap_err().to_string().contains("container"));
    let container = add(&mut engine, "Epic", json!({"acceptance":[],"attributes":{"size":"large"}}));
    assert!(engine.state.waiting_reason(task(&engine, &container)).unwrap().contains("no child"));
    assert!(engine.create(json!({"title":"Child","outcome":"x","cwd":directory,"acceptance":["y"],"parent":container,"dependencies":[container]})).is_err());
    let child = add(&mut engine, "Child", json!({"parent":container}));
    assert_eq!(engine.task_value(task(&engine, &container), false).unwrap()["children"], json!([child]));
    assert!(engine.remove(&container, Actor::Planner).is_err());
}

#[test]
fn corrective_edits_reopen_dependents_and_supersede_rewires_them() {
    let (_fixture, mut engine) = board();
    let base = add(&mut engine, "Base", json!({}));
    let next = add(&mut engine, "Next", json!({"dependencies":[base]}));
    for id in [&base, &next] {
        run(&mut engine, id, outcome("completed", &[]));
        run(&mut engine, id, review("accepted", &["passed"], &[]));
    }
    engine.edit(&base, json!({"outcome":"  Make   the change "})).unwrap();
    assert_eq!(task(&engine, &next).stage, "done");
    engine.edit(&base, json!({"outcome":"Make a different change"})).unwrap();
    assert_eq!(task(&engine, &base).stage, "implementation");
    assert_eq!(task(&engine, &next).stage, "implementation");
    let replacement = add(&mut engine, "Replacement", json!({"supersedes":[base]}));
    assert_eq!(task(&engine, &next).dependencies, vec![replacement]);
    assert_eq!(engine.state.archived.last().unwrap().history.last().unwrap().kind, "superseded");
}

#[test]
fn board_control_needs_a_human_and_default_actor_is_planner() {
    let (_fixture, mut engine) = board();
    let id = add(&mut engine, "Task", json!({}));
    assert_eq!(Actor::api(&json!({})).unwrap(), Actor::Planner);
    assert!(engine.control_as(true, None, Actor::Planner, "stop all").is_err());
    engine.control_as(true, Some(&id), Actor::Human, "user stop").unwrap();
    assert!(engine.control_as(false, Some(&id), Actor::Planner, "resume").is_err());
    engine.control_as(false, Some(&id), Actor::Human, "user resume").unwrap();
    engine.control_as(true, None, Actor::Human, "stop all").unwrap();
    assert!(engine.state.paused);
}

#[test]
fn legacy_tasks_migrate_into_replan() {
    let (fixture, mut engine) = board();
    let id = add(&mut engine, "Legacy", json!({}));
    let mut raw: Value = read(&fixture.0.join("state.json")).unwrap();
    raw["tasks"][0]["refinements"] = json!(["Old appended prose"]);
    raw["tasks"][0]["audit"] = json!(true);
    raw["tasks"][0]["attributes"] = json!({"kind":"bug","tags":["x"],"priority":1,"originatingRequest":"words","links":[]});
    write(&fixture.0.join("state.json"), &raw).unwrap();
    let engine = Engine::new(fixture.0.clone()).unwrap();
    let task = task(&engine, &id);
    assert_eq!(task.stage, "replan");
    assert_eq!(task.attributes.lane, Lane::Audit);
    assert_eq!(task.attributes.priority, Some(1));
}

#[test]
fn usage_reset_parses_local_time() {
    assert!(crate::telemetry::usage_reset("You've hit your usage limit. Visit x or try again at Nov 16th, 2026 3:06 PM.").unwrap().is_some_and(|ms| ms > 1_700_000_000_000));
    assert_eq!(crate::telemetry::usage_reset("You've hit your usage limit."), Some(None));
    assert_eq!(crate::telemetry::usage_reset("all good"), None);
}
