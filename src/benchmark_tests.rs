use super::*;
use crate::test_support::{Temp, merge};
use sha2::{Digest, Sha256};
use std::fs;

struct Bench {
    engine: Engine,
    group: PathBuf,
    temp: Temp,
}
fn sha(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn prompts(directory: &Path) {
    fs::write(directory.join("PROJECT_PLANNER.md"), "Planner instructions").unwrap();
    fs::write(directory.join("WORKER_PROTOCOL.md"), "Worker instructions").unwrap();
}
fn settings() -> Value {
    json!({"planner":"sol_6_1_light","worker":"sol_6_1_light","prompts":{
        "project_planner":{"candidate":"planner.md","sha256":sha("Planner instructions")},
        "worker_protocol":{"candidate":"worker.md","sha256":sha("Worker instructions")}}})
}
impl Bench {
    fn new() -> Self {
        let temp = Temp::new();
        let group = temp.dir("group");
        prompts(&group);
        Self { engine: temp.engine(), group, temp }
    }
    fn run_dir(&self, name: &str) -> PathBuf {
        let path = self.temp.dir(&format!("group/{name}"));
        prompts(&path);
        path
    }
    fn input(&self, run: &Path, id: &str) -> Value {
        json!({"workspace":run,"group":self.group,"settings":settings(),"run_id":id,"deadline_ms":now() + 600_000})
    }
    fn task(&mut self, cwd: &Path, title: &str, patch: Value) -> Result<String> {
        let value = merge(json!({"title":title,"outcome":"Outcome","cwd":cwd,"acceptance":["Criterion"],"standalone":true}), patch);
        Ok(self.engine.create(value)?["id"].as_str().unwrap().to_owned())
    }
    fn get(&self, id: &str) -> &Task {
        &self.engine.state.tasks[self.engine.task(id).unwrap()]
    }
    fn expire(&mut self, run: &Path) {
        let mut state = self.engine.state.clone();
        state.benchmark_runs.get_mut(run).unwrap().deadline_ms = now() - 1;
        self.engine.commit(state).unwrap();
    }
}

#[test]
fn registration_stores_the_run_and_group_settings_idempotently() {
    let mut bench = Bench::new();
    let run = bench.run_dir("run1");
    let input = bench.input(&run, "run-1");
    assert_eq!(bench.engine.register_benchmark(input.clone()).unwrap(), json!({"ok":true}));
    let stored = &bench.engine.state.benchmark_runs[&run];
    assert_eq!((stored.id.as_str(), stored.group.as_path()), ("run-1", bench.group.as_path()));
    assert!(bench.engine.state.config.workspaces[&bench.group].prompts.is_some());
    let sequence = bench.engine.state.sequence;
    bench.engine.register_benchmark(input.clone()).unwrap();
    assert_eq!(bench.engine.state.sequence, sequence);
    let later = merge(input.clone(), json!({"deadline_ms":input["deadline_ms"].as_u64().unwrap() + 1}));
    assert!(bench.engine.register_benchmark(later).unwrap_err().to_string().contains("registered differently"));
    let renamed = merge(input.clone(), json!({"run_id":"run-2"}));
    assert!(bench.engine.register_benchmark(renamed).unwrap_err().to_string().contains("registered differently"));
    let reloaded = Engine::new(bench.engine.directory.clone()).unwrap();
    assert!(reloaded.state.benchmark_runs[&run] == bench.engine.state.benchmark_runs[&run]);
}

#[test]
fn registration_validates_paths_deadline_prompts_and_overlap() {
    let mut bench = Bench::new();
    let run = bench.run_dir("run1");
    let outside = bench.temp.dir("outside");
    prompts(&outside);
    let bare = bench.temp.dir("group/bare");
    let input = bench.input(&run, "run-1");
    let error = |bench: &mut Bench, input: Value| bench.engine.register_benchmark(input).unwrap_err().to_string();
    assert!(error(&mut bench, merge(input.clone(), json!({"workspace":outside}))).contains("within its group"));
    assert!(error(&mut bench, merge(input.clone(), json!({"deadline_ms":now() - 1}))).contains("future deadline"));
    assert!(error(&mut bench, merge(input.clone(), json!({"run_id":""}))).contains("future deadline"));
    assert!(error(&mut bench, merge(input.clone(), json!({"workspace":bare}))).contains("os error 2"));
    assert!(error(&mut bench, merge(input.clone(), json!({"workspace":null}))).contains("workspace required"));
    assert!(error(&mut bench, merge(input.clone(), json!({"group":null}))).contains("group required"));
    assert!(error(&mut bench, merge(input.clone(), json!({"run_id":null}))).contains("run_id required"));
    assert!(error(&mut bench, merge(input.clone(), json!({"deadline_ms":"soon"}))).contains("deadline required"));
    assert!(error(&mut bench, merge(input.clone(), json!({"settings":{"limits":{"words":0}}}))).contains("Limits must be positive"));
    assert!(bench.engine.state.benchmark_runs.is_empty());
    bench.engine.register_benchmark(input).unwrap();
    let nested = bench.run_dir("run1/inner");
    let inner = bench.input(&nested, "run-2");
    assert!(error(&mut bench, inner).contains("Run paths overlap"));
    let whole = bench.input(&bench.group, "run-3");
    assert!(error(&mut bench, whole).contains("Run paths overlap"));
}

#[test]
fn run_tasks_belong_to_the_group_and_keys_stay_per_run() {
    let mut bench = Bench::new();
    let first = bench.run_dir("run1");
    let second = bench.run_dir("run2");
    for (run, id) in [(&first, "run-1"), (&second, "run-2")] {
        let input = bench.input(run, id);
        bench.engine.register_benchmark(input).unwrap();
    }
    let a = bench.task(&first, "Keyed", json!({"key":"k"})).unwrap();
    let b = bench.task(&second, "Keyed", json!({"key":"k"})).unwrap();
    assert_ne!(a, b);
    assert_eq!(bench.get(&a).workspace.as_deref(), Some(bench.group.as_path()));
    assert_eq!(bench.get(&b).workspace.as_deref(), Some(bench.group.as_path()));
    let again = bench.task(&first, "Keyed", json!({"key":"k","outcome":"Updated"})).unwrap();
    assert_eq!(again, a);
    assert_eq!(bench.engine.state.tasks.len(), 2);
    let scoped = bench.engine.updates(Some(&bench.group), None, "");
    assert_eq!(scoped["ids"].as_array().unwrap().len(), 2);
}

#[test]
fn expired_runs_stop_their_tasks_and_close_the_workspace() {
    let mut bench = Bench::new();
    let first = bench.run_dir("run1");
    let second = bench.run_dir("run2");
    let early = bench.task(&first, "Created before registration", json!({})).unwrap();
    assert_eq!(bench.get(&early).workspace.as_deref(), Some(first.as_path()));
    for (run, id) in [(&first, "run-1"), (&second, "run-2")] {
        let input = bench.input(run, id);
        bench.engine.register_benchmark(input).unwrap();
    }
    let open = bench.task(&second, "Other run", json!({})).unwrap();
    bench.engine.expire_benchmark_runs().unwrap();
    assert_eq!(bench.get(&early).workspace.as_deref(), Some(bench.group.as_path()));
    assert!(!bench.get(&early).stopped);
    bench.expire(&first);
    bench.engine.expire_benchmark_runs().unwrap();
    let task = bench.get(&early);
    assert!(task.stopped);
    let event = task.history.last().unwrap();
    assert_eq!((event.kind.as_str(), event.actor, event.evidence["reason"].as_str()), ("stopped", Actor::Scheduler, Some("Benchmark run ended")));
    assert!(expired(&bench.engine.state, task));
    assert!(!bench.get(&open).stopped);
    assert!(bench.task(&first, "Too late", json!({})).unwrap_err().to_string().contains("Benchmark workspace is closed"));
    assert!(bench.engine.edit(&early, json!({"outcome":"Changed"})).unwrap_err().to_string().contains("Benchmark workspace is closed"));
    let length = bench.get(&early).history.len();
    bench.engine.expire_benchmark_runs().unwrap();
    assert_eq!(bench.get(&early).history.len(), length);
}

#[test]
fn stopping_a_run_ends_it_immediately() {
    let mut bench = Bench::new();
    let run = bench.run_dir("run1");
    let input = bench.input(&run, "run-1");
    bench.engine.register_benchmark(input).unwrap();
    let id = bench.task(&run, "Running", json!({})).unwrap();
    let done = bench.task(&run, "Finished", json!({})).unwrap();
    let index = bench.engine.task(&done).unwrap();
    bench.engine.state.tasks[index].stage = "done".into();
    assert!(bench.engine.stop_benchmark("unknown").unwrap_err().to_string().contains("not registered"));
    bench.engine.stop_benchmark("run-1").unwrap();
    assert!(bench.engine.state.benchmark_runs[&run].deadline_ms <= now());
    assert!(bench.get(&id).stopped);
    assert!(!bench.get(&done).stopped);
    bench.engine.stop_benchmark("run-1").unwrap();
    assert!(!expired(&bench.engine.state, &serde_json::from_value(json!({"id":"x","title":"t","outcome":"o","cwd":bench.temp.path(),
        "acceptance":[],"dependencies":[],"stage":"implementation","stopped":false,"attempts":[],"active":null,"feedback":[],"failure":"","retryAt":0})).unwrap()));
}
