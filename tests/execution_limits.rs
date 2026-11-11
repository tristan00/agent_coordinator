use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use relay_coordinator::{
    engine,
    model::*,
    plan,
    projects::{self, Host},
    runner::demo_result,
    workers,
};
use serde_json::json;
use tower::ServiceExt;

fn act(s: &mut Snapshot, task: &str, action: &str, extra: serde_json::Value) {
    let revision = s.tasks.iter().find(|t| t.id == task).unwrap().revision;
    let mut value = json!({"action":action,"revision":revision});
    for (key, value_extra) in extra.as_object().unwrap() {
        value[key] = value_extra.clone();
    }
    engine::action(s, task, value).unwrap();
}

#[test]
fn defaults_allow_retries_and_sendbacks_beyond_old_ceilings() {
    let mut s = Snapshot::default();
    let a = engine::save_agent(
        &mut s,
        None,
        json!({"name":"worker", "adapter":"demo", "stages":["planning","implementation"]}),
    )
    .unwrap();
    assert_eq!(a.timeout_seconds, 0);
    let t = engine::create_task(
        &mut s,
        json!({"title":"Keep working","acceptance":"Deliver it","tags":["demo"]}),
    )
    .unwrap();
    assert_eq!(
        (t.max_attempts, t.max_sendbacks, t.timeout_seconds),
        (0, 0, 0)
    );
    act(&mut s, &t.id, "start", json!({}));
    for expected in 1..=25 {
        let (run, _) = engine::claim(&mut s, Some(&t.id)).unwrap().unwrap();
        assert_eq!(run.number, expected);
        engine::fail(&mut s, &run.id, "A retryable failure", "").unwrap();
        act(&mut s, &t.id, "retry", json!({}));
    }
    for _ in 0..25 {
        let (run, _) = engine::claim(&mut s, Some(&t.id)).unwrap().unwrap();
        engine::finish(&mut s, &run.id, demo_result(&run.input), String::new()).unwrap();
        act(
            &mut s,
            &t.id,
            "send_back",
            json!({"stage":"planning","reason":"Continue the revision"}),
        );
    }
    assert_eq!(s.tasks[0].sendbacks, 25);
    assert!(engine::claim(&mut s, Some(&t.id)).unwrap().is_some());
}

#[test]
fn explicit_configuration_is_not_rejected_by_arbitrary_upper_bounds() {
    let mut s = Snapshot::default();
    engine::save_settings(
        &mut s,
        json!({"concurrency":32,"retry_delay_seconds":7200,"project_name":"w".repeat(500)}),
    )
    .unwrap();
    let agent = engine::save_agent(&mut s, None, json!({"name":"agent".repeat(100),"adapter":"cli","command":"agent","capacity":32,"timeout_seconds":172800,"stages":["planning"],"unset_env":(0..150).map(|n|format!("UNSET_{n}")).collect::<Vec<_>>()})).unwrap();
    assert_eq!(agent.capacity, 32);
    let t = engine::create_task(&mut s, json!({"title":"t".repeat(500),"max_attempts":100,"max_sendbacks":100,"timeout_seconds":172800,"token_budget":0})).unwrap();
    assert_eq!(t.max_attempts, 100);
    workers::register(&mut s, serde_json::from_value(json!({"source":"runtime","agent_id":"long-lease","name":"Worker","stages":["planning"],"lease_seconds":u64::MAX})).unwrap()).unwrap();
    assert!(workers::connected(s.agents.last().unwrap()));
    workers::expire_assignments(&mut s);
}

#[test]
fn imported_tasks_default_to_unlimited_and_keep_planner_binding() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Snapshot::default();
    let request = json!({"plan":{"schema_version":1,"plan_id":"continuous","planner_id":"origin-planner","workspaces":{"app":dir.path()},"tasks":[{"code":"TASK","title":"Work","description":"Work","acceptance":["Done"],"workspace":"app","points":1,"task_type":"feature","priority":2}]}});
    plan::reconcile(&mut s, serde_json::from_value(request).unwrap(), false).unwrap();
    assert_eq!(
        (
            s.tasks[0].max_attempts,
            s.tasks[0].max_sendbacks,
            s.tasks[0].timeout_seconds
        ),
        (0, 0, 0)
    );
    assert_eq!(s.tasks[0].planner_id.as_deref(), Some("origin-planner"));
}

#[tokio::test]
async fn large_artifacts_and_scoped_api_payloads_do_not_hit_old_size_caps() {
    let content = "x".repeat(3_000_000);
    let result: AgentResult = serde_json::from_value(json!({"outcome":"complete","summary":"Large deliverable","artifacts":[{"name":"output","content":content}]})).unwrap();
    engine::validate_result(&result).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let host = Host::open(dir.path(), 4242, String::new()).unwrap();
    host.stop();
    let app = host.app("default").unwrap();
    let response = projects::router(host)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects/default/tasks")
                .header("host", "127.0.0.1:4242")
                .header("x-relay-token", &app.token)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Large task","description":content}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let task: Task = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(task.description.len(), 3_000_000);
}
