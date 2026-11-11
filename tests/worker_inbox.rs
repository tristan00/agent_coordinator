use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use relay_coordinator::{
    api::{self, App},
    engine::*,
    model::*,
    runner::Runner,
    store::Store,
    workers,
};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
use tower::ServiceExt;

fn register(s: &mut Snapshot, id: &str, stages: &[&str]) -> Agent {
    workers::register(s,serde_json::from_value(json!({"source":"test-runtime","agent_id":id,"name":id,"stages":stages,"lease_seconds":30})).unwrap()).unwrap()
}
fn task(s: &mut Snapshot, title: &str) -> Task {
    create_task(
        s,
        json!({"title":title,"acceptance":"Checked fixture output"}),
    )
    .unwrap()
}
fn result() -> AgentResult {
    serde_json::from_value(json!({"outcome":"complete","summary":"Fixture checked","artifacts":[{"name":"evidence","content":"Test fixture output"}]})).unwrap()
}
async fn post(app: axum::Router, path: &str, body: Value) -> Value {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("host", "127.0.0.1:4242")
                .header("x-relay-token", "test-token")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 2_000_000).await.unwrap()).unwrap();
    assert!(status.is_success(), "{status}: {value}");
    value
}

#[tokio::test]
async fn registered_workers_receive_the_whole_pipeline_without_per_task_claims() {
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(temp.path().join("relay.sqlite")).unwrap());
    let (first, dependent) = store
        .mutate(|s| {
            s.settings.retry_delay_seconds = 0;
            let first = task(s, "First");
            let dependent = create_task(
                s,
                json!({"title":"Dependent","acceptance":"Checked","dependencies":[first.id]}),
            )?;
            Ok((first.id, dependent.id))
        })
        .unwrap();
    let runner = Runner::new(store.clone(), temp.path().to_owned());
    let router = api::router(App {
        store: store.clone(),
        runner: runner.clone(),
        token: "test-token".into(),
        approval_code: String::new(),
        port: 4242,
    });
    // Registration happens once. Only Relay selects tasks and role handoffs.
    let mut workers = Vec::new();
    for stage in ["planning", "implementation", "review"] {
        let response = post(
            router.clone(),
            "/api/workers/register",
            json!({"source":"test-runtime","agent_id":stage,"name":stage,"stages":[stage]}),
        )
        .await;
        workers.push((stage, response["worker_id"].as_str().unwrap().to_string()));
    }
    runner.start_scheduler();
    let mut handles = Vec::new();
    for (stage, worker_id) in workers {
        let router = router.clone();
        handles.push(tokio::spawn(async move {
            let mut rejected = false;
            let mut failed = false;
            loop {
                let response = post(
                    router.clone(),
                    "/api/workers/next",
                    json!({"worker_id":worker_id,"wait_seconds":1}),
                )
                .await;
                if response["attempt"].is_null() {
                    continue;
                }
                let a = &response["attempt"];
                assert_eq!(a["stage"], stage);
                assert_eq!(response["next_action"], "execute");
                let mut output = result();
                if stage == "planning" && !failed {
                    failed = true;
                    output.outcome = "failed".into();
                    output.reason = "Transient provider failure in test".into();
                }
                if stage == "review" && !rejected {
                    rejected = true;
                    output.outcome = "send_back".into();
                    output.send_back_to = Some("implementation".into());
                    output.reason = "Test one automatic revision".into();
                }
                post(
                    router.clone(),
                    &format!("/api/external/{}/result", a["id"].as_str().unwrap()),
                    serde_json::to_value(output).unwrap(),
                )
                .await;
            }
        }));
    }
    let finished = tokio::time::timeout(std::time::Duration::from_secs(12), async {
        loop {
            if store
                .read()
                .unwrap()
                .tasks
                .iter()
                .all(|t| t.status == "completed")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await;
    runner.stopping.store(true, Ordering::Relaxed);
    for handle in handles {
        handle.abort();
    }
    assert!(finished.is_ok(), "Background assignments failed to finish");
    let s = store.read().unwrap();
    assert_eq!(s.attempts.len(), 12);
    let repair = s
        .tasks
        .iter()
        .find(|t| t.recovery_of.as_ref().is_some_and(|r| r.task_id == first))
        .unwrap();
    assert_eq!(repair.sendbacks, 1);
    assert_eq!(s.tasks.iter().find(|t| t.id == first).unwrap().sendbacks, 0);
    assert_eq!(
        s.tasks.iter().find(|t| t.id == dependent).unwrap().status,
        "completed"
    );
    assert_eq!(dispatch_status(&s)["mode"], "idle");
    assert!(!s.events.iter().any(|e| e.kind == "human_complete"));
}

#[test]
fn an_unstarted_offer_survives_restart_and_delivery_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("relay.sqlite");
    let db = Store::open(&path).unwrap();
    let (worker, offer) = db
        .mutate(|s| {
            task(s, "Durable handoff");
            let worker = register(s, "builder", &["planning"]);
            reconcile(s)?;
            assert_eq!(workers::dispatch(s)?, 1);
            assert!(s.attempts.is_empty());
            Ok((
                worker.id,
                s.agents[0]
                    .worker
                    .as_ref()
                    .unwrap()
                    .assignment
                    .as_ref()
                    .unwrap()
                    .id
                    .clone(),
            ))
        })
        .unwrap();
    drop(db);
    let db = Store::open(path).unwrap();
    assert_eq!(db.mutate(recover).unwrap(), 0);
    let run = db
        .mutate(|s| workers::receive(s, &worker))
        .unwrap()
        .unwrap();
    assert_eq!(run.external.as_ref().unwrap().request_id, offer);
    assert_eq!(
        db.mutate(|s| workers::receive(s, &worker))
            .unwrap()
            .unwrap()
            .id,
        run.id
    );
    assert_eq!(db.read().unwrap().attempts.len(), 1);
}

#[test]
fn disconnected_workers_release_unstarted_work_without_consuming_attempts() {
    let mut s = Snapshot::default();
    task(&mut s, "Reassignment");
    let first = register(&mut s, "offline", &["planning"]);
    let second = register(&mut s, "online", &["planning"]);
    reconcile(&mut s).unwrap();
    workers::dispatch(&mut s).unwrap();
    assert!(s.agents[0].worker.as_ref().unwrap().assignment.is_some());
    s.agents[0].worker.as_mut().unwrap().seen_at = "2000-01-01T00:00:00Z".into();
    workers::dispatch(&mut s).unwrap();
    assert!(workers::receive(&mut s, &first.id).unwrap().is_none());
    let run = workers::receive(&mut s, &second.id).unwrap().unwrap();
    assert_eq!(run.number, 1);
    assert_eq!(s.attempts.len(), 1);
}

#[test]
fn pause_and_requirement_changes_prevent_delivery_of_old_work() {
    let mut s = Snapshot::default();
    let t = task(&mut s, "Mutable requirements");
    let w = register(&mut s, "worker", &["planning"]);
    reconcile(&mut s).unwrap();
    workers::dispatch(&mut s).unwrap();
    s.settings.paused = true;
    assert!(workers::receive(&mut s, &w.id).unwrap().is_none());
    assert!(s.attempts.is_empty());
    let revision = s.tasks[0].revision;
    update_task(
        &mut s,
        &t.id,
        json!({"revision":revision,"acceptance":"Changed acceptance"}),
    )
    .unwrap();
    s.settings.paused = false;
    assert!(workers::receive(&mut s, &w.id).unwrap().is_none());
    workers::dispatch(&mut s).unwrap();
    let run = workers::receive(&mut s, &w.id).unwrap().unwrap();
    assert_eq!(run.input["task"]["acceptance"], "Changed acceptance");
}

#[test]
fn reservations_prevent_conflicting_workspaces_and_release_on_disconnect() {
    let temp = tempfile::tempdir().unwrap();
    let mut s = Snapshot::default();
    for name in ["One", "Two"] {
        create_task(
            &mut s,
            json!({"title":name,"acceptance":"Checked","workspace":temp.path()}),
        )
        .unwrap();
    }
    let first = register(&mut s, "first", &["planning"]);
    let second = register(&mut s, "second", &["planning"]);
    reconcile(&mut s).unwrap();
    assert_eq!(workers::dispatch(&mut s).unwrap(), 1);
    assert_eq!(
        s.agents
            .iter()
            .filter(|a| a.worker.as_ref().unwrap().assignment.is_some())
            .count(),
        1
    );
    workers::disconnect(&mut s, &first.id).unwrap();
    assert_eq!(workers::dispatch(&mut s).unwrap(), 1);
    assert!(workers::receive(&mut s, &second.id).unwrap().is_some());
}

#[test]
fn blocked_tasks_do_not_stop_other_worker_assignments() {
    let mut s = Snapshot::default();
    let gate = create_task(&mut s, json!({"title":"Decision","human_approval":true})).unwrap();
    create_task(
        &mut s,
        json!({"title":"Blocked","acceptance":"Checked","dependencies":[gate.id]}),
    )
    .unwrap();
    let ready = task(&mut s, "Independent");
    let w = register(&mut s, "worker", &["planning"]);
    reconcile(&mut s).unwrap();
    workers::dispatch(&mut s).unwrap();
    assert_eq!(
        workers::receive(&mut s, &w.id).unwrap().unwrap().task_id,
        ready.id
    );
    assert!(!s.settings.paused);
}
