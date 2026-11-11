use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use relay_coordinator::{
    api::{self, App},
    engine::*,
    model::*,
    runner::{Runner, demo_result, parse_result},
    store::*,
};
use serde_json::{Value, json};
use std::sync::{Arc, Barrier};
use tower::ServiceExt;

fn database() -> Store {
    Store::open(":memory:").unwrap()
}
fn make_task(store: &Store) -> Task {
    store.mutate(|s|create_task(s,json!({"title":"Implement the feature","acceptance":"The acceptance checks pass.","tags":["demo"]}))).unwrap()
}

#[test]
fn task_keys_remain_unique_when_earlier_records_have_been_removed() {
    let mut s = Snapshot::default();
    let first = create_task(&mut s, json!({"title":"First"})).unwrap();
    let second = create_task(&mut s, json!({"title":"Second"})).unwrap();
    s.tasks.retain(|t| t.id != first.id);
    let third = create_task(&mut s, json!({"title":"Third"})).unwrap();
    assert_eq!(second.key, "RL-002");
    assert_eq!(third.key, "RL-003");
}
fn setup() -> (Store, Task) {
    let store = database();
    store
        .mutate(|s| {
            for (name, stages) in [
                ("Planner", vec!["planning"]),
                ("Builder", vec!["implementation"]),
                ("Reviewer", vec!["review"]),
            ] {
                save_agent(
                    s,
                    None,
                    json!({"name":name,"adapter":"demo","stages":stages}),
                )?;
            }
            Ok(())
        })
        .unwrap();
    let task = make_task(&store);
    (store, task)
}
fn act(store: &Store, task: &str, action_name: &str, extra: Value) -> AppResult<Task> {
    let t = store
        .read()?
        .tasks
        .into_iter()
        .find(|t| t.id == task)
        .unwrap();
    let mut v = json!({"action":action_name,"revision":t.revision});
    for (k, value) in extra.as_object().unwrap() {
        v[k] = value.clone();
    }
    store.mutate(|s| action(s, task, v))
}
fn run(store: &Store, task: &str) -> Attempt {
    store.mutate(|s| claim(s, Some(task))).unwrap().unwrap().0
}
fn complete(store: &Store, a: &Attempt) {
    store
        .mutate(|s| finish(s, &a.id, demo_result(&a.input), String::new()))
        .unwrap();
}

#[test]
fn full_pipeline_preserves_each_stage_and_model() {
    let (store, t) = setup();
    act(&store, &t.id, "start", json!({})).unwrap();
    for stage in EXECUTABLE_STAGES.iter() {
        let a = run(&store, &t.id);
        assert_eq!(&a.stage, stage);
        complete(&store, &a);
    }
    let s = store.read().unwrap();
    assert_eq!(s.tasks[0].status, "completed");
    assert_eq!(s.attempts.len(), 3);
    assert_eq!(s.artifacts.len(), 3);
    assert!(
        s.attempts
            .iter()
            .all(|a| a.duration_ms.is_some() && a.model == "deterministic-demo")
    );
}

#[test]
fn sendback_creates_new_visit_and_preserves_superseded_artifacts() {
    let (store, t) = setup();
    act(&store, &t.id, "start", json!({})).unwrap();
    let plan = run(&store, &t.id);
    complete(&store, &plan);
    let build = run(&store, &t.id);
    let mut result = demo_result(&build.input);
    result.outcome = "send_back".into();
    result.send_back_to = Some("planning".into());
    result.reason = "Plan omitted migration of existing rows".into();
    store
        .mutate(|s| finish(s, &build.id, result, String::new()))
        .unwrap();
    let s = store.read().unwrap();
    assert_eq!(s.tasks[0].stage, "planning");
    assert_eq!(s.tasks[0].sendbacks, 1);
    assert_eq!(s.tasks[0].visit, 3);
    assert!(s.artifacts.iter().all(|a| a.stale));
    let retry_plan = run(&store, &t.id);
    assert_eq!(retry_plan.number, 1);
    assert!(
        retry_plan.input["feedback"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "sendback")
    );
    complete(&store, &retry_plan);
    for _ in 0..2 {
        let a = run(&store, &t.id);
        complete(&store, &a);
    }
    assert_eq!(store.read().unwrap().tasks[0].stage, "done");
}

#[test]
fn duplicate_completion_is_idempotent() {
    let (store, t) = setup();
    act(&store, &t.id, "start", json!({})).unwrap();
    let a = run(&store, &t.id);
    complete(&store, &a);
    let before = store.read().unwrap();
    complete(&store, &a);
    let after = store.read().unwrap();
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.artifacts.len(), after.artifacts.len());
    assert_eq!(after.tasks[0].stage, "implementation");
}

#[test]
fn paused_attempt_cannot_publish_late_results() {
    let (store, t) = setup();
    act(&store, &t.id, "start", json!({})).unwrap();
    let a = run(&store, &t.id);
    act(&store, &t.id, "pause", json!({})).unwrap();
    complete(&store, &a);
    let s = store.read().unwrap();
    assert_eq!(s.tasks[0].status, "paused");
    assert_eq!(s.tasks[0].stage, "planning");
    assert!(s.artifacts.is_empty());
    assert_eq!(s.attempts[0].status, "cancelled");
}

#[test]
fn retry_increments_attempt_without_changing_visit() {
    let (store, t) = setup();
    act(&store, &t.id, "start", json!({})).unwrap();
    let a = run(&store, &t.id);
    store
        .mutate(|s| fail(s, &a.id, "Worker crashed", ""))
        .unwrap();
    act(&store, &t.id, "retry", json!({})).unwrap();
    let b = run(&store, &t.id);
    assert_eq!(b.visit, a.visit);
    assert_eq!(b.number, 2);
}

#[test]
fn retry_budget_is_enforced() {
    let (store, t) = setup();
    store
        .mutate(|s| update_task(s, &t.id, json!({"revision":t.revision,"max_attempts":1})))
        .unwrap();
    act(&store, &t.id, "start", json!({})).unwrap();
    let a = run(&store, &t.id);
    store.mutate(|s| fail(s, &a.id, "fail", "")).unwrap();
    assert!(act(&store, &t.id, "retry", json!({})).is_err());
}

#[test]
fn sendback_budget_is_enforced_atomically() {
    let (store, t) = setup();
    store
        .mutate(|s| update_task(s, &t.id, json!({"revision":t.revision,"max_sendbacks":1})))
        .unwrap();
    act(&store, &t.id, "start", json!({})).unwrap();
    let a = run(&store, &t.id);
    complete(&store, &a);
    act(
        &store,
        &t.id,
        "send_back",
        json!({"stage":"planning","reason":"First revision"}),
    )
    .unwrap();
    let a = run(&store, &t.id);
    complete(&store, &a);
    let revision = store.read().unwrap().revision;
    assert!(
        act(
            &store,
            &t.id,
            "send_back",
            json!({"stage":"planning","reason":"Missing detail"})
        )
        .is_err()
    );
    assert_eq!(store.read().unwrap().revision, revision);
}

#[test]
fn stage_skipping_and_empty_evidence_are_rejected() {
    let (store, t) = setup();
    act(&store, &t.id, "start", json!({})).unwrap();
    assert!(act(&store, &t.id, "complete_stage", json!({"reason":""})).is_err());
    assert!(
        act(
            &store,
            &t.id,
            "send_back",
            json!({"stage":"done","reason":"skip"})
        )
        .is_err()
    );
    assert_eq!(store.read().unwrap().tasks[0].stage, "planning");
}

#[test]
fn stale_edits_return_conflict() {
    let (store, t) = setup();
    store
        .mutate(|s| update_task(s, &t.id, json!({"revision":t.revision,"title":"New title"})))
        .unwrap();
    let error = store
        .mutate(|s| {
            update_task(
                s,
                &t.id,
                json!({"revision":t.revision,"title":"Lost update"}),
            )
        })
        .unwrap_err();
    assert_eq!(error.0, 409);
    assert_eq!(store.read().unwrap().tasks[0].title, "New title");
}

#[test]
fn changed_requirements_replan_and_invalidate_approvals() {
    let (store, t) = setup();
    act(&store, &t.id, "start", json!({})).unwrap();
    let a = run(&store, &t.id);
    complete(&store, &a);
    let current = store.read().unwrap().tasks[0].clone();
    store
        .mutate(|s| {
            update_task(
                s,
                &t.id,
                json!({"revision":current.revision,"acceptance":"New acceptance criteria"}),
            )
        })
        .unwrap();
    let s = store.read().unwrap();
    assert_eq!(s.tasks[0].stage, "planning");
    assert!(s.artifacts[0].stale);
    assert_eq!(s.tasks[0].visit, 3);
}

#[test]
fn dependencies_block_claims_until_completed() {
    let (store, t) = setup();
    let dependency = make_task(&store);
    store
        .mutate(|s| {
            update_task(
                s,
                &t.id,
                json!({"revision":t.revision,"dependencies":[dependency.id]}),
            )
        })
        .unwrap();
    act(&store, &t.id, "start", json!({})).unwrap();
    assert!(store.mutate(|s| claim(s, Some(&t.id))).is_err());
    act(&store, &dependency.id, "start", json!({})).unwrap();
    for _ in 0..3 {
        let a = run(&store, &dependency.id);
        complete(&store, &a);
    }
    assert!(store.mutate(|s| claim(s, Some(&t.id))).unwrap().is_some());
}

#[test]
fn dependency_cycles_are_rejected() {
    let store = database();
    let a = make_task(&store);
    let b = make_task(&store);
    store
        .mutate(|s| {
            update_task(
                s,
                &a.id,
                json!({"revision":a.revision,"dependencies":[b.id]}),
            )
        })
        .unwrap();
    assert!(
        store
            .mutate(|s| update_task(
                s,
                &b.id,
                json!({"revision":b.revision,"dependencies":[a.id]})
            ))
            .is_err()
    );
}

#[test]
fn hierarchy_rules_and_parent_completion_are_enforced() {
    let store = database();
    let epic = store
        .mutate(|s| create_task(s, json!({"title":"Epic","kind":"epic"})))
        .unwrap();
    let child = store
        .mutate(|s| create_task(s, json!({"title":"Child","parent_id":epic.id})))
        .unwrap();
    assert!(
        act(
            &store,
            &epic.id,
            "complete_stage",
            json!({"reason":"All done"})
        )
        .is_err()
    );
    assert!(
        store
            .mutate(|s| create_task(
                s,
                json!({"title":"Bad child","kind":"subtask","parent_id":epic.id})
            ))
            .is_err()
    );
    assert!(
        store
            .mutate(|s| create_task(
                s,
                json!({"title":"Good child","kind":"subtask","parent_id":child.id})
            ))
            .is_ok()
    );
}

#[test]
fn restart_preserves_data_and_marks_inflight_failed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay.db");
    let store = Store::open(&path).unwrap();
    store
        .mutate(|s| {
            s.settings.paused = true;
            seed_demo(s)
        })
        .unwrap();
    let t = store
        .read()
        .unwrap()
        .tasks
        .iter()
        .find(|t| t.stage == "planning")
        .unwrap()
        .clone();
    let a = run(&store, &t.id);
    drop(store);
    let restored = Store::open(&path).unwrap();
    assert_eq!(restored.mutate(recover).unwrap(), 1);
    let s = restored.read().unwrap();
    assert_eq!(
        s.attempts.iter().find(|r| r.id == a.id).unwrap().status,
        "failed"
    );
    assert!(s.settings.paused);
    assert_eq!(
        s.tasks.iter().find(|x| x.id == t.id).unwrap().stage,
        "planning"
    );
}

#[test]
fn racing_claims_only_start_one_attempt() {
    let (store, t) = setup();
    act(&store, &t.id, "start", json!({})).unwrap();
    let store = Arc::new(store);
    let barrier = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let store = store.clone();
            let barrier = barrier.clone();
            let tid = t.id.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store
                    .mutate(|s| claim(s, Some(&tid)))
                    .ok()
                    .flatten()
                    .is_some()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter(|b| *b)
            .count(),
        1
    );
    assert_eq!(store.read().unwrap().attempts.len(), 1);
}

#[test]
fn unknown_usage_remains_unknown() {
    let (store, t) = setup();
    act(&store, &t.id, "start", json!({})).unwrap();
    let a = run(&store, &t.id);
    complete(&store, &a);
    let usage = store.read().unwrap().attempts[0].usage.clone();
    assert_eq!(usage.input_tokens, None);
    assert_eq!(usage.output_tokens, None);
    assert_eq!(usage.cost_usd, None);
    assert_eq!(usage.source, "unavailable");
}

#[test]
fn reported_token_budget_stops_future_attempts() {
    let (store, t) = setup();
    store
        .mutate(|s| update_task(s, &t.id, json!({"revision":t.revision,"token_budget":100})))
        .unwrap();
    act(&store, &t.id, "start", json!({})).unwrap();
    let a = run(&store, &t.id);
    let mut result = demo_result(&a.input);
    result.usage = Usage {
        input_tokens: Some(80),
        output_tokens: Some(30),
        cost_usd: None,
        source: "reported".into(),
    };
    store
        .mutate(|s| finish(s, &a.id, result, String::new()))
        .unwrap();
    assert!(
        store
            .mutate(|s| claim(s, Some(&t.id)))
            .unwrap_err()
            .1
            .contains("budget")
    );
}

#[test]
fn one_workspace_is_serialized_even_for_different_agents() {
    let (store, a) = setup();
    let b = make_task(&store);
    let dir = tempfile::tempdir().unwrap();
    for t in [&a, &b] {
        store
            .mutate(|s| {
                update_task(
                    s,
                    &t.id,
                    json!({"revision":t.revision,"workspace":dir.path()}),
                )
            })
            .unwrap();
        act(&store, &t.id, "start", json!({})).unwrap();
    }
    run(&store, &a.id);
    let error = store.mutate(|s| claim(s, Some(&b.id))).unwrap_err();
    assert!(error.1.contains("workspace"));
}

#[test]
fn independent_review_rejects_same_agent_profile() {
    let store = database();
    store
        .mutate(|s| {
            save_agent(
                s,
                None,
                json!({"name":"Generalist","adapter":"demo","stages":EXECUTABLE_STAGES}),
            )?;
            Ok(())
        })
        .unwrap();
    let t = make_task(&store);
    act(&store, &t.id, "start", json!({})).unwrap();
    for _ in 0..2 {
        let a = run(&store, &t.id);
        complete(&store, &a);
    }
    assert!(store.mutate(|s| claim(s, Some(&t.id))).is_err());
}

#[test]
fn demo_agents_cannot_claim_real_tasks() {
    let (store, _) = setup();
    let t = store
        .mutate(|s| create_task(s, json!({"title":"Real task","acceptance":"Real evidence"})))
        .unwrap();
    act(&store, &t.id, "start", json!({})).unwrap();
    assert!(store.mutate(|s| claim(s, Some(&t.id))).is_err());
}

#[test]
fn backup_reopens_with_artifacts_and_history() {
    let (store, t) = setup();
    act(&store, &t.id, "start", json!({})).unwrap();
    let a = run(&store, &t.id);
    complete(&store, &a);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("backup.db");
    store.backup(&path).unwrap();
    let restored = Store::open(path).unwrap().read().unwrap();
    assert_eq!(restored.tasks[0].id, t.id);
    assert_eq!(restored.artifacts.len(), 1);
    assert_eq!(restored.events.len(), store.read().unwrap().events.len());
}

#[test]
fn malformed_agent_results_cannot_advance_tasks() {
    assert!(parse_result("Not JSON").is_err());
    assert!(parse_result(r#"{"outcome":"complete","summary":"done"}"#).is_err());
    assert!(parse_result(r#"{"outcome":"send_back","summary":"bad","reason":""}"#).is_err());
}

#[test]
fn planning_agent_can_reestimate_complexity_without_erasing_initial_estimate() {
    let (store, t) = setup();
    act(&store, &t.id, "start", json!({})).unwrap();
    let a = run(&store, &t.id);
    let mut result = demo_result(&a.input);
    result.estimate = Some(EstimateProposal {
        points: 8,
        confidence: "medium".into(),
        reason: "Touches three subsystems and needs migration validation".into(),
    });
    store
        .mutate(|s| finish(s, &a.id, result, String::new()))
        .unwrap();
    let s = store.read().unwrap();
    assert_eq!(s.tasks[0].complexity, 8);
    assert_eq!(s.tasks[0].estimates.len(), 2);
    assert_eq!(s.tasks[0].estimates[0].points, 3);
    assert!(s.events.iter().any(|e| e.kind == "estimated"));
}

#[test]
fn failure_rolls_back_all_artifact_mutations() {
    let (store, t) = setup();
    act(&store, &t.id, "start", json!({})).unwrap();
    let a = run(&store, &t.id);
    let mut result = demo_result(&a.input);
    result.outcome = "send_back".into();
    result.send_back_to = Some("backlog".into());
    result.reason = "bad".into();
    let before = serde_json::to_value(store.read().unwrap()).unwrap();
    assert!(
        store
            .mutate(|s| finish(s, &a.id, result, String::new()))
            .is_err()
    );
    assert_eq!(serde_json::to_value(store.read().unwrap()).unwrap(), before);
}

#[test]
fn reopening_upstream_invalidates_completed_dependents() {
    let (store, a) = setup();
    let b = make_task(&store);
    store
        .mutate(|s| {
            update_task(
                s,
                &b.id,
                json!({"revision":b.revision,"dependencies":[a.id]}),
            )
        })
        .unwrap();
    for t in [&a, &b] {
        act(&store, &t.id, "start", json!({})).unwrap();
        for _ in 0..3 {
            let run = run(&store, &t.id);
            complete(&store, &run);
        }
    }
    act(
        &store,
        &a.id,
        "send_back",
        json!({"stage":"planning","reason":"Requirements gap"}),
    )
    .unwrap();
    let s = store.read().unwrap();
    let b = s.tasks.iter().find(|t| t.id == b.id).unwrap();
    assert_eq!(b.stage, "planning");
    assert!(
        s.artifacts
            .iter()
            .filter(|x| x.task_id == b.id)
            .all(|a| a.stale)
    );
    assert!(eligible_agent(&s, b).is_err());
}

#[test]
fn indirect_hierarchy_dependency_cycle_is_rejected() {
    let store = database();
    let epic = store
        .mutate(|s| create_task(s, json!({"title":"Epic","kind":"epic"})))
        .unwrap();
    let child = store
        .mutate(|s| create_task(s, json!({"title":"Child","parent_id":epic.id})))
        .unwrap();
    let external = make_task(&store);
    store
        .mutate(|s| {
            update_task(
                s,
                &external.id,
                json!({"revision":external.revision,"dependencies":[epic.id]}),
            )
        })
        .unwrap();
    assert!(
        store
            .mutate(|s| update_task(
                s,
                &child.id,
                json!({"revision":child.revision,"dependencies":[external.id]})
            ))
            .is_err()
    );
}

#[test]
fn completed_task_can_be_archived_and_restored_without_losing_completion() {
    let (store, t) = setup();
    act(&store, &t.id, "start", json!({})).unwrap();
    for _ in 0..3 {
        let a = run(&store, &t.id);
        complete(&store, &a);
    }
    act(&store, &t.id, "archive", json!({"reason":"Finished"})).unwrap();
    assert!(store.read().unwrap().tasks[0].archived);
    act(&store, &t.id, "restore", json!({})).unwrap();
    assert_eq!(store.read().unwrap().tasks[0].status, "completed");
    assert!(!store.read().unwrap().tasks[0].archived);
}

#[tokio::test]
async fn cancellation_terminates_cli_and_releases_slot() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(database());
    let agent=store.mutate(|s|save_agent(s,None,json!({"name":"Slow CLI","adapter":"cli","command":env!("CARGO_BIN_EXE_relay-coordinator"),"args":["example-agent","--delay-ms","10000"],"stages":["planning"]}))).unwrap();
    let t = make_task(&store);
    act(&store, &t.id, "start", json!({})).unwrap();
    let (a, _) = store.mutate(|s| claim(s, Some(&t.id))).unwrap().unwrap();
    let runner = Runner::new(store.clone(), dir.path().to_path_buf());
    runner.launch(a, agent);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    act(&store, &t.id, "pause", json!({})).unwrap();
    for _ in 0..50 {
        if store.read().unwrap().attempts[0].status == "cancelled" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let s = store.read().unwrap();
    assert_eq!(s.attempts[0].status, "cancelled");
    assert_eq!(s.tasks[0].status, "paused");
    assert!(s.artifacts.is_empty());
    assert!(
        !dir.path()
            .join("runs")
            .join(&s.attempts[0].id)
            .join("result.json")
            .exists()
    );
}

#[tokio::test]
async fn timeout_fails_local_execution() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(database());
    let agent=store.mutate(|s|save_agent(s,None,json!({"name":"Timeout CLI","adapter":"cli","command":env!("CARGO_BIN_EXE_relay-coordinator"),"args":["example-agent","--delay-ms","10000"],"stages":["planning"],"timeout_seconds":5}))).unwrap();
    let t = make_task(&store);
    act(&store, &t.id, "start", json!({})).unwrap();
    let (a, _) = store.mutate(|s| claim(s, Some(&t.id))).unwrap().unwrap();
    let runner = Runner::new(store.clone(), dir.path().to_path_buf());
    runner.launch(a, agent);
    for _ in 0..80 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if store.read().unwrap().attempts[0].status != "running" {
            break;
        }
    }
    let s = store.read().unwrap();
    assert_eq!(s.attempts[0].status, "failed");
    assert!(s.attempts[0].error.contains("exceeded"));
    assert!(s.attempts[0].duration_ms.unwrap() >= 4_900);
    assert!(s.artifacts.is_empty());
}

#[tokio::test]
async fn ollama_adapter_normalizes_reported_usage() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mock=axum::Router::new().route("/api/chat",axum::routing::post(|axum::Json(input):axum::Json<Value>|async move{assert_eq!(input["stream"],false);assert!(input["messages"][0]["content"].as_str().unwrap().contains("cannot inspect"));axum::Json(json!({"model":"local-test-model","prompt_eval_count":42,"eval_count":17,"message":{"content":json!({"outcome":"complete","summary":"Text plan","artifacts":[{"name":"plan.md","content":"A text plan"}]}).to_string()}}))}));
    let mock_server = tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(database());
    let agent=store.mutate(|s|save_agent(s,None,json!({"name":"Local text model","adapter":"ollama","model":"local-test-model","endpoint":format!("http://{addr}"),"stages":["planning"]}))).unwrap();
    let task = make_task(&store);
    act(&store, &task.id, "start", json!({})).unwrap();
    let (a, _) = store.mutate(|s| claim(s, Some(&task.id))).unwrap().unwrap();
    Runner::new(store.clone(), dir.path().to_path_buf()).launch(a, agent);
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if store.read().unwrap().attempts[0].status != "running" {
            break;
        }
    }
    let s = store.read().unwrap();
    assert_eq!(s.attempts[0].status, "completed", "{:?}", s.attempts[0]);
    assert_eq!(s.attempts[0].usage.input_tokens, Some(42));
    assert_eq!(s.attempts[0].usage.output_tokens, Some(17));
    assert_eq!(s.attempts[0].model, "local-test-model");
    mock_server.abort();
}

fn test_app() -> (axum::Router, Arc<Store>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(database());
    let runner = Runner::new(store.clone(), dir.path().to_path_buf());
    (
        api::router(App {
            store: store.clone(),
            runner,
            token: "test-token".into(),
            approval_code: "test-approval".into(),
            port: 4242,
        }),
        store,
        dir,
    )
}
fn request(method: &str, path: &str, body: Value) -> axum::http::request::Builder {
    let _ = body;
    Request::builder()
        .method(method)
        .uri(path)
        .header("Host", "127.0.0.1:4242")
        .header("Content-Type", "application/json")
}

#[tokio::test]
async fn approval_requires_operator_authority_and_survives_reopen_of_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay.sqlite");
    let store = Arc::new(Store::open(&path).unwrap());
    let (gate, dependent) = store
        .mutate(|s| {
            let gate = create_task(
                s,
                json!({"title":"Approve plan", "human_approval": true,
            "acceptance":"Operator approved the requirements"}),
            )?;
            let dependent = create_task(
                s,
                json!({"title":"Implement", "acceptance":"Verified",
            "dependencies":[gate.id], "tags":["demo"]}),
            )?;
            save_agent(
                s,
                None,
                json!({"name":"Test", "adapter":"demo", "stages":["planning"]}),
            )?;
            s.settings.paused = false;
            Ok((gate, dependent))
        })
        .unwrap();
    let app = api::router(App {
        store: store.clone(),
        runner: Runner::new(store.clone(), dir.path().to_path_buf()),
        token: "test-token".into(),
        approval_code: "operator-code".into(),
        port: 4242,
    });
    let action_path = format!("/api/tasks/{}/action", gate.id);
    for action in ["start", "retry", "complete_stage", "approve"] {
        let payload =
            json!({"revision": gate.revision, "action": action, "reason":"Test evidence"});
        let response = app
            .clone()
            .oneshot(
                request("POST", &action_path, Value::Null)
                    .header("x-relay-token", "test-token")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if action == "approve" {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::CONFLICT
            }
        );
    }
    let response = app
        .clone()
        .oneshot(
            request("POST", &format!("/api/tasks/{}/run", gate.id), Value::Null)
                .header("x-relay-token", "test-token")
                .body(Body::from(json!({"revision":gate.revision}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    act(&store, &dependent.id, "start", json!({})).unwrap();
    assert!(store.mutate(|s| claim(s, None)).unwrap().is_none());
    assert_eq!(store.read().unwrap().attempts.len(), 0);

    let payload = json!({"revision":gate.revision,"action":"approve","reason":"Operator accepted requirements"});
    let response = app
        .clone()
        .oneshot(
            request("POST", &action_path, Value::Null)
                .header("x-relay-token", "test-token")
                .header("x-relay-approval", "operator-code")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let reopened = Store::open(&path).unwrap();
    let snapshot = reopened.read().unwrap();
    assert_eq!(
        snapshot.tasks[0].approval.as_ref().unwrap().reason,
        "Operator accepted requirements"
    );
    assert_eq!(snapshot.tasks[0].stage, "done");
    assert!(
        snapshot
            .events
            .iter()
            .any(|event| event.kind == "approved" && event.actor == "human")
    );
    let attempt = run(&reopened, &dependent.id);
    complete(&reopened, &attempt);

    let payload = json!({"revision":snapshot.tasks[0].revision,"action":"reopen_approval","reason":"Requirements changed"});
    let response = app
        .oneshot(
            request("POST", &action_path, Value::Null)
                .header("x-relay-token", "test-token")
                .header("x-relay-approval", "operator-code")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = reopened.read().unwrap();
    assert!(snapshot.tasks[0].approval.is_none());
    assert_eq!(snapshot.tasks[0].stage, "backlog");
    assert_eq!(snapshot.tasks[1].stage, "planning");
    assert!(snapshot.artifacts.iter().all(|artifact| artifact.stale));
    assert_eq!(snapshot.attempts[0].status, "completed");
}

#[tokio::test]
async fn api_requires_local_token_and_rejects_cross_origin() {
    let (app, _, _dir) = test_app();
    let response = app
        .clone()
        .oneshot(
            request("POST", "/api/tasks", json!({}))
                .body(Body::from(r#"{"title":"No token"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .oneshot(
            request("POST", "/api/tasks", json!({}))
                .header("x-relay-token", "test-token")
                .header("Origin", "http://evil.example")
                .body(Body::from(r#"{"title":"Cross origin"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_rejects_dns_rebinding_host() {
    let (app, _, _dir) = test_app();
    let r = app
        .oneshot(
            Request::builder()
                .uri("/api/state")
                .header("Host", "evil.example:4242")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_create_task_and_get_board_state() {
    let (app, _, _dir) = test_app();
    let response = app
        .clone()
        .oneshot(
            request("POST", "/api/tasks", json!({}))
                .header("x-relay-token", "test-token")
                .body(Body::from(r#"{"title":"API task","acceptance":"Works"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(
            request("GET", "/api/state", json!({}))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = to_bytes(response.into_body(), 2_000_000).await.unwrap();
    let state: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(state["tasks"][0]["title"], "API task");
}

#[tokio::test]
async fn actual_cli_process_returns_artifact_and_persists_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(database());
    let inherited_name = std::env::vars_os()
        .find(|(_, value)| !value.is_empty())
        .unwrap()
        .0
        .to_string_lossy()
        .into_owned();
    let agent=store.mutate(|s|save_agent(s,None,json!({"name":"CLI protocol test","adapter":"cli","command":env!("CARGO_BIN_EXE_relay-coordinator"),"args":["example-agent","--delay-ms","2500","--assert-env-absent",inherited_name],"unset_env":[inherited_name],"instructions":"unique instruction αβ", "stages":["planning"]}))).unwrap();
    let task = make_task(&store);
    act(&store, &task.id, "start", json!({})).unwrap();
    let (a, _) = store.mutate(|s| claim(s, Some(&task.id))).unwrap().unwrap();
    let runner = Runner::new(store.clone(), dir.path().to_path_buf());
    runner.launch(a.clone(), agent);
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if store.read().unwrap().attempts[0].status != "running" {
            break;
        }
    }
    let state = store.read().unwrap();
    assert_eq!(
        state.attempts[0].status, "completed",
        "{:?}",
        state.attempts[0]
    );
    assert_eq!(state.tasks[0].stage, "implementation");
    assert!(state.attempts[0].duration_ms.unwrap() >= 2_400);
    let run_dir = dir.path().join("runs").join(&a.id);
    let stdout = std::fs::read_to_string(run_dir.join("stdout.log")).unwrap();
    assert!(serde_json::from_str::<Value>(&stdout).is_ok());
    let prompt = std::fs::read_to_string(run_dir.join("prompt.md")).unwrap();
    assert_eq!(prompt.matches("unique instruction αβ").count(), 1);
    assert!(
        dir.path()
            .join("runs")
            .join(a.id)
            .join("input.json")
            .exists()
    );
}
