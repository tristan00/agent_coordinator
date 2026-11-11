use relay_coordinator::{callbacks, engine, external, model::*, store::Store};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

fn registration(source: &str, agent_id: &str, endpoint: Option<String>) -> callbacks::Registration {
    serde_json::from_value(json!({"source":source,"agent_id":agent_id,"name":"Originating planner","context":{"conversation":"original-planning-session"},"callback_url":endpoint,"workspace_default":true})).unwrap()
}

fn block(s: &mut Snapshot) -> Attempt {
    engine::create_task(
        s,
        json!({"title":"Recover actual work","acceptance":"Deliver the requested change"}),
    )
    .unwrap();
    let attempt = external::claim(s, serde_json::from_value(json!({"source":"test-worker","agent_id":"builder","name":"Builder","stages":["planning"],"request_id":id()})).unwrap()).unwrap().unwrap();
    engine::finish(s, &attempt.id, serde_json::from_value(json!({"outcome":"blocked","summary":"Cannot access dependency","reason":"Registry connection failed"})).unwrap(), String::new()).unwrap();
    callbacks::notify(s, 0, &attempt.id, "Registry connection failed");
    attempt
}

#[test]
fn callback_is_durable_deduplicated_and_receipt_does_not_resolve_work() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("relay.sqlite");
    let store = Store::open(&db).unwrap();
    let (planner, attempt, notification_id) = store
        .mutate(|s| {
            let planner = callbacks::register(s, registration("cursor", "origin", None))?;
            let attempt = block(s);
            for _ in 0..4 {
                callbacks::notify(s, 0, &attempt.id, "Registry connection failed");
            }
            assert_eq!(s.planner_notifications.len(), 1);
            assert_eq!(
                s.planner_notifications[0].payload["planner"]["context"]["conversation"],
                "original-planning-session"
            );
            Ok((planner, attempt, s.planner_notifications[0].id.clone()))
        })
        .unwrap();
    drop(store);
    let store = Store::open(&db).unwrap();
    store
        .mutate(|s| {
            assert_eq!(s.planners.len(), 1);
            let first = callbacks::next(s, &planner.id)?.unwrap();
            let repeated = callbacks::next(s, &planner.id)?.unwrap();
            assert_eq!(first.id, repeated.id);
            assert_eq!(first.id, notification_id);
            assert_eq!(first.attempt_id, attempt.id);
            assert_eq!(
                callbacks::acknowledge(s, "different-planner", &first.id)
                    .unwrap_err()
                    .0,
                404
            );
            let before = serde_json::to_value(&s.tasks).unwrap();
            callbacks::acknowledge(s, &planner.id, &first.id)?;
            callbacks::acknowledge(s, &planner.id, &first.id)?;
            assert!(callbacks::next(s, &planner.id)?.is_none());
            assert_eq!(serde_json::to_value(&s.tasks).unwrap(), before);
            assert_eq!(
                s.events
                    .iter()
                    .filter(|e| e.kind == "planner_callback_acknowledged")
                    .count(),
                1
            );
            Ok(())
        })
        .unwrap();
    let other = Store::open(dir.path().join("other.sqlite")).unwrap();
    assert_eq!(
        other
            .mutate(|s| callbacks::next(s, &planner.id))
            .unwrap_err()
            .0,
        404
    );
}

#[test]
fn missing_receiver_is_visible_and_late_registration_backfills_the_failure() {
    let mut s = Snapshot::default();
    let attempt = block(&mut s);
    callbacks::notify(&mut s, 0, &attempt.id, "same failure");
    assert!(s.planner_notifications.is_empty());
    assert_eq!(
        s.events
            .iter()
            .filter(|e| e.kind == "planner_callback_unrouted")
            .count(),
        1
    );
    let planner = callbacks::register(&mut s, registration("claude", "planner", None)).unwrap();
    assert_eq!(s.planner_notifications.len(), 1);
    assert_eq!(s.planner_notifications[0].planner_id, planner.id);
    let same = callbacks::register(&mut s, registration("claude", "planner", None)).unwrap();
    assert_eq!(planner.id, same.id);
    assert_eq!(s.planner_notifications.len(), 1);
}

#[test]
fn child_and_recovery_notifications_return_to_the_original_planner() {
    let mut s = Snapshot::default();
    let default = callbacks::register(&mut s, registration("local", "default", None)).unwrap();
    let mut explicit = registration("other-runtime", "original", None);
    explicit.workspace_default = false;
    let planner = callbacks::register(&mut s, explicit).unwrap();
    let parent = engine::create_task(
        &mut s,
        json!({"kind":"epic","title":"Original project","planner_id":planner.id}),
    )
    .unwrap();
    let child =
        engine::create_task(&mut s, json!({"title":"Child task","parent_id":parent.id})).unwrap();
    let recovery = engine::create_task(&mut s, json!({"title":"Repair task"})).unwrap();
    s.tasks[2].recovery_of = Some(RecoveryIncident {
        task_id: child.id.clone(),
        attempt_id: "original-attempt".into(),
        stage: "implementation".into(),
        resumed: false,
    });
    callbacks::notify(&mut s, 2, "repair-attempt", "Repair needs a different tool");
    assert_eq!(s.planner_notifications[0].planner_id, planner.id);
    assert_ne!(s.planner_notifications[0].planner_id, default.id);
    assert_eq!(s.planner_notifications[0].origin_task_id, child.id);
    assert_eq!(s.planner_notifications[0].task_id, recovery.id);
    assert_eq!(
        s.planner_notifications[0].payload["recovery_task_ids"],
        json!([recovery.id])
    );
}

#[test]
fn acknowledged_unresolved_incident_is_followed_up_without_bypassing_pause() {
    let mut s = Snapshot::default();
    let planner = callbacks::register(&mut s, registration("local", "planner", None)).unwrap();
    block(&mut s);
    let original = s.planner_notifications[0].id.clone();
    callbacks::acknowledge(&mut s, &planner.id, &original).unwrap();
    s.planner_notifications[0].acknowledged_at = Some("2000-01-01T00:00:00Z".into());
    s.settings.paused = true;
    callbacks::remind_unresolved(&mut s);
    assert_eq!(s.planner_notifications.len(), 1);
    s.settings.paused = false;
    s.tasks[0].current_attempt = Some(s.attempts[0].id.clone());
    s.attempts[0].status = "running".into();
    callbacks::remind_unresolved(&mut s);
    assert_eq!(s.planner_notifications.len(), 1);
    s.tasks[0].current_attempt = None;
    s.attempts[0].status = "blocked".into();
    callbacks::remind_unresolved(&mut s);
    callbacks::remind_unresolved(&mut s);
    assert_eq!(s.planner_notifications.len(), 2);
    assert_eq!(
        s.planner_notifications[1].payload["kind"],
        "recovery_follow_up"
    );
    assert_eq!(
        s.planner_notifications[1].payload["previous_notification_id"],
        original
    );
    assert!(s.tasks.iter().all(|t| t.status != "completed"));
}

#[test]
fn pull_delivery_is_fair_and_late_registration_recovers_missing_routes() {
    let mut s = Snapshot::default();
    let task = engine::create_task(
        &mut s,
        json!({"title":"No configured runtime","acceptance":"Finish work"}),
    )
    .unwrap();
    callbacks::notify(
        &mut s,
        0,
        "routing:planning:0",
        "No enabled planning runtime",
    );
    let planner = callbacks::register(&mut s, registration("local", "planner", None)).unwrap();
    assert_eq!(s.planner_notifications.len(), 1);
    assert_eq!(s.planner_notifications[0].task_id, task.id);
    assert_eq!(
        s.planner_notifications[0].reason,
        "No enabled planning runtime"
    );
    callbacks::notify(&mut s, 0, "different-incident", "Another failure");
    let first = callbacks::next(&mut s, &planner.id).unwrap().unwrap();
    let second = callbacks::next(&mut s, &planner.id).unwrap().unwrap();
    assert_ne!(first.id, second.id);
    assert!(
        s.planner_notifications
            .iter()
            .all(|n| n.acknowledged_at.is_none())
    );
}

async fn wait_for(store: &Store, condition: impl Fn(&Snapshot) -> bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if condition(&store.read().unwrap()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("callback delivery did not progress");
}

#[tokio::test]
async fn unreachable_webhook_retries_across_restart_and_http_success_requires_receipt() {
    use axum::{Json, Router, extract::State, routing::post};
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("relay.sqlite");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener); // Start with an unreachable receiver.
    let store = Arc::new(Store::open(&db).unwrap());
    store
        .mutate(|s| {
            callbacks::register(
                s,
                registration("any-provider", "origin", Some(format!("http://{address}/"))),
            )?;
            block(s);
            // Callback delivery is not coupled to workspace execution admissions.
            s.settings.paused = true;
            Ok(())
        })
        .unwrap();
    let first_stop = Arc::new(AtomicBool::new(false));
    callbacks::start_dispatcher(store.clone(), first_stop.clone());
    wait_for(&store, |s| {
        !s.planner_notifications[0].last_error.is_empty()
    })
    .await;
    let notification_id = store.read().unwrap().planner_notifications[0].id.clone();
    first_stop.store(true, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(350)).await;
    drop(store);

    let received = Arc::new(Mutex::new(Vec::<Value>::new()));
    let router = Router::new().route("/", post(|State(received):State<Arc<Mutex<Vec<Value>>>>, Json(value):Json<Value>| async move {
        let mut values = received.lock().unwrap();
        values.push(value.clone());
        if values.len() == 1 { Json(json!({"ok":true})) } // Not an acknowledgement.
        else { Json(json!({"acknowledged":value["notification_id"]})) }
    })).with_state(received.clone());
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let store = Arc::new(Store::open(&db).unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    callbacks::start_dispatcher(store.clone(), stop.clone());
    wait_for(&store, |s| {
        s.planner_notifications[0].acknowledged_at.is_some()
    })
    .await;
    stop.store(true, Ordering::Relaxed);
    let s = store.read().unwrap();
    assert_eq!(s.planner_notifications.len(), 1);
    assert_eq!(s.planner_notifications[0].id, notification_id);
    assert!(s.planner_notifications[0].delivery_attempts >= 3);
    assert_eq!(s.tasks[0].status, "blocked");
    let deliveries = received.lock().unwrap();
    assert_eq!(deliveries.len(), 2);
    assert!(
        deliveries
            .iter()
            .all(|v| v["notification_id"] == notification_id)
    );
    assert!(
        deliveries
            .iter()
            .all(|v| v.get("token").is_none() && v.get("approval_code").is_none())
    );
    server.abort();
}
