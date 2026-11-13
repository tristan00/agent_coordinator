use super::*;
use crate::test_support::{Temp, merge};
use tiny_http::{Method, TestRequest};

const PORT: u16 = 54_321;

fn request(method: Method, url: &str, body: &str, headers: &[(&str, &str)]) -> Request {
    let mut test = TestRequest::new().with_method(method).with_path(url).with_body(Box::leak(body.to_owned().into_boxed_str()));
    for (name, value) in headers {
        test = test.with_header(header(name, value));
    }
    test.into()
}
fn host() -> String {
    format!("127.0.0.1:{PORT}")
}
fn api(method: Method, url: &str, body: Value) -> Request {
    request(method, url, &body.to_string(), &[("Host", &host()), ("Content-Type", "application/json")])
}
fn encode(text: &str) -> String {
    text.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}
struct Site {
    engine: Engine,
    ws: PathBuf,
    _temp: Temp,
}
impl Site {
    fn new() -> Self {
        let temp = Temp::new();
        let ws = temp.dir("ws");
        Self { engine: temp.engine(), ws, _temp: temp }
    }
    fn spec(&self, title: &str) -> Value {
        json!({"title":title,"outcome":"Outcome","cwd":self.ws,"acceptance":["Criterion"],"standalone":true})
    }
    fn send(&mut self, request: Request) {
        let mut shutdown = false;
        handle(request, &mut self.engine, PORT, "", &mut shutdown).unwrap();
        assert!(!shutdown);
    }
    fn get(&mut self, url: &str) -> Result<Option<Value>> {
        let path = url.split('?').next().unwrap().to_owned();
        let parts: Vec<&str> = path.split('/').collect();
        get(&mut self.engine, &parts, url, "last error")
    }
    fn ids(&self) -> Vec<String> {
        self.engine.state.tasks.iter().map(|t| t.id.clone()).collect()
    }
}

#[test]
fn decode_handles_percent_escapes_plus_and_malformed_sequences() {
    assert_eq!(decode("a%20b+c"), "a b c");
    assert_eq!(decode("%C3%A9"), "\u{e9}");
    assert_eq!(decode("100%"), "100%");
    assert_eq!(decode("%zz%4"), "%zz%4");
    assert_eq!(decode("%2B%26%3D"), "+&=");
    assert_eq!(decode("%FF"), "\u{fffd}");
    assert_eq!(decode(""), "");
}

#[test]
fn update_query_parses_cursor_wait_and_canonical_workspace() {
    let temp = Temp::new();
    let ws = temp.dir("ws");
    let empty = update_query("/api/updates").unwrap();
    assert_eq!((empty.workspace, empty.after, empty.wait), (None, None, 0));
    let url = format!("/api/updates?after=5&&wait=10&workspace={}", encode(ws.to_str().unwrap()));
    let query = update_query(&url).unwrap();
    assert_eq!((query.workspace.as_deref(), query.after, query.wait), (Some(ws.as_path()), Some(5), 10));
    for (url, message) in [
        ("/api/updates?after=abc", "after must be a sequence number"),
        ("/api/updates?after", "after must be a sequence number"),
        ("/api/updates?wait=-1", "wait must be whole seconds"),
        ("/api/updates?since=1", "Unknown updates parameter since"),
    ] {
        assert!(update_query(url).err().unwrap().to_string().contains(message), "{url}");
    }
    let missing = format!("/api/updates?workspace={}", encode(temp.path().join("absent").to_str().unwrap()));
    assert!(update_query(&missing).is_err());
    assert!(update_query("/api/updates?workspace=relative").is_err());
}

#[test]
fn authorization_requires_local_host_and_matching_origin() {
    let local = host();
    let localhost = format!("localhost:{PORT}");
    let origin = format!("http://{local}");
    let check = |headers: &[(&str, &str)]| authorized(&request(Method::Get, "/", "", headers), PORT);
    assert!(check(&[("Host", &local)]));
    assert!(check(&[("host", &localhost)]));
    assert!(check(&[("Host", &local), ("Origin", &origin)]));
    assert!(check(&[("Host", &local), ("Origin", &format!("http://{localhost}"))]));
    assert!(!check(&[]));
    assert!(!check(&[("Host", "127.0.0.1:1")]));
    assert!(!check(&[("Host", "evil.example")]));
    assert!(!check(&[("Host", &local), ("Origin", "http://evil.example")]));
    assert!(!check(&[("Host", &local), ("Origin", &format!("https://{local}"))]));
}

#[test]
fn task_routes_create_edit_replan_and_delete() {
    let mut site = Site::new();
    site.send(api(Method::Post, "/api/tasks", site.spec("Alpha")));
    let ids = site.ids();
    assert_eq!(ids.len(), 1);
    let id = ids[0].clone();
    site.send(api(Method::Patch, &format!("/api/tasks/{id}"), json!({"outcome":"Edited outcome"})));
    assert_eq!(site.engine.state.tasks[0].outcome, "Edited outcome");
    site.send(api(Method::Post, &format!("/api/tasks/{id}/replan"), json!({"reason":"Rethink"})));
    assert_eq!(site.engine.state.tasks[0].stage, "replan");
    site.send(api(Method::Post, &format!("/api/tasks/{id}/stop"), json!({})));
    assert!(site.engine.state.tasks[0].stopped);
    site.send(api(Method::Post, &format!("/api/tasks/{id}/resume"), json!({})));
    assert!(!site.engine.state.tasks[0].stopped);
    site.send(api(Method::Delete, &format!("/api/tasks/{id}"), json!({})));
    assert!(site.engine.state.tasks.is_empty());
    assert_eq!(site.engine.state.archived.len(), 1);
}

#[test]
fn board_and_batch_routes_apply_their_effects() {
    let mut site = Site::new();
    site.send(api(Method::Post, "/api/stop", json!({})));
    assert!(site.engine.state.paused);
    site.send(api(Method::Post, "/api/resume", json!({})));
    assert!(!site.engine.state.paused);
    let container = merge(site.spec("Group"), json!({"acceptance":[]}));
    let batch = json!({"requestKey":"web-1","container":container,"children":[{"localKey":"a","title":"Child","outcome":"o","acceptance":["c"]}]});
    site.send(api(Method::Post, "/api/tasks/batch", batch.clone()));
    site.send(api(Method::Post, "/api/tasks/batch", batch));
    assert_eq!(site.engine.state.tasks.len(), 2);
    site.send(api(Method::Put, "/api/config", json!({"implementation":["i"],"review":["r"]})));
    assert_eq!(site.engine.state.config.replan, ["i"]);
    site.send(api(Method::Post, "/api/analytics", json!({"groupBy":"lane"})));
    site.send(api(Method::Post, "/api/unknown", json!({})));
    site.send(api(Method::Post, "/api/benchmark-runs/none/stop", json!({})));
    assert_eq!(site.engine.state.tasks.len(), 2);
}

#[test]
fn rejected_requests_have_no_effect() {
    let mut site = Site::new();
    let body = site.spec("Alpha").to_string();
    site.send(request(Method::Post, "/api/tasks", &body, &[("Host", "evil.example"), ("Content-Type", "application/json")]));
    site.send(request(Method::Post, "/api/tasks", &body, &[("Host", &host())]));
    site.send(request(Method::Post, "/api/tasks", &body, &[("Host", &host()), ("Content-Type", "text/plain")]));
    site.send(request(Method::Post, "/api/tasks", &body, &[("Host", &host()), ("Origin", "http://evil.example"), ("Content-Type", "application/json")]));
    site.send(request(Method::Post, "/api/tasks", "{not json", &[("Host", &host()), ("Content-Type", "application/json")]));
    site.send(request(Method::Post, "/api/tasks", "", &[("Host", &host()), ("Content-Type", "application/json")]));
    site.send(api(Method::Get, "/api/tasks", json!({})));
    assert!(site.engine.state.tasks.is_empty());
    site.send(request(Method::Post, "/api/tasks", &body, &[("Host", &host()), ("Content-Type", "application/json; charset=utf-8")]));
    assert_eq!(site.engine.state.tasks.len(), 1);
    assert_eq!(site.engine.state.sequence, 1);
}

#[test]
fn get_routes_serve_updates_tasks_and_analytics() {
    let mut site = Site::new();
    let id = site.engine.create(site.spec("Alpha")).unwrap()["id"].as_str().unwrap().to_owned();
    let updates = site.get("/api/updates?after=0").unwrap().unwrap();
    assert_eq!((updates["sequence"].as_u64(), updates["error"].as_str()), (Some(1), Some("last error")));
    assert_eq!(updates["tasks"].as_array().unwrap().len(), 1);
    assert!(site.get("/api/updates?after=1").unwrap().unwrap()["tasks"].as_array().unwrap().is_empty());
    assert!(site.get("/api/updates?after=x").is_err());
    let task = site.get(&format!("/api/tasks/{id}")).unwrap().unwrap();
    assert_eq!(task["history"].as_array().unwrap().len(), 1);
    assert_eq!(task["status"], "To do");
    assert!(site.get("/api/tasks/missing").unwrap().is_none());
    assert!(site.get("/api/nothing").unwrap().is_none());
    assert_eq!(site.get("/api/analytics").unwrap().unwrap()["groups"]["all"]["tasks"], 1);
    assert!(site.get("/api/deployment/requests/bad id").is_err());
}

#[test]
fn only_event_mode_long_polls_without_changes_are_parked() {
    let mut site = Site::new();
    let ws = site.ws.clone();
    let mut state = site.engine.state.clone();
    state.config.workspaces.insert(ws.clone(), crate::history::WorkspaceSettings { updates: crate::history::Updates::Events, ..Default::default() });
    site.engine.commit(state).unwrap();
    let sequence = site.engine.state.sequence;
    let other = site._temp.dir("other");
    let url = |workspace: &Path, after: u64, wait: u64| format!("/api/updates?workspace={}&after={after}&wait={wait}", encode(workspace.to_str().unwrap()));
    let local = host();
    let park = |site: &Site, method: Method, url: &str, host: &str| parkable(&request(method, url, "", &[("Host", host)]), &site.engine, PORT).is_some();
    assert!(park(&site, Method::Get, &url(&ws, sequence, 5), &local));
    assert!(!park(&site, Method::Get, &url(&ws, sequence - 1, 5), &local));
    assert!(!park(&site, Method::Get, &url(&ws, sequence, 0), &local));
    assert!(!park(&site, Method::Get, &url(&other, sequence, 5), &local));
    assert!(!park(&site, Method::Post, &url(&ws, sequence, 5), &local));
    assert!(!park(&site, Method::Get, &url(&ws, sequence, 5), "evil.example"));
    assert!(!park(&site, Method::Get, &format!("{}&bogus=1", url(&ws, sequence, 5)), &local));
    assert!(!park(&site, Method::Get, "/api/state?wait=5", &local));
}

#[test]
fn release_answers_expired_changed_or_all_parked_requests() {
    let mut site = Site::new();
    let parked = |after: u64, until: Instant| Parked {
        request: request(Method::Get, "/api/updates", "", &[("Host", &host())]),
        query: UpdateQuery { workspace: None, after: Some(after), wait: 5 },
        until,
    };
    let later = Instant::now() + Duration::from_secs(60);
    let waiting = release(vec![parked(0, Instant::now()), parked(0, later)], &site.engine, "", false);
    assert_eq!(waiting.len(), 1);
    site.engine.create(site.spec("Alpha")).unwrap();
    assert!(release(waiting, &site.engine, "", false).is_empty());
    let current = site.engine.state.sequence;
    let waiting = release(vec![parked(current, later)], &site.engine, "", false);
    assert_eq!(waiting.len(), 1);
    assert!(release(waiting, &site.engine, "", true).is_empty());
}

#[test]
fn deployment_requires_an_accepted_review_of_the_current_revision() {
    let task = |stage: &str, revision: u64, attempts: Value| -> Task {
        serde_json::from_value(json!({"id":"t","title":"T","outcome":"O","cwd":"C:\\","acceptance":["c"],"dependencies":[],"revision":revision,
            "stage":stage,"stopped":false,"attempts":attempts,"active":null,"feedback":[],"failure":"","retryAt":0})).unwrap()
    };
    let review = |revision: u64, status: &str| json!({"id":"a","revision":revision,"stage":"review","started":0,"ended":null,"result":{"status":status},"error":null});
    assert!(deployment_reviewed(&task("done", 1, json!([review(1, "accepted")]))));
    assert!(!deployment_reviewed(&task("done", 1, json!([review(0, "accepted")]))));
    assert!(!deployment_reviewed(&task("review", 1, json!([review(1, "accepted")]))));
    assert!(!deployment_reviewed(&task("done", 1, json!([review(1, "rejected")]))));
    assert!(!deployment_reviewed(&task("done", 0, json!([]))));
}

#[test]
fn deployment_event_reads_recorded_handoffs() {
    let temp = Temp::new();
    assert!(deployment_event(temp.path()).is_null());
    crate::write(&temp.path().join("deployment.json"), &json!({"status":"verified_live"})).unwrap();
    assert_eq!(deployment_event(temp.path())["status"], "verified_live");
    std::fs::write(temp.path().join("deployment.json"), "{broken").unwrap();
    assert!(deployment_event(temp.path()).is_null());
}
