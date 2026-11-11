use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use relay_coordinator::{
    engine,
    model::{self, Attempt, Usage},
    projects::{self, Host},
};
use serde_json::{Value, json};
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
    (
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap()).unwrap(),
    )
}

fn attempt(task_id: &str, status: &str, usage: Usage) -> Attempt {
    Attempt {
        id: model::id(),
        task_id: task_id.into(),
        stage: "implementation".into(),
        visit: 1,
        number: 1,
        agent_id: "same-agent".into(),
        agent_name: "Builder".into(),
        adapter: "cli".into(),
        model: "test".into(),
        status: status.into(),
        started_at: model::now(),
        finished_at: None,
        duration_ms: if status == "running" {
            None
        } else {
            Some(1000)
        },
        summary: "private summary".into(),
        error: String::new(),
        usage,
        input: json!({"private":"handoff"}),
        output: None,
        log: "private log".into(),
        external: None,
    }
}

#[tokio::test]
async fn overview_and_scoped_metrics_share_counts_and_preserve_partial_usage() {
    let dir = tempfile::tempdir().unwrap();
    let host = Host::open(dir.path(), 4242, String::new()).unwrap();
    let other = host.create("Other").unwrap();
    let empty = host.create("Empty").unwrap();
    host.stop();
    host.app("default")
        .unwrap()
        .store
        .mutate(|s| {
            for title in ["Complete", "Needs input", "Archived", "Epic"] {
                engine::create_task(
                    s,
                    json!({"title":title, "kind":if title == "Epic" {"epic"} else {"task"}}),
                )?;
            }
            s.tasks[0].status = "completed".into();
            s.tasks[0].stage = "done".into();
            s.tasks[1].status = "needs_input".into();
            s.tasks[1].sendbacks = 1;
            s.tasks[2].archived = true;
            s.tasks[2].sendbacks = 2;
            s.attempts.push(attempt(
                &s.tasks[0].id,
                "completed",
                Usage {
                    input_tokens: Some(100),
                    output_tokens: None,
                    cost_usd: Some(0.5),
                    source: "reported".into(),
                },
            ));
            s.attempts
                .push(attempt(&s.tasks[1].id, "failed", Usage::default()));
            Ok(())
        })
        .unwrap();
    host.app(&other.id)
        .unwrap()
        .store
        .mutate(|s| {
            let task = engine::create_task(s, json!({"title":"Other task"}))?;
            s.attempts.push(attempt(
                &task.id,
                "running",
                Usage {
                    input_tokens: Some(0),
                    output_tokens: Some(40),
                    cost_usd: None,
                    source: "reported".into(),
                },
            ));
            Ok(())
        })
        .unwrap();
    let token = host.app("default").unwrap().token;
    let router = projects::router(host);
    let (status, global) = call(&router, &token, "GET", "/api/overview", Value::Null).await;
    assert_eq!(status, 200);
    assert_eq!(global["scope"], "global");
    assert_eq!(global["workspaces"].as_array().unwrap().len(), 3);
    assert_eq!(
        global["metrics"],
        json!({"tasks":3,"completed":1,"attention":1,"running_attempts":1,"attempts":3,"failed_attempts":1,"sendbacks":3,"duration_ms":2000,"input_tokens":100,"output_tokens":40,"cost_usd":0.5,"input_token_attempts":2,"output_token_attempts":1,"cost_attempts":1})
    );
    for workspace in global["workspaces"].as_array().unwrap() {
        let (_, scoped) = call(
            &router,
            &token,
            "GET",
            &format!("{}/metrics", workspace["api_base"].as_str().unwrap()),
            Value::Null,
        )
        .await;
        assert_eq!(workspace["metrics"], scoped);
        assert!(workspace.get("tasks").is_none());
        assert!(workspace.get("artifacts").is_none());
    }
    let (_, blank) = call(
        &router,
        &token,
        "GET",
        &format!("{}/metrics", empty.api_base),
        Value::Null,
    )
    .await;
    assert_eq!(blank["tasks"], 0);
    assert!(blank["input_tokens"].is_null());
    assert!(blank["output_tokens"].is_null());
    assert!(blank["cost_usd"].is_null());
    assert!(!global.to_string().contains("private"));
    let (_, legacy) = call(&router, &token, "GET", "/api/metrics", Value::Null).await;
    assert_eq!(legacy["tasks"], 2);
}

#[tokio::test]
async fn scoped_settings_patch_is_guarded_partial_and_compatible() {
    let dir = tempfile::tempdir().unwrap();
    let host = Host::open(dir.path(), 4242, String::new()).unwrap();
    let other = host.create("Other").unwrap();
    host.stop();
    let token = host.app("default").unwrap().token;
    let router = projects::router(host.clone());
    let prior = host.app(&other.id).unwrap().store.read().unwrap();
    let original = serde_json::to_value(&prior.settings).unwrap();
    let (status, saved) = call(
        &router,
        &token,
        "PATCH",
        &format!("{}/settings", other.api_base),
        json!({"project_name":"  Renamed  ","expected_revision":prior.revision}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(saved["project_name"], "Renamed");
    for key in [
        "paused",
        "concurrency",
        "stage_agents",
        "independent_review",
        "auto_advance",
        "retry_delay_seconds",
    ] {
        assert_eq!(saved[key], original[key]);
    }
    assert_eq!(
        host.app("default")
            .unwrap()
            .store
            .read()
            .unwrap()
            .settings
            .project_name,
        "My workspace"
    );
    assert_eq!(
        call(
            &router,
            &token,
            "PATCH",
            &format!("{}/settings", other.api_base),
            json!({"paused":true,"expected_revision":prior.revision})
        )
        .await
        .0,
        409
    );
    assert_eq!(
        call(&router, &token, "PATCH", "/api/settings", saved.clone())
            .await
            .0,
        409,
        "legacy full-object clients still enforce unique names"
    );
    let mut full = saved.clone();
    full["project_name"] = json!("Default renamed");
    assert_eq!(
        call(&router, &token, "PATCH", "/api/settings", full)
            .await
            .0,
        200
    );
    for name in ["", "   "] {
        assert_eq!(
            call(
                &router,
                &token,
                "POST",
                "/api/projects",
                json!({"name":name})
            )
            .await
            .0,
            400
        );
        assert_eq!(
            call(
                &router,
                &token,
                "PATCH",
                "/api/settings",
                json!({"project_name":name})
            )
            .await
            .0,
            400
        );
    }
    assert_eq!(
        call(
            &router,
            "wrong",
            "PATCH",
            "/api/settings",
            json!({"paused":true})
        )
        .await
        .0,
        403
    );
}

#[tokio::test]
async fn global_settings_validate_scope_and_preserve_workspace_pause_preferences() {
    let dir = tempfile::tempdir().unwrap();
    let host = Host::open(dir.path(), 4242, String::new()).unwrap();
    let other = host.create("Other").unwrap();
    host.save_settings("default", json!({"paused":true}))
        .unwrap();
    host.stop();
    let token = host.app("default").unwrap().token;
    let router = projects::router(host.clone());
    for invalid in [
        json!({"paused":true}),
        json!({"draining":"true"}),
        json!({"draining":true,"concurrency":4}),
        json!({}),
        Value::Null,
    ] {
        assert_eq!(
            call(&router, &token, "PATCH", "/api/global/settings", invalid)
                .await
                .0,
            400
        );
        assert_eq!(host.global_settings()["draining"], false);
    }
    assert_eq!(
        call(
            &router,
            "",
            "PATCH",
            "/api/global/settings",
            json!({"draining":true})
        )
        .await
        .0,
        403
    );
    let (status, diagnostics) = call(
        &router,
        &token,
        "PATCH",
        "/api/global/settings",
        json!({"draining":true}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(diagnostics["draining"], true);
    assert!(diagnostics.get("token").is_none());
    assert!(diagnostics["data_dir"].is_string());
    assert_eq!(host.create("During maintenance").err().unwrap().0, 503);
    for id in ["default", &other.id] {
        assert!(
            host.app(id)
                .unwrap()
                .runner
                .draining
                .load(std::sync::atomic::Ordering::Relaxed)
        );
    }
    assert_eq!(
        call(
            &router,
            &token,
            "POST",
            "/api/admin/drain",
            json!({"draining":false})
        )
        .await
        .0,
        200
    );
    assert!(
        host.app("default")
            .unwrap()
            .store
            .read()
            .unwrap()
            .settings
            .paused
    );
    assert!(
        !host
            .app(&other.id)
            .unwrap()
            .store
            .read()
            .unwrap()
            .settings
            .paused
    );
}
