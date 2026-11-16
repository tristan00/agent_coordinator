use serde_json::{Value, json};

pub struct Route {
    pub method: &'static str,
    pub path: &'static str,
    pub query: &'static str,
    pub purpose: &'static str,
    pub body: Option<&'static str>,
    pub agent: bool,
}
const fn route(method: &'static str, path: &'static str, query: &'static str, purpose: &'static str, body: Option<&'static str>, agent: bool) -> Route {
    Route { method, path, query, purpose, body, agent }
}
pub const FOLDER: &str = "{folder}";
pub const ROUTES: &[Route] = &[
    route("GET", "/api", "", "Every route with an example request body", None, true),
    route("GET", "/api/task-schema", "?workspace=W", "Allowed values for task fields: roles and what each is for, sizes, needs, dependency reasons and this workspace's limits", None, true),
    route("GET", "/api/tasks", "?workspace=W", "List the tasks", None, true),
    route("GET", "/api/tasks/{id}", "", "Read one task", None, true),
    route(
        "POST",
        "/api/tasks",
        "",
        "Create a task; its cwd must be inside this workspace. Group it with parent (a container id) or standalone true. Optional: dependencies [{\"target\":id,\"reason\":\"uses_api\"}], after [ids], attributes, model, effort",
        Some(r#"{"title":"Parse the input files","outcome":"Every input file loads into the typed model","cwd":"{folder}","acceptance":["The parser tests pass"],"standalone":true,"attributes":{"role":"backend-code","size":"small"}}"#),
        true,
    ),
    route(
        "POST",
        "/api/tasks/batch",
        "",
        "Create a container and its children at once. A container has no acceptance criteria; children inherit its cwd and refer to each other with {\"local\":localKey}",
        Some(
            r#"{"requestKey":"plan-1","container":{"title":"Game server","outcome":"The server runs the simulation","cwd":"{folder}","standalone":true},"children":[{"localKey":"api","title":"HTTP API","outcome":"Clients can join a game","acceptance":["The API tests pass"]},{"localKey":"log","title":"Action log","outcome":"Every action is logged","acceptance":["Replaying the log reproduces the state"],"after":[{"local":"api"}]}]}"#,
        ),
        true,
    ),
    route("PATCH", "/api/tasks/{id}", "", "Correct a task that is not running; send only the fields that change", Some(r#"{"outcome":"Every input file loads, including empty ones","acceptance":["The parser tests pass, including the empty-file case"]}"#), true),
    route("POST", "/api/tasks/{id}/stop", "", "Stop a task", None, true),
    route("POST", "/api/tasks/{id}/resume", "", "Resume a task", None, true),
    route("POST", "/api/tasks/{id}/replan", "", "Ask for a task to be replanned", Some(r#"{"reason":"The approach cannot reach the speed target"}"#), true),
    route("DELETE", "/api/tasks/{id}", "", "Remove a task", None, true),
    route("GET", "/api/updates", "?workspace=W&after=SEQ&wait=S", "Changes after sequence SEQ; {wait}", None, true),
    route("GET", "/api/agents", "?workspace=W", "Worker models a task can request", None, true),
    route("GET", "/api/attempts/{id}/log", "", "Read an attempt's log", None, true),
    route("GET", "/api/sessions/{id}", "", "Read a planner session; planners have their session id in `$COORDINATOR_SESSION`", None, true),
    route("GET", "/api/sessions", "", "List planner sessions", None, false),
    route("GET", "/api/sessions/{id}/requests/{n}/events", "?after=OFFSET", "Planner transcript lines of request n added after a byte offset, with the next offset", None, false),
    route("POST", "/api/sessions", "", "Start a planner session; fields workspace, directory (a folder inside it) and request", None, false),
    route("POST", "/api/sessions/{id}/messages", "", "Send a follow-up request to a waiting session; field request", None, false),
    route("POST", "/api/sessions/{id}/stop", "", "Stop a session", None, false),
    route("POST", "/api/workspaces", "", "Create a workspace with the default prompts; field path, optional settings", None, false),
    route("GET", "/api/workspaces/defaults", "", "Default prompts and settings", None, false),
    route("GET", "/api/state", "", "The whole board", None, false),
    route("GET", "/api/analytics", "", "Analytics report", None, false),
    route("POST", "/api/analytics", "", "Analytics report for a query", None, false),
    route("PUT", "/api/config", "", "Replace the coordinator configuration", None, false),
    route("POST", "/api/stop", "", "Pause all work", None, false),
    route("POST", "/api/resume", "", "Resume all work", None, false),
    route("POST", "/api/wipe", "", "Remove every task", None, false),
    route("GET", "/api/deployment", "", "Running build and deployment state", None, false),
    route("POST", "/api/deployment", "", "Redeploy; field source (a source directory) or candidate (an executable)", None, false),
    route("GET", "/api/deployment/requests/{id}", "", "Read a redeployment request", None, false),
    route("POST", "/api/deployment/quiesce", "", "Prepare the running build for handoff", None, false),
    route("POST", "/api/deployment/handoff", "", "Hand off to a deployed build", None, false),
    route("GET", "/api/kill-guard", "", "Kill guard directory and variable", None, false),
];
fn matches(pattern: &str, parts: &[&str]) -> bool {
    let pattern: Vec<_> = pattern.split('/').collect();
    pattern.len() == parts.len() && pattern.iter().zip(parts).all(|(p, part)| p == part || (p.starts_with('{') && !part.is_empty()))
}
pub fn find(method: &str, parts: &[&str]) -> Option<&'static Route> {
    ROUTES.iter().find(|r| r.method == method && matches(r.path, parts))
}
pub fn index() -> Value {
    let routes: Vec<_> = ROUTES.iter().map(|r| json!({"method":r.method,"path":format!("{}{}", r.path, r.query),"use":r.purpose,"body":r.body.map(|b| serde_json::from_str::<Value>(b).unwrap())})).collect();
    json!({"routes":routes,"placeholders":{"W":"the workspace path, URL-encoded","{id}":"a task, attempt or session id","{n}":"a request number, counting from 1","OFFSET":"a byte offset from the previous response","{folder}":"the folder the task works in, inside the workspace, with forward slashes"}})
}
pub fn hint(method: &str, parts: &[&str], error: &str) -> String {
    match find(method, parts) {
        Some(route) => match route.body {
            Some(body) => format!("{error}. Example body for {method} {}: {body} ({FOLDER} is a folder inside the workspace, with forward slashes). Allowed values: GET /api/task-schema?workspace=W", route.path),
            None => error.to_owned(),
        },
        None => format!("{error}. GET /api lists every route"),
    }
}
pub fn guide(wait: &str) -> String {
    let mut text = String::from("| Endpoint | Use |\n|---|---|\n");
    for r in ROUTES.iter().filter(|r| r.agent) {
        text += &format!("| `{} {}{}` | {} |\n", r.method, r.path, r.query, r.purpose.replace("{wait}", wait));
    }
    text += &format!("\nExample request bodies; `{FOLDER}` is the folder the task works in, inside this workspace, with forward slashes:\n\n");
    for r in ROUTES.iter().filter(|r| r.agent) {
        if let Some(body) = r.body {
            text += &format!("- `{} {}`: `{body}`\n", r.method, r.path);
        }
    }
    text
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
