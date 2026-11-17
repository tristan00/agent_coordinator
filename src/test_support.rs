use crate::engine::Engine;
use serde_json::Value;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct Temp(PathBuf, RefCell<Option<(Child, String)>>);

fn executable() -> PathBuf {
    if let Some(path) = std::env::var_os("AGENT_TASKS_EXE") {
        return PathBuf::from(path);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("agent_tasks").join("target");
    ["release", "debug"].iter().map(|profile| root.join(profile).join("agent-tasks.exe")).find(|path| path.exists()).expect("Build agent_tasks first or set AGENT_TASKS_EXE")
}

impl Temp {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("coordinator-tests-{}", crate::engine::id()));
        fs::create_dir_all(&path).unwrap();
        let path = path.canonicalize().unwrap();
        assert!(!path.starts_with(Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize().unwrap()));
        Self(path, RefCell::new(None))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn dir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }
    pub fn tasks_url(&self) -> String {
        if let Some((_, url)) = self.1.borrow().as_ref() {
            return url.clone();
        }
        let data = self.0.join("task-service");
        fs::create_dir_all(&data).unwrap();
        let child = Command::new(executable()).arg("serve").env("AGENT_TASKS_DATA", &data).env("AGENT_TASKS_PORT", "0").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let port = loop {
            if let Ok(text) = fs::read_to_string(data.join("port.json"))
                && let Ok(port) = text.trim().parse::<u16>()
            {
                break port;
            }
            assert!(Instant::now() < deadline, "agent_tasks did not start");
            std::thread::sleep(Duration::from_millis(20));
        };
        let url = format!("http://127.0.0.1:{port}");
        *self.1.borrow_mut() = Some((child, url.clone()));
        url
    }
    pub fn open(&self, directory: PathBuf) -> crate::Result<Engine> {
        Engine::open(directory, Some(&self.tasks_url()))
    }
    pub fn engine(&self) -> Engine {
        self.open(self.0.join("board")).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        if let Some((mut child, _)) = self.1.borrow_mut().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let root = std::env::temp_dir().canonicalize().unwrap();
        if self.0.starts_with(&root) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
pub fn merge(mut base: Value, patch: Value) -> Value {
    for (key, value) in patch.as_object().unwrap() {
        base[key] = value.clone();
    }
    base
}
