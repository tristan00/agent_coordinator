use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use relay_coordinator::{
    engine,
    model::{self, Artifact, Attempt, Settings, Usage},
    projects::{self, Host},
    store::Store,
};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn call(router: &Router, token: &str, path: &str, body: Value) -> (u16, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
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
    (
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn reset_requires_confirmation_and_quiescence_then_backs_up_only_target_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let host = Host::open(dir.path(), 4242, String::new()).unwrap();
    let project = host.create("Target board").unwrap();
    host.stop();
    let token = host.app("default").unwrap().token;
    let router = projects::router(host.clone());
    let path = format!("{}/reset", project.api_base);
    let confirmation = json!({"project_id":project.id,"confirm_name":"Target board"});
    let source = dir.path().join("source.txt");
    std::fs::write(&source, "Source files stay unchanged").unwrap();
    host.app("default")
        .unwrap()
        .store
        .mutate(|s| {
            engine::create_task(s, json!({"title":"Unrelated work"}))?;
            Ok(())
        })
        .unwrap();
    let other_before =
        serde_json::to_value(host.app("default").unwrap().store.read().unwrap()).unwrap();
    let store = host.app(&project.id).unwrap().store;
    store.mutate(|s| {
        let task = engine::create_task(s, json!({"title":"Reset this task", "workspace":dir.path()}))?;
        engine::save_agent(s, None, json!({"name":"Builder","adapter":"cli","command":"example","stages":["implementation"]}))?;
        s.settings.concurrency = 5;
        s.settings.paused = true;
        s.attempts.push(Attempt {
            id:model::id(), task_id:task.id.clone(), stage:"implementation".into(), visit:1, number:1,
            agent_id:s.agents[0].id.clone(),agent_name:"Builder".into(),adapter:"cli".into(),model:String::new(),
            status:"completed".into(),started_at:model::now(),finished_at:Some(model::now()),duration_ms:Some(1),
            summary:"Evidence".into(),error:String::new(),usage:Usage::default(),input:Value::Null,output:None,log:String::new(),external:None,
        });
        s.artifacts.push(Artifact {id:model::id(), task_id:task.id,attempt_id:None,stage:"implementation".into(),visit:1,name:"Evidence".into(),content:"Preserve in backup".into(),media_type:"text/plain".into(),stale:false,created_at:model::now()});
        Ok(())
    }).unwrap();
    let prior = serde_json::to_value(store.read().unwrap()).unwrap();
    assert_eq!(call(&router, "", &path, confirmation.clone()).await.0, 403);
    assert_eq!(
        call(
            &router,
            &token,
            &path,
            json!({"project_id":"default","confirm_name":"Target board"})
        )
        .await
        .0,
        400
    );
    assert_eq!(
        call(
            &router,
            &token,
            &path,
            json!({"project_id":project.id,"confirm_name":"wrong"})
        )
        .await
        .0,
        409
    );
    assert_eq!(
        call(
            &router,
            &token,
            "/api/projects/default/reset",
            json!({"project_id":"default","confirm_name":"My workspace"})
        )
        .await
        .0,
        400
    );
    assert_eq!(serde_json::to_value(store.read().unwrap()).unwrap(), prior);
    for status in ["running", "cancelling", "lost"] {
        store
            .mutate(|s| {
                s.attempts[0].status = status.into();
                Ok(())
            })
            .unwrap();
        let before = serde_json::to_value(store.read().unwrap()).unwrap();
        assert_eq!(
            call(&router, &token, &path, confirmation.clone()).await.0,
            409
        );
        assert_eq!(serde_json::to_value(store.read().unwrap()).unwrap(), before);
    }
    store
        .mutate(|s| {
            s.attempts[0].status = "completed".into();
            s.agents[0].worker = Some(model::WorkerRegistration {
                source: "test".into(),
                seen_at: model::now(),
                lease_seconds: 120,
                assignment: Some(model::Assignment {
                    id: model::id(),
                    task_id: s.tasks[0].id.clone(),
                    task_revision: 1,
                    stage: "implementation".into(),
                    visit: 1,
                    workspace: String::new(),
                    offered_at: model::now(),
                }),
            });
            Ok(())
        })
        .unwrap();
    assert_eq!(
        call(&router, &token, &path, confirmation.clone()).await.0,
        409
    );
    store
        .mutate(|s| {
            s.agents[0].worker = None;
            Ok(())
        })
        .unwrap();
    let before_reset = serde_json::to_value(store.read().unwrap()).unwrap();
    let (status, result) = call(&router, &token, &path, confirmation).await;
    assert_eq!(status, 200);
    assert_eq!(result["project_id"], project.id);
    let reset = store.read().unwrap();
    assert!(
        reset.tasks.is_empty()
            && reset.attempts.is_empty()
            && reset.artifacts.is_empty()
            && reset.events.is_empty()
            && reset.agents.is_empty()
    );
    let expected = Settings {
        project_name: "Target board".into(),
        ..Settings::default()
    };
    assert_eq!(
        serde_json::to_value(reset.settings).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(
        serde_json::to_value(host.app("default").unwrap().store.read().unwrap()).unwrap(),
        other_before
    );
    assert_eq!(
        std::fs::read_to_string(source).unwrap(),
        "Source files stay unchanged"
    );
    let backup = Store::open(result["backup_path"].as_str().unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(backup.read().unwrap()).unwrap(),
        before_reset
    );
    assert_eq!(host.list().unwrap().len(), 2);
}
