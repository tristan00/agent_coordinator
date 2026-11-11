use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use relay_coordinator::{
    engine, external,
    projects::{self, Host},
    store::Store,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tower::ServiceExt;

async fn call(router: &Router, token: &str, method: &str, path: &str, body: Value) -> (u16, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("host", "127.0.0.1:4242")
                .header("content-type", "application/json")
                .header("x-relay-token", token)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 10_000_000).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn pause(host: &Host, id: &str) {
    host.app(id)
        .unwrap()
        .store
        .mutate(|s| {
            s.settings.paused = true;
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn scoped_requests_preserve_isolation_and_project_identity_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let host = Host::open(dir.path(), 4242, String::new()).unwrap();
    pause(&host, "default");
    let token = host.app("default").unwrap().token;
    let router = projects::router(host.clone());
    assert_eq!(
        call(
            &router,
            "wrong",
            "POST",
            "/api/projects",
            json!({"name":"Other"})
        )
        .await
        .0,
        403
    );
    let (status, project) = call(
        &router,
        &token,
        "POST",
        "/api/projects",
        json!({"name":"Other"}),
    )
    .await;
    assert_eq!(status, 200);
    let id = project["id"].as_str().unwrap();
    pause(&host, id);
    let base = project["api_base"].as_str().unwrap();
    assert_eq!(
        call(
            &router,
            &token,
            "POST",
            "/api/projects",
            json!({"name":"other"})
        )
        .await
        .0,
        409
    );
    let (_, task) = call(
        &router,
        &token,
        "POST",
        &format!("{base}/tasks"),
        json!({"title":"Isolated", "acceptance":"Check"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();
    assert_eq!(
        call(
            &router,
            &token,
            "PATCH",
            &format!("/api/projects/default/tasks/{task_id}"),
            json!({"revision":task["revision"],"title":"Wrong board"})
        )
        .await
        .0,
        404
    );
    assert_eq!(
        call(
            &router,
            &token,
            "POST",
            "/api/projects/default/tasks",
            json!({"title":"Wrong dependency","dependencies":[task_id]})
        )
        .await
        .0,
        404
    );
    assert_eq!(
        call(
            &router,
            &token,
            "GET",
            "/api/projects/missing/state",
            Value::Null
        )
        .await
        .0,
        404
    );
    let registration = json!({"source":"client","agent_id":"same-identity","name":"Builder","stages":["implementation"],"lease_seconds":120});
    let (_, a) = call(
        &router,
        &token,
        "POST",
        &format!("{base}/workers/register"),
        registration.clone(),
    )
    .await;
    let (_, b) = call(
        &router,
        &token,
        "POST",
        "/api/projects/default/workers/register",
        registration,
    )
    .await;
    assert_eq!(a["worker_id"], b["worker_id"]);
    assert_eq!(a["next"], format!("{base}/workers/next"));
    assert_eq!(b["next"], "/api/projects/default/workers/next");
    let (_, other) = call(
        &router,
        &token,
        "GET",
        &format!("{base}/state"),
        Value::Null,
    )
    .await;
    let (_, default) = call(&router, &token, "GET", "/api/state", Value::Null).await;
    assert_eq!(other["tasks"].as_array().unwrap().len(), 1);
    assert!(default["tasks"].as_array().unwrap().is_empty());
    assert!(default["artifacts"].as_array().unwrap().is_empty());
    assert_eq!(default["agents"].as_array().unwrap().len(), 1);
    assert_eq!(other["agents"].as_array().unwrap().len(), 1);
    assert_eq!(other["project_id"], id);
    let (_, exported) = call(
        &router,
        &token,
        "GET",
        &format!("{base}/export"),
        Value::Null,
    )
    .await;
    assert_eq!(exported["tasks"][0]["id"], task_id);
    let id = id.to_owned();
    let saved = other["tasks"].clone();
    host.stop();
    drop(router);
    drop(host);
    tokio::time::sleep(Duration::from_millis(30)).await;
    let reopened = Host::open(dir.path(), 4242, String::new()).unwrap();
    assert_eq!(
        serde_json::to_value(reopened.app(&id).unwrap().store.read().unwrap().tasks).unwrap(),
        saved
    );
    assert_eq!(reopened.list().unwrap().len(), 2);
    assert!(
        reopened
            .app(&id)
            .unwrap()
            .store
            .read()
            .unwrap()
            .settings
            .paused
    );
    assert_ne!(reopened.app(&id).unwrap().token, token);
    assert!(Host::open(dir.path(), 4243, String::new()).is_err());
    reopened.stop();
}

#[tokio::test]
async fn schedulers_execute_concurrently_and_host_drain_covers_every_project() {
    let dir = tempfile::tempdir().unwrap();
    let host = Host::open(dir.path(), 4242, String::new()).unwrap();
    let project = host.create("Coordinator").unwrap();
    host.drain(true);
    for id in ["default", project.id.as_str()] {
        host.app(id).unwrap().store.mutate(|s| {
            for stage in ["planning", "implementation", "review"] {
                engine::save_agent(s, None, json!({"name":stage,"stages":[stage],"adapter":"cli","command":env!("CARGO_BIN_EXE_relay-coordinator"),"args":["example-agent","--delay-ms","150"],"timeout_seconds":10}))?;
            }
            engine::create_task(s, json!({"title":"Concurrent pipeline", "acceptance":"Fixture returns stage evidence", "tags":["demo"]}))?;
            Ok(())
        }).unwrap();
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    for id in ["default", project.id.as_str()] {
        assert!(
            host.app(id)
                .unwrap()
                .store
                .read()
                .unwrap()
                .attempts
                .is_empty()
        );
    }
    assert!(host.create("During upgrade").is_err());
    host.drain(false);
    let mut overlap = false;
    let mut complete = false;
    for _ in 0..250 {
        let a = host.app("default").unwrap().store.read().unwrap();
        let b = host.app(&project.id).unwrap().store.read().unwrap();
        overlap |= a.attempts.iter().any(|a| a.status == "running")
            && b.attempts.iter().any(|a| a.status == "running");
        if a.tasks[0].status == "completed" && b.tasks[0].status == "completed" {
            complete = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert!(overlap, "The two projects never executed at the same time");
    assert!(complete, "Automatic pipelines did not finish");
    pause(&host, "default");
    assert!(
        !host
            .app(&project.id)
            .unwrap()
            .store
            .read()
            .unwrap()
            .settings
            .paused
    );
    host.stop();
}

#[test]
fn cross_project_workspace_claims_are_atomic_and_keep_lost_workers_fenced() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    let pool = Default::default();
    let a = Arc::new(Store::open_shared(":memory:", Arc::clone(&pool)).unwrap());
    let b = Arc::new(Store::open_shared(":memory:", pool).unwrap());
    for (db, path) in [(&a, dir.path()), (&b, nested.as_path())] {
        db.mutate(|s| {
            s.settings.paused = true;
            engine::create_task(
                s,
                json!({"title":"Workspace ownership", "acceptance":"Check", "workspace":path}),
            )?;
            engine::reconcile(s)?;
            let t = s.tasks[0].clone();
            engine::action(s, &t.id, json!({"action":"start","revision":t.revision}))?;
            Ok(())
        })
        .unwrap();
    }
    fn claim(db: &Store, key: &str) -> Result<serde_json::Value, u16> {
        db.mutate(|s| external::claim(s, serde_json::from_value(json!({"source":"test","agent_id":key,"name":key,"request_id":key,"task_id":s.tasks[0].id,"stages":["planning"],"lease_seconds":30})).unwrap())).map(|a| serde_json::to_value(a).unwrap()).map_err(|e| e.0)
    }
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = [a.clone(), b.clone()]
        .into_iter()
        .enumerate()
        .map(|(i, db)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                claim(&db, &format!("worker-{i}"))
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    let (owner, other) = if results[0].is_ok() { (a, b) } else { (b, a) };
    owner
        .mutate(|s| {
            s.attempts[0].status = "lost".into();
            Ok(())
        })
        .unwrap();
    let waiting = other.read().unwrap();
    assert_eq!(
        engine::eligible_agent(&waiting, &waiting.tasks[0]).unwrap_err(),
        "Another project is using this workspace"
    );
    assert!(claim(&other, "waiting").is_err());
    owner
        .mutate(|s| {
            s.attempts[0].status = "cancelled".into();
            Ok(())
        })
        .unwrap();
    assert!(claim(&other, "released").is_ok());
}
