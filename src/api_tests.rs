use super::*;
use crate::test_support::Temp;
use std::path::Path;

fn resolve(body: &str, folder: &Path) -> Value {
    serde_json::from_str(&body.replace(FOLDER, &crate::git::shown(folder))).unwrap()
}

#[test]
fn every_example_body_is_accepted_by_its_route() {
    let temp = Temp::new();
    let folder = temp.dir("workspace");
    let mut engine = temp.engine();
    let mut shutdown = false;
    let seed = engine.create(json!({"title":"Seed","outcome":"It exists","cwd":folder,"acceptance":["It holds"],"standalone":true})).unwrap()["id"].as_str().unwrap().to_owned();
    let mut tried = 0;
    for route in ROUTES.iter().filter(|r| r.body.is_some()) {
        let path = route.path.replace("{id}", &seed);
        let parts: Vec<&str> = path.split('/').collect();
        let result = crate::web::dispatch(&mut engine, route.method, &parts, resolve(route.body.unwrap(), &folder), 0, &mut shutdown);
        assert!(result.is_ok(), "{} {}: {}", route.method, route.path, result.err().map(|e| e.to_string()).unwrap_or_default());
        tried += 1;
    }
    assert_eq!(tried, 4);
    assert!(!shutdown);
}

#[test]
fn rejected_requests_carry_the_route_example_and_unknown_routes_point_to_the_index() {
    let error = hint("POST", &["", "api", "tasks"], "cwd is required");
    assert!(error.starts_with("cwd is required") && error.contains("{\"title\""));
    assert!(hint("POST", &["", "api", "tasks", "abc", "replan"], "Unknown task").contains(r#"{"reason":"#));
    assert_eq!(hint("POST", &["", "api", "tasks", "abc", "stop"], "Unknown task"), "Unknown task");
    assert!(hint("POST", &["", "api", "nothing"], "Unknown route").contains("GET /api"));
    let listed = index();
    let routes = listed["routes"].as_array().unwrap();
    assert_eq!(routes.len(), ROUTES.len());
    assert!(routes.iter().any(|r| r["path"] == "/api/tasks/batch" && r["body"]["children"].is_array()));
}

#[test]
fn the_workspace_guide_lists_every_agent_route_and_no_admin_route() {
    let text = guide("it waits");
    for r in ROUTES {
        assert_eq!(text.contains(&format!("`{} {}{}`", r.method, r.path, r.query)), r.agent, "{} {}", r.method, r.path);
    }
    assert!(text.contains(r#""requestKey":"plan-1""#));
}
