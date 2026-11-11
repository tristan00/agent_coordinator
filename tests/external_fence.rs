use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use relay_coordinator::{
    api::{self, App},
    engine,
    runner::Runner,
    store::Store,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn post(app: &axum::Router, path: &str, body: Value) -> (u16, Value) {
    let response = app
        .clone()
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
    let status = response.status().as_u16();
    let body =
        serde_json::from_slice(&to_bytes(response.into_body(), 2_000_000).await.unwrap()).unwrap();
    (status, body)
}

#[tokio::test]
async fn late_external_results_cannot_release_workspace_until_stop_acknowledgement() {
    for action in ["pause", "cancel", "expire"] {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(temp.path().join("relay.sqlite")).unwrap());
        let (owner, competitor) = store
            .mutate(|s| {
                s.settings.paused = true;
                let owner = engine::create_task(
                    s,
                    json!({"title":"Owner", "acceptance":"Fixture", "workspace":temp.path()}),
                )?;
                let competitor = engine::create_task(
                    s,
                    json!({"title":"Competitor", "acceptance":"Fixture", "workspace":temp.path()}),
                )?;
                for task in [&owner, &competitor] {
                    engine::action(
                        s,
                        &task.id,
                        json!({"action":"start", "revision":task.revision}),
                    )?;
                }
                Ok((owner, competitor))
            })
            .unwrap();
        let app = api::router(App {
            store: store.clone(),
            runner: Runner::new(store.clone(), temp.path().into()),
            token: "test-token".into(),
            approval_code: String::new(),
            port: 4242,
        });
        let claim = |task_id: &str, request_id: &str| {
            json!({"source":"fence-test", "agent_id":request_id,
            "name":request_id, "request_id":request_id, "stages":["planning"], "task_id":task_id, "lease_seconds":120})
        };
        let (status, run) = post(&app, "/api/external/claim", claim(&owner.id, "owner")).await;
        assert_eq!(status, 200, "{run}");
        let id = run["attempt"]["id"].as_str().unwrap();
        if action == "expire" {
            store
                .mutate(|s| {
                    s.attempts[0].external.as_mut().unwrap().heartbeat_at =
                        "2000-01-01T00:00:00Z".into();
                    engine::reconcile(s)
                })
                .unwrap();
        } else {
            let revision = store
                .read()
                .unwrap()
                .tasks
                .iter()
                .find(|t| t.id == owner.id)
                .unwrap()
                .revision;
            let (status, body) = post(
                &app,
                &format!("/api/tasks/{}/action", owner.id),
                json!({"action":action,"revision":revision}),
            )
            .await;
            assert_eq!(status, 200, "{body}");
        }
        let before = serde_json::to_value(store.read().unwrap()).unwrap();
        for outcome in ["complete", "failed"] {
            let (status, body) = post(&app, &format!("/api/external/{id}/result"), json!({"outcome":outcome,"summary":"Late response","reason":"Delayed", "artifacts":[{"name":"late.txt","content":"Must not be published"}]})).await;
            assert_eq!(status, 409, "{action}: {body}");
            assert_eq!(serde_json::to_value(store.read().unwrap()).unwrap(), before);
        }
        let (status, body) = post(
            &app,
            "/api/external/claim",
            claim(&competitor.id, "blocked"),
        )
        .await;
        assert_eq!(status, 409, "{body}");
        assert!(body["attempt"].is_null());
        let stop_url = format!("/api/external/{id}/stopped");
        assert_eq!(post(&app, &stop_url, json!({})).await.0, 200);
        assert_eq!(post(&app, &stop_url, json!({})).await.0, 200);
        let (status, body) = post(&app, "/api/external/claim", claim(&competitor.id, "next")).await;
        assert_eq!(status, 200, "{body}");
        let current = body["attempt"]["id"].as_str().unwrap();
        assert_ne!(current, id);
        let before = serde_json::to_value(store.read().unwrap()).unwrap();
        assert_eq!(
            post(
                &app,
                &format!("/api/external/{id}/result"),
                json!({"outcome":"complete","summary":"Duplicate","artifacts":[{"name":"duplicate.txt","content":"Ignored"}]})
            )
            .await
            .0,
            200
        );
        assert_eq!(serde_json::to_value(store.read().unwrap()).unwrap(), before);
    }
}
