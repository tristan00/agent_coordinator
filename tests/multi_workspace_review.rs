use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use relay_coordinator::projects::{self, Host};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn request(
    router: &Router,
    token: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
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

#[tokio::test]
async fn rejected_settings_patches_are_atomic_and_names_remain_unique() {
    let directory = tempfile::tempdir().unwrap();
    let host = Host::open(directory.path(), 4242, String::new()).unwrap();
    let second = host.create("Second").unwrap();
    for id in ["default", second.id.as_str()] {
        host.app(id)
            .unwrap()
            .store
            .mutate(|state| {
                state.settings.paused = true;
                Ok(())
            })
            .unwrap();
    }
    let token = host.app("default").unwrap().token;
    let router = projects::router(host.clone());
    let before = request(&router, &token, "GET", "/api/state", Value::Null)
        .await
        .1;
    for invalid in [
        json!({"project_name":"Would rename", "concurrency":0}),
        json!({"project_name":"Would rename", "global_concurrency":2}),
        json!({"project_name":"Would rename", "expected_revision":before["revision"].as_u64().unwrap()+1}),
        json!({"project_name":"  second  "}),
    ] {
        let (status, _) = request(&router, &token, "PATCH", "/api/settings", invalid).await;
        assert!([400, 409].contains(&status));
        let after = request(&router, &token, "GET", "/api/state", Value::Null)
            .await
            .1;
        assert_eq!(after["settings"], before["settings"]);
        assert_eq!(after["revision"], before["revision"]);
    }

    let second_settings = format!("{}/settings", second.api_base);
    let (first, second_result) = tokio::join!(
        request(
            &router,
            &token,
            "PATCH",
            "/api/settings",
            json!({"project_name":"Shared name"})
        ),
        request(
            &router,
            &token,
            "PATCH",
            &second_settings,
            json!({"project_name":" shared NAME "})
        ),
    );
    let mut statuses = [first.0, second_result.0];
    statuses.sort();
    assert_eq!(statuses, [200, 409]);
    let (status, _) = request(
        &router,
        &token,
        "POST",
        "/api/projects",
        json!({"name":"SHARED NAME"}),
    )
    .await;
    assert_eq!(status, 409);
    let names: Vec<_> = host
        .list()
        .unwrap()
        .into_iter()
        .map(|project| project.name.to_lowercase())
        .collect();
    assert_eq!(
        names
            .iter()
            .filter(|name| name.as_str() == "shared name")
            .count(),
        1
    );
    host.stop();
}

#[tokio::test]
async fn scoped_legacy_drain_cannot_resume_a_host_in_maintenance() {
    let directory = tempfile::tempdir().unwrap();
    let host = Host::open(directory.path(), 4242, String::new()).unwrap();
    let second = host.create("Second").unwrap();
    let token = host.app("default").unwrap().token;
    let router = projects::router(host.clone());
    let (status, settings) = request(
        &router,
        &token,
        "PATCH",
        "/api/global/settings",
        json!({"draining":true}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(settings["draining"], true);

    for base in ["/api/projects/default", second.api_base.as_str()] {
        let (status, _) = request(
            &router,
            &token,
            "POST",
            &format!("{base}/admin/drain"),
            json!({"draining":false}),
        )
        .await;
        assert!(
            status >= 400,
            "A workspace route must not clear host maintenance"
        );
        let (_, state) = request(
            &router,
            &token,
            "GET",
            &format!("{base}/state"),
            Value::Null,
        )
        .await;
        assert_eq!(state["coordinator"]["draining"], true);
    }
    let (_, global) = request(&router, &token, "GET", "/api/global/settings", Value::Null).await;
    assert_eq!(global["draining"], true);
    let (status, _) = request(
        &router,
        &token,
        "PATCH",
        "/api/global/settings",
        json!({"draining":false}),
    )
    .await;
    assert_eq!(status, 200);
    for id in ["default", second.id.as_str()] {
        assert!(
            !host
                .app(id)
                .unwrap()
                .runner
                .draining
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        assert!(!host.app(id).unwrap().store.read().unwrap().settings.paused);
    }
    host.stop();
}
