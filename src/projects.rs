use crate::{
    api::{self, App},
    engine,
    metrics::Metrics,
    model,
    runner::Runner,
    store::*,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path as RoutePath, Request, State},
    middleware,
    response::{IntoResponse, Response},
    routing::get,
};
use fs2::FileExt;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::File,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tower::ServiceExt;

#[derive(Clone, Serialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub url: String,
    pub api_base: String,
    pub data_dir: PathBuf,
}
struct Runtime {
    app: App,
    _lock: File,
}
pub struct Host {
    root: PathBuf,
    port: u16,
    token: String,
    approval_code: String,
    pool: WorkspacePool,
    projects: RwLock<BTreeMap<String, Runtime>>,
    draining: AtomicBool,
}
impl Host {
    pub fn open(root: &Path, port: u16, approval_code: String) -> AppResult<Arc<Self>> {
        std::fs::create_dir_all(root)?;
        let root = std::fs::canonicalize(root)?;
        let host = Arc::new(Self {
            root,
            port,
            token: model::id(),
            approval_code,
            pool: WorkspacePool::default(),
            projects: RwLock::new(BTreeMap::new()),
            draining: AtomicBool::new(false),
        });
        let default = host.runtime(&host.root)?;
        let directory = host.root.join("projects");
        std::fs::create_dir_all(&directory)?;
        let canonical_directory = std::fs::canonicalize(&directory)?;
        {
            let mut projects = host.projects.write().unwrap();
            projects.insert("default".into(), default);
            for entry in std::fs::read_dir(&directory)? {
                let entry = entry?;
                let id = entry.file_name().to_string_lossy().into_owned();
                if uuid::Uuid::parse_str(&id).is_err() {
                    continue;
                }
                let path = std::fs::canonicalize(entry.path())?;
                if path.parent() != Some(canonical_directory.as_path()) {
                    return Err(bad(
                        "Project directories must stay inside the server data directory",
                    ));
                }
                if path.join("relay.sqlite").is_file() {
                    projects.insert(id, host.runtime(&path)?);
                }
            }
        }
        for runtime in host.projects.read().unwrap().values() {
            runtime.app.runner.start_scheduler();
        }
        Ok(host)
    }
    fn runtime(&self, path: &Path) -> AppResult<Runtime> {
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.join("relay.lock"))?;
        lock.try_lock_exclusive()
            .map_err(|_| conflict("Another Relay instance is using this project directory"))?;
        let store = Arc::new(Store::open_shared(
            path.join("relay.sqlite"),
            self.pool.clone(),
        )?);
        store.mutate(engine::recover)?;
        let runner = Runner::new(store.clone(), path.to_owned());
        runner
            .draining
            .store(self.draining.load(Ordering::Relaxed), Ordering::Relaxed);
        Ok(Runtime {
            app: App {
                store,
                runner,
                token: self.token.clone(),
                approval_code: self.approval_code.clone(),
                port: self.port,
            },
            _lock: lock,
        })
    }
    pub fn app(&self, id: &str) -> AppResult<App> {
        self.projects
            .read()
            .unwrap()
            .get(id)
            .map(|r| r.app.clone())
            .ok_or_else(|| AppError(404, "Project not found".into()))
    }
    fn describe(id: &str, app: &App) -> AppResult<Project> {
        Ok(Project {
            id: id.into(),
            name: app.store.read()?.settings.project_name,
            url: format!("/?project={id}"),
            api_base: format!("/api/projects/{id}"),
            data_dir: app.runner.data_dir.clone(),
        })
    }
    pub fn list(&self) -> AppResult<Vec<Project>> {
        self.projects
            .read()
            .unwrap()
            .iter()
            .map(|(id, r)| Self::describe(id, &r.app))
            .collect()
    }
    pub fn create(&self, name: &str) -> AppResult<Project> {
        let name = engine::workspace_name(name)?;
        let mut projects = self.projects.write().unwrap();
        if self.draining.load(Ordering::Relaxed) {
            return Err(AppError(503, "Coordinator upgrading; retry shortly".into()));
        }
        for runtime in projects.values() {
            if runtime
                .app
                .store
                .read()?
                .settings
                .project_name
                .trim()
                .to_lowercase()
                == name.to_lowercase()
            {
                return Err(conflict("A workspace with that name already exists"));
            }
        }
        let id = model::id();
        let path = self.root.join("projects").join(&id);
        std::fs::create_dir(&path)?;
        let runtime = self.runtime(&path)?;
        runtime.app.store.mutate(|s| {
            s.settings.project_name = name;
            Ok(())
        })?;
        let project = Self::describe(&id, &runtime.app)?;
        runtime.app.runner.start_scheduler();
        projects.insert(id, runtime);
        Ok(project)
    }
    pub fn save_settings(&self, id: &str, patch: Value) -> AppResult<model::Settings> {
        // Serialize renames with both other renames and workspace creation. The
        // store transaction additionally guards revision checks against scheduler changes.
        let projects = self.projects.write().unwrap();
        let runtime = projects
            .get(id)
            .ok_or_else(|| AppError(404, "Workspace not found".into()))?;
        if let Some(value) = patch.get("project_name") {
            let name = engine::workspace_name(
                value
                    .as_str()
                    .ok_or_else(|| bad("Workspace name must be text"))?,
            )?;
            for (_, other) in projects
                .iter()
                .filter(|(other_id, _)| other_id.as_str() != id)
            {
                if other
                    .app
                    .store
                    .read()?
                    .settings
                    .project_name
                    .trim()
                    .to_lowercase()
                    == name.to_lowercase()
                {
                    return Err(conflict("A workspace with that name already exists"));
                }
            }
        }
        let settings = runtime
            .app
            .store
            .mutate(|s| engine::save_settings(s, patch))?;
        runtime.app.runner.wake.notify_one();
        Ok(settings)
    }
    pub fn global_settings(&self) -> Value {
        json!({
            "draining": self.draining.load(Ordering::Relaxed),
            "build_id": env!("RELAY_BUILD_ID"),
            "version": env!("CARGO_PKG_VERSION"),
            "data_dir": self.root,
        })
    }
    pub fn reset_workspace(&self, id: &str, confirmation: Value) -> AppResult<Value> {
        let fields = confirmation
            .as_object()
            .ok_or_else(|| bad("Reset confirmation must be an object"))?;
        if fields.len() != 2 || confirmation["project_id"].as_str() != Some(id) {
            return Err(bad(
                "Confirm the exact project_id and confirm_name to reset a workspace",
            ));
        }
        let name = confirmation["confirm_name"]
            .as_str()
            .ok_or_else(|| bad("confirm_name must be the exact workspace name"))?;
        if id == "default" {
            return Err(bad(
                "The default workspace cannot be reset through this endpoint",
            ));
        }
        // Serialize identity confirmation with rename/create. Store admission locks
        // protect against in-flight task and worker mutations within the workspace.
        let projects = self.projects.write().unwrap();
        let runtime = projects
            .get(id)
            .ok_or_else(|| AppError(404, "Workspace not found".into()))?;
        let backup = runtime
            .app
            .runner
            .data_dir
            .join("backups")
            .join(format!("before-reset-{}.sqlite", model::id()));
        let revision = runtime.app.store.reset_workspace(name, &backup)?;
        runtime.app.runner.wake.notify_one();
        Ok(
            json!({"project_id":id,"name":name,"reset":true,"revision":revision,"backup_path":backup}),
        )
    }
    pub fn overview(&self) -> AppResult<Value> {
        let projects = self.projects.read().unwrap();
        let mut total = Metrics::default();
        let mut workspaces = Vec::new();
        for (id, runtime) in projects.iter() {
            let state = runtime.app.store.read()?;
            let metrics = Metrics::from_snapshot(&state);
            total.add(&metrics);
            let (checked_at, error) = runtime
                .app
                .runner
                .health
                .lock()
                .map(|h| h.clone())
                .unwrap_or_default();
            workspaces.push(json!({
                "id": id,
                "name": state.settings.project_name,
                "url": format!("/?project={id}"),
                "api_base": format!("/api/projects/{id}"),
                "data_dir": runtime.app.runner.data_dir,
                "metrics": metrics,
                "paused": state.settings.paused,
                "concurrency": state.settings.concurrency,
                "dispatch": engine::dispatch_status(&state),
                "coordinator": {
                    "checked_at": checked_at,
                    "error": error,
                    "draining": runtime.app.runner.draining.load(Ordering::Relaxed),
                    "mode": if state.settings.paused {"paused"} else {"running"},
                },
            }));
        }
        Ok(json!({
            "scope": "global", "generated_at": model::now(),
            "metrics": total, "workspaces": workspaces, "server": self.global_settings(),
        }))
    }
    pub fn drain(&self, value: bool) {
        let projects = self.projects.write().unwrap();
        self.draining.store(value, Ordering::Relaxed);
        for r in projects.values() {
            r.app.runner.draining.store(value, Ordering::Relaxed);
            r.app.runner.wake.notify_one();
        }
    }
    fn drain_workspace(&self, id: &str, value: bool) -> AppResult<Value> {
        // Keep the legacy scoped control compatible, but serialize its check and
        // mutation with host maintenance so it cannot race a global drain.
        let projects = self.projects.read().unwrap();
        let runtime = projects
            .get(id)
            .ok_or_else(|| AppError(404, "Workspace not found".into()))?;
        if self.draining.load(Ordering::Relaxed) {
            return Err(AppError(503, "Server is draining all workspaces".into()));
        }
        runtime.app.runner.draining.store(value, Ordering::Relaxed);
        runtime.app.runner.wake.notify_one();
        Ok(json!({"build_id":env!("RELAY_BUILD_ID"),"draining":value}))
    }
    pub fn stop(&self) {
        for r in self.projects.read().unwrap().values() {
            r.app.runner.stopping.store(true, Ordering::Relaxed);
            r.app.runner.wake.notify_one();
        }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn router(host: Arc<Host>) -> Router {
    let guard = host.app("default").unwrap();
    Router::new()
        .route("/api/projects", get(list).post(create))
        .route(
            "/api/projects/{id}/reset",
            axum::routing::post(reset_workspace),
        )
        .route("/api/overview", get(overview))
        .route(
            "/api/global/settings",
            get(global_settings).patch(update_global_settings),
        )
        .route("/api/settings", axum::routing::patch(default_settings))
        .route(
            "/api/projects/{id}/settings",
            axum::routing::patch(workspace_settings),
        )
        .route("/api/admin/drain", axum::routing::post(drain))
        .route(
            "/api/projects/{id}/admin/drain",
            axum::routing::post(workspace_drain),
        )
        .fallback(dispatch)
        .layer(DefaultBodyLimit::disable())
        .layer(middleware::from_fn_with_state(guard, api::guard))
        .with_state(host)
}
async fn list(State(host): State<Arc<Host>>) -> AppResult<Json<Vec<Project>>> {
    Ok(Json(host.list()?))
}
async fn create(State(host): State<Arc<Host>>, Json(v): Json<Value>) -> AppResult<Json<Project>> {
    Ok(Json(host.create(v["name"].as_str().unwrap_or(""))?))
}
async fn reset_workspace(
    State(host): State<Arc<Host>>,
    RoutePath(id): RoutePath<String>,
    Json(v): Json<Value>,
) -> AppResult<Json<Value>> {
    Ok(Json(host.reset_workspace(&id, v)?))
}
async fn overview(State(host): State<Arc<Host>>) -> AppResult<Json<Value>> {
    Ok(Json(host.overview()?))
}
async fn global_settings(State(host): State<Arc<Host>>) -> Json<Value> {
    Json(host.global_settings())
}
async fn update_global_settings(
    State(host): State<Arc<Host>>,
    Json(v): Json<Value>,
) -> AppResult<Json<Value>> {
    let fields = v
        .as_object()
        .ok_or_else(|| bad("Global settings must be an object"))?;
    if fields.len() != 1 || !fields.contains_key("draining") {
        return Err(bad("The only editable global setting is draining"));
    }
    let draining = v["draining"]
        .as_bool()
        .ok_or_else(|| bad("draining must be a boolean"))?;
    host.drain(draining);
    Ok(Json(host.global_settings()))
}
async fn default_settings(
    State(host): State<Arc<Host>>,
    Json(v): Json<Value>,
) -> AppResult<Json<model::Settings>> {
    Ok(Json(host.save_settings("default", v)?))
}
async fn workspace_settings(
    State(host): State<Arc<Host>>,
    RoutePath(id): RoutePath<String>,
    Json(v): Json<Value>,
) -> AppResult<Json<model::Settings>> {
    Ok(Json(host.save_settings(&id, v)?))
}
async fn drain(State(host): State<Arc<Host>>, Json(v): Json<Value>) -> Json<Value> {
    host.drain(v["draining"].as_bool().unwrap_or(true));
    Json(
        json!({"build_id":env!("RELAY_BUILD_ID"),"draining":host.draining.load(Ordering::Relaxed)}),
    )
}
async fn workspace_drain(
    State(host): State<Arc<Host>>,
    RoutePath(id): RoutePath<String>,
    Json(v): Json<Value>,
) -> AppResult<Json<Value>> {
    Ok(Json(host.drain_workspace(
        &id,
        v["draining"].as_bool().unwrap_or(true),
    )?))
}
async fn dispatch(State(host): State<Arc<Host>>, mut request: Request) -> AppResult<Response> {
    let path = request.uri().path().to_owned();
    let (id, target) = if let Some(scoped) = path.strip_prefix("/api/projects/") {
        let (id, tail) = scoped
            .split_once('/')
            .ok_or_else(|| AppError(404, "Project endpoint not found".into()))?;
        (id, format!("/api/{tail}"))
    } else {
        ("default", path.clone())
    };
    let app = host.app(id)?;
    request
        .extensions_mut()
        .insert(api::ProjectScope(format!("/api/projects/{id}")));
    if target == "/api/admin/drain" && host.draining.load(Ordering::Relaxed) {
        return Err(AppError(503, "Server is draining all projects".into()));
    }
    if request.method() == axum::http::Method::GET
        && ["/api/bootstrap", "/api/state"].contains(&target.as_str())
    {
        let mut value = if target == "/api/bootstrap" {
            api::bootstrap(State(app)).await.0
        } else {
            api::snapshot(State(app)).await?.0
        };
        value["project_id"] = json!(id);
        value["api_base"] = json!(format!("/api/projects/{id}"));
        value["projects"] = json!(host.list()?);
        if target == "/api/bootstrap" {
            value["capabilities"]
                .as_array_mut()
                .unwrap()
                .push(json!("multiple_projects"));
            value["capabilities"].as_array_mut().unwrap().extend([
                json!("global_overview"),
                json!("scoped_metrics"),
                json!("global_settings"),
                json!("partial_workspace_settings"),
            ]);
        }
        return Ok(Json(value).into_response());
    }
    let query = request
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    *request.uri_mut() = format!("{target}{query}")
        .parse()
        .map_err(|_| bad("Invalid project path"))?;
    Ok(api::router(app).oneshot(request).await.unwrap())
}
