use crate::{callbacks, engine, external, model::*, plan, runner::Runner, store::*, workers};
use axum::{
    Extension, Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, patch, post},
};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone)]
pub struct App {
    pub store: Arc<Store>,
    pub runner: Runner,
    pub token: String,
    pub approval_code: String,
    pub port: u16,
}
#[derive(Clone)]
pub(crate) struct ProjectScope(pub String);
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::from_u16(self.0).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({"error":self.1})),
        )
            .into_response()
    }
}
pub fn router(app: App) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../web/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/app.js"),
                )
            }),
        )
        .route(
            "/status.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/status.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../web/style.css"),
                )
            }),
        )
        .route(
            "/metrics.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/metrics.js"),
                )
            }),
        )
        .route(
            "/metrics.js.LEGAL.txt",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                    include_str!("../web/metrics.js.LEGAL.txt"),
                )
            }),
        )
        .route("/api/bootstrap", get(bootstrap))
        .route("/api/state", get(snapshot))
        .route("/api/metrics", get(metrics))
        .route("/api/tasks", post(create_task))
        .route("/api/tasks/{id}", patch(update_task))
        .route("/api/tasks/{id}/action", post(task_action))
        .route("/api/tasks/{id}/run", post(run_task))
        .route("/api/plan/preview", post(preview_plan))
        .route("/api/plan/import", post(import_plan))
        .route("/api/agents", post(create_agent))
        .route("/api/agents/{id}", patch(update_agent))
        .route("/api/settings", patch(settings))
        .route("/api/external/claim", post(external_claim))
        .route("/api/workers/report", post(worker_report))
        .route("/api/workers/register", post(register_worker))
        .route("/api/workers/next", post(next_assignment))
        .route("/api/workers/disconnect", post(disconnect_worker))
        .route("/api/planners/register", post(register_planner))
        .route("/api/planners/{id}/next", post(next_planner_notification))
        .route(
            "/api/planners/{id}/ack",
            post(acknowledge_planner_notification),
        )
        .route("/api/admin/drain", post(drain))
        .route("/api/external/{id}/heartbeat", post(external_heartbeat))
        .route("/api/external/{id}/result", post(external_result))
        .route("/api/external/{id}/stopped", post(external_stopped))
        .route("/api/backup", post(backup))
        .route("/api/export", get(export))
        .layer(DefaultBodyLimit::disable())
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app)
}
pub(crate) async fn guard(State(app): State<App>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let allowed = [
        format!("127.0.0.1:{}", app.port),
        format!("localhost:{}", app.port),
        format!("[::1]:{}", app.port),
    ];
    if !allowed.iter().any(|v| v == host) {
        return AppError(403, "Only local requests are accepted".into()).into_response();
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        if origin.to_str().ok() != Some(&format!("http://{host}")) {
            return AppError(403, "Cross-origin requests are not allowed".into()).into_response();
        }
    }
    if request.method() != axum::http::Method::GET
        && request.method() != axum::http::Method::HEAD
        && request
            .headers()
            .get("x-relay-token")
            .and_then(|v| v.to_str().ok())
            != Some(&app.token)
    {
        return AppError(403, "Missing local session token. Reload the page.".into())
            .into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response.headers_mut().insert("content-security-policy","default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'".parse().unwrap());
    response
}
pub(crate) async fn bootstrap(State(app): State<App>) -> Json<Value> {
    Json(
        json!({"product":"relay","token":app.token,"version":env!("CARGO_PKG_VERSION"),"build_id":env!("RELAY_BUILD_ID"),"worker_protocol":1,"workflow_version":2,"capabilities":["worker_inbox","worker_claim","worker_heartbeat","worker_result","worker_presence","automatic_handoffs","automatic_retries","planner_callbacks","drain"],"stages":STAGES,"data_dir":app.runner.data_dir,"project_name":app.store.read().map(|s| s.settings.project_name).unwrap_or_default()}),
    )
}
pub(crate) async fn snapshot(State(app): State<App>) -> AppResult<Json<Value>> {
    let state = app.store.read()?;
    let mut value = serde_json::to_value(&state)?;
    let reasons: serde_json::Map<String, Value> = state
        .tasks
        .iter()
        .filter(|t| !t.archived && t.kind != "epic" && t.status == "ready" && t.stage != "done")
        .filter_map(|t| {
            engine::eligible_agent(&state, t).err().map(|r| {
                (
                    t.id.clone(),
                    json!(if r.starts_with("No eligible agent")
                        && state
                            .agents
                            .iter()
                            .any(|a| a.adapter == "external" && a.stages.contains(&t.stage))
                    {
                        "Waiting for a worker claim".to_string()
                    } else {
                        r
                    }),
                )
            })
        })
        .collect();
    value["queue_reasons"] = Value::Object(reasons);
    value["dispatch"] = engine::dispatch_status(&state);
    let (checked_at, error) = app
        .runner
        .health
        .lock()
        .map(|h| h.clone())
        .unwrap_or_default();
    value["coordinator"] = json!({"checked_at":checked_at,"error":error,"mode":if state.settings.paused {"paused"} else {"running"},"draining":app.runner.draining.load(std::sync::atomic::Ordering::Relaxed),"build_id":env!("RELAY_BUILD_ID")});
    Ok(Json(value))
}
async fn metrics(State(app): State<App>) -> AppResult<Json<crate::metrics::Metrics>> {
    Ok(Json(crate::metrics::Metrics::from_snapshot(
        &app.store.read()?,
    )))
}
async fn create_task(State(app): State<App>, Json(v): Json<Value>) -> AppResult<Json<Task>> {
    Ok(Json(app.store.mutate(|s| engine::create_task(s, v))?))
}
async fn update_task(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> AppResult<Json<Task>> {
    Ok(Json(app.store.mutate(|s| engine::update_task(s, &id, v))?))
}
async fn task_action(
    State(app): State<App>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(v): Json<Value>,
) -> AppResult<Json<Task>> {
    let authorized = !app.approval_code.is_empty()
        && headers
            .get("x-relay-approval")
            .and_then(|value| value.to_str().ok())
            == Some(app.approval_code.as_str());
    Ok(Json(
        app.store
            .mutate(|s| engine::action_as(s, &id, v, authorized))?,
    ))
}
async fn preview_plan(
    State(app): State<App>,
    Json(request): Json<plan::ImportRequest>,
) -> AppResult<Json<plan::ImportReport>> {
    Ok(Json(plan::reconcile(
        &mut app.store.read()?,
        request,
        true,
    )?))
}
async fn import_plan(
    State(app): State<App>,
    Json(request): Json<plan::ImportRequest>,
) -> AppResult<Json<plan::ImportReport>> {
    Ok(Json(
        app.store.mutate(|s| plan::reconcile(s, request, false))?,
    ))
}
async fn run_task(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> AppResult<Json<Attempt>> {
    if app
        .runner
        .draining
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return Err(AppError(503, "Coordinator upgrading; retry shortly".into()));
    }
    let job = app.store.mutate(|s| {
        let task = s
            .tasks
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| AppError(404, "Task not found".into()))?;
        if v["revision"].as_u64() != Some(task.revision) {
            return Err(conflict("Task changed. Refresh and try again."));
        }
        engine::claim(s, Some(&id))?.ok_or_else(|| conflict("No work available"))
    })?;
    let attempt = job.0.clone();
    app.runner.launch(job.0, job.1);
    Ok(Json(attempt))
}
async fn create_agent(State(app): State<App>, Json(v): Json<Value>) -> AppResult<Json<Agent>> {
    Ok(Json(app.store.mutate(|s| engine::save_agent(s, None, v))?))
}
async fn update_agent(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(v): Json<Value>,
) -> AppResult<Json<Agent>> {
    Ok(Json(
        app.store.mutate(|s| engine::save_agent(s, Some(&id), v))?,
    ))
}
async fn settings(State(app): State<App>, Json(v): Json<Value>) -> AppResult<Json<Settings>> {
    let settings = app.store.mutate(|s| engine::save_settings(s, v))?;
    app.runner.wake.notify_one();
    Ok(Json(settings))
}
async fn external_claim(
    State(app): State<App>,
    Json(v): Json<external::Claim>,
) -> AppResult<Json<Value>> {
    if app
        .runner
        .draining
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return Err(AppError(
            503,
            "Coordinator upgrading; retry this request ID shortly".into(),
        ));
    }
    Ok(Json(
        json!({"attempt":app.store.mutate(|s| external::claim(s, v))?}),
    ))
}
async fn drain(State(app): State<App>, Json(v): Json<Value>) -> Json<Value> {
    app.runner.draining.store(
        v["draining"].as_bool().unwrap_or(true),
        std::sync::atomic::Ordering::Relaxed,
    );
    Json(
        json!({"build_id":env!("RELAY_BUILD_ID"),"draining":app.runner.draining.load(std::sync::atomic::Ordering::Relaxed)}),
    )
}
async fn worker_report(
    State(app): State<App>,
    Json(v): Json<external::Report>,
) -> AppResult<Json<Agent>> {
    Ok(Json(app.store.mutate(|s| external::report(s, v))?))
}

async fn register_worker(
    State(app): State<App>,
    scope: Option<Extension<ProjectScope>>,
    Json(v): Json<workers::Registration>,
) -> AppResult<Json<Value>> {
    let agent = app.store.mutate(|s| workers::register(s, v))?;
    app.runner.wake.notify_one();
    Ok(Json(
        json!({"worker_id":agent.id,"agent":agent,"next":format!("{}/workers/next", scope.map(|s| s.0.0).unwrap_or_else(|| "/api".into()))}),
    ))
}

#[derive(serde::Deserialize)]
struct InboxRequest {
    worker_id: String,
    #[serde(default = "inbox_wait")]
    wait_seconds: u64,
}
fn inbox_wait() -> u64 {
    20
}

async fn next_assignment(
    State(app): State<App>,
    Json(v): Json<InboxRequest>,
) -> AppResult<Json<Value>> {
    app.store.mutate(|s| workers::poll(s, &v.worker_id))?;
    app.runner.wake.notify_one();
    let lease = app
        .store
        .read()?
        .agents
        .iter()
        .find(|a| a.id == v.worker_id)
        .and_then(|a| a.worker.as_ref())
        .map(|w| w.lease_seconds)
        .unwrap_or(30);
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(v.wait_seconds.min(30).min((lease / 3).max(1)));
    loop {
        if app
            .runner
            .draining
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(AppError(
                503,
                "Coordinator upgrading; reconnect shortly".into(),
            ));
        }
        if let Some(attempt) = app.store.mutate(|s| workers::receive(s, &v.worker_id))? {
            return Ok(Json(
                json!({"attempt":attempt,"next_action":if attempt.status == "running" {"execute"} else {"stop_and_acknowledge"}}),
            ));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(Json(
                json!({"attempt":null,"next_action":"wait","retry_after_ms":500,"dispatch":engine::dispatch_status(&app.store.read()?)}),
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

async fn disconnect_worker(
    State(app): State<App>,
    Json(v): Json<InboxRequest>,
) -> AppResult<Json<Value>> {
    app.store.mutate(|s| workers::disconnect(s, &v.worker_id))?;
    app.runner.wake.notify_one();
    Ok(Json(json!({"ok":true})))
}

async fn register_planner(
    State(app): State<App>,
    Json(v): Json<callbacks::Registration>,
) -> AppResult<Json<Value>> {
    let planner = app.store.mutate(|s| callbacks::register(s, v))?;
    Ok(Json(json!({"planner_id":planner.id,"planner":planner})))
}

async fn next_planner_notification(
    State(app): State<App>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    let notification = app.store.mutate(|s| callbacks::next(s, &id))?;
    Ok(Json(
        json!({"notification":notification,"retry_after_ms":1000}),
    ))
}

#[derive(serde::Deserialize)]
struct PlannerReceipt {
    notification_id: String,
}

async fn acknowledge_planner_notification(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(v): Json<PlannerReceipt>,
) -> AppResult<Json<Value>> {
    let notification = app
        .store
        .mutate(|s| callbacks::acknowledge(s, &id, &v.notification_id))?;
    Ok(Json(
        json!({"acknowledged":notification.id,"acknowledged_at":notification.acknowledged_at}),
    ))
}
async fn external_heartbeat(
    State(app): State<App>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    Ok(Json(app.store.mutate(|s| external::heartbeat(s, &id))?))
}
async fn external_result(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(result): Json<AgentResult>,
) -> AppResult<Json<Value>> {
    app.store.mutate(|s| {
        let a = s
            .attempts
            .iter()
            .find(|a| a.id == id && a.external.is_some())
            .ok_or_else(|| AppError(404, "External attempt not found".into()))?;
        if a.status == "lost" {
            return Err(conflict(
                "Lease expired; stop the old run and acknowledge it before retrying",
            ));
        }
        engine::finish(s, &id, result, String::new())?;
        engine::reconcile(s)
    })?;
    app.runner.wake.notify_one();
    Ok(Json(json!({"ok":true})))
}
async fn external_stopped(
    State(app): State<App>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    app.store.mutate(|s| external::acknowledge_stop(s, &id))?;
    app.runner.wake.notify_one();
    Ok(Json(json!({"ok":true})))
}
async fn backup(State(app): State<App>) -> AppResult<Json<Value>> {
    let dir = app.runner.data_dir.join("backups");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "relay-{}.sqlite",
        chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f")
    ));
    app.store.backup(&path)?;
    Ok(Json(json!({"path":path})))
}
async fn export(State(app): State<App>) -> AppResult<Response> {
    Ok((
        [
            (header::CONTENT_TYPE, "application/json"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=relay-export.json",
            ),
        ],
        serde_json::to_string_pretty(&app.store.read()?)?,
    )
        .into_response())
}
