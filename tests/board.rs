use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Server(Child, std::path::PathBuf);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

fn call(port: u16, method: &str, path: &str, body: Option<Value>) -> Value {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    serde_json::from_str(response.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

fn until(deadline: Duration, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < deadline, "condition not reached");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn board_runs_a_task_through_stub_workers_to_done() {
    let root = std::env::current_exe().unwrap().parent().unwrap().join(format!("board-it-{}", std::process::id()));
    std::fs::create_dir_all(root.join("work")).unwrap();
    let result = |name: &str, value: Value| {
        let path = root.join(name);
        std::fs::write(&path, value.to_string()).unwrap();
        vec!["cmd".to_owned(), "/c".into(), "copy".into(), "/y".into(), path.to_string_lossy().into_owned(), "%COORDINATOR_RESULT%".into()]
    };
    let implementation = result("completed.json", json!({"status":"completed","summary":"Implemented","checks":["stub"],"findings":[]}));
    let review = result("accepted.json", json!({"status":"accepted","summary":"Verified","checks":["stub"],"findings":[],"criteria":["passed"]}));
    let child = Command::new(env!("CARGO_BIN_EXE_agent-coordinator")).env("COORDINATOR_DATA", root.join("data")).env("PORT", "0").stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let server = Server(child, root.clone());
    let port_file = root.join("data").join("port.json");
    until(Duration::from_secs(20), || port_file.exists());
    let port: u16 = std::fs::read_to_string(&port_file).unwrap().trim().parse().unwrap();
    call(port, "PUT", "/api/config", Some(json!({"implementation":implementation,"review":review,"replan":implementation})));
    let rejected = call(port, "POST", "/api/tasks", Some(json!({"title":"Bad","outcome":"x","cwd":root.join("work"),"acceptance":["a","b","c","d"]})));
    assert!(rejected["error"].as_str().unwrap().contains("At most 3"));
    let task = call(port, "POST", "/api/tasks", Some(json!({"title":"Stubbed","outcome":"Run stubs","cwd":root.join("work"),"acceptance":["stub passes"]})));
    let id = task["id"].as_str().unwrap().to_owned();
    until(Duration::from_secs(60), || call(port, "GET", &format!("/api/tasks/{id}"), None)["status"] == "Done");
    let state = call(port, "GET", "/api/state", None);
    assert_eq!(state["tasks"][0]["attempts"].as_array().unwrap().len(), 2);
    drop(server);
}
