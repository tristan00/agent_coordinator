use crate::engine::Engine;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Temp(PathBuf);
impl Temp {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("coordinator-tests-{}", crate::engine::id()));
        fs::create_dir_all(&path).unwrap();
        let path = path.canonicalize().unwrap();
        assert!(!path.starts_with(Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize().unwrap()));
        Self(path)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn dir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }
    pub fn engine(&self) -> Engine {
        Engine::new(self.0.join("board")).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
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
