use super::*;
use crate::test_support::{Temp, merge};

struct Board {
    engine: Engine,
    ws: PathBuf,
    temp: Temp,
}
impl Board {
    fn new() -> Self {
        let temp = Temp::new();
        let ws = temp.dir("workspace");
        Self { engine: temp.engine(), ws, temp }
    }
    fn spec(&self, title: &str) -> Value {
        json!({"title":title,"outcome":"The observable outcome holds","cwd":self.ws,"acceptance":["First criterion holds"],"standalone":true})
    }
    fn with(&self, title: &str, patch: Value) -> Value {
        merge(self.spec(title), patch)
    }
    fn container(&self, title: &str) -> Value {
        self.with(title, json!({"acceptance":[]}))
    }
    fn child(&self, title: &str, parent: &str) -> Value {
        let mut value = self.spec(title);
        value.as_object_mut().unwrap().remove("standalone");
        value["parent"] = json!(parent);
        value
    }
    fn create(&mut self, value: Value) -> String {
        self.engine.create(value).unwrap()["id"].as_str().unwrap().to_owned()
    }
    fn add(&mut self, title: &str) -> String {
        let value = self.spec(title);
        self.create(value)
    }
    fn fail(&mut self, value: Value) -> String {
        let before = self.engine.state.tasks.len();
        let error = self.engine.create(value).expect_err("create should fail").to_string();
        assert_eq!(self.engine.state.tasks.len(), before);
        error
    }
    fn edit_error(&mut self, id: &str, patch: Value) -> String {
        self.engine.edit(id, patch).expect_err("edit should fail").to_string()
    }
    fn task(&self, id: &str) -> &Task {
        &self.engine.state.tasks[self.engine.task(id).unwrap()]
    }
    fn kinds(&self, id: &str) -> Vec<String> {
        self.task(id).history.iter().map(|e| e.kind.clone()).collect()
    }
    fn waiting(&self, id: &str) -> Option<String> {
        self.engine.state.waiting_reason(self.task(id))
    }
    fn mutate(&mut self, change: impl FnOnce(&mut State)) {
        let mut state = self.engine.state.clone();
        change(&mut state);
        self.engine.commit(state).unwrap();
    }
    fn dispatch(&mut self, id: &str) -> String {
        let index = self.engine.task(id).unwrap();
        let attempt = super::id();
        fs::create_dir_all(self.engine.attempt_path(&attempt).join("scratch")).unwrap();
        let attempt_id = attempt.clone();
        self.mutate(move |state| {
            let task = &mut state.tasks[index];
            let stage = task.stage.clone();
            task.active = Some(attempt_id.clone());
            task.record("dispatched", Actor::Scheduler, "dispatch", json!({"stage":stage}));
            let evidence = AttemptEvidence { configuration: Some(json!({"scratch":true})), ..Default::default() };
            task.attempts.push(Attempt { id: attempt_id, revision: task.revision, stage, started: now(), ended: None, result: None, error: None, evidence });
        });
        attempt
    }
    fn directory(&self, id: &str) -> PathBuf {
        self.engine.attempt_path(self.task(id).active.as_deref().unwrap())
    }
    fn finish(&mut self, id: &str, exit: Value, result: Option<Value>) {
        let index = self.engine.task(id).unwrap();
        let directory = self.directory(id);
        if let Some(result) = result {
            write(&directory.join("result.json"), &result).unwrap();
        }
        write(&directory.join("exit.json"), &exit).unwrap();
        self.engine.poll_attempt(index).unwrap();
        assert!(self.task(id).active.is_none());
    }
    fn complete(&mut self, id: &str, result: Value) {
        self.finish(id, json!({"code":0,"ended":now()}), Some(result));
    }
    fn run(&mut self, id: &str, result: Value) -> String {
        let attempt = self.dispatch(id);
        self.complete(id, result);
        attempt
    }
    fn accept(&mut self, id: &str) {
        let passed = vec!["passed"; self.task(id).acceptance.len()];
        self.run(id, outcome("completed", &passed));
        self.run(id, outcome("accepted", &passed));
        assert_eq!(self.task(id).stage, "done");
    }
    fn latest(&self, id: &str) -> &Attempt {
        self.task(id).attempts.last().unwrap()
    }
}
fn outcome(status: &str, criteria: &[&str]) -> Value {
    json!({"status":status,"summary":"Worker summary","criteria":criteria,"findings":["A finding"]})
}
fn rejected() -> Value {
    outcome("rejected", &["failed"])
}

#[test]
fn stage_transitions_and_worker_statuses_define_the_pipeline() {
    assert_eq!(next_stage("completed"), Some("review"));
    assert_eq!(next_stage("accepted"), Some("done"));
    assert_eq!(next_stage("rejected"), Some("implementation"));
    assert_eq!(next_stage("blocked"), Some("replan"));
    assert_eq!(next_stage("held"), None);
    assert_eq!(worker_statuses("implementation"), ["completed", "blocked"]);
    assert_eq!(worker_statuses("review"), ["accepted", "rejected", "blocked"]);
    assert_eq!(worker_statuses("replan"), ["replanned", "held"]);
    assert_eq!(normalized("\u{feff}  a \n b  "), "a b");
}

#[test]
fn minimal_standalone_task_is_created_in_its_canonical_workspace() {
    let mut board = Board::new();
    let created = board.engine.create(board.with("  Build the parser  ", json!({"actor":"human"}))).unwrap();
    assert_eq!(created["status"], "To do");
    assert_eq!(created["container"], false);
    assert_eq!(created["definitionWords"], 3 + 4 + 3);
    assert!(created.get("history").is_none());
    let id = created["id"].as_str().unwrap();
    let task = board.task(id);
    assert_eq!(task.title, "Build the parser");
    assert_eq!(task.workspace.as_deref(), Some(board.ws.as_path()));
    assert_eq!((task.stage.as_str(), task.revision, task.status()), ("implementation", 0, "To do"));
    assert_eq!(task.history.len(), 1);
    assert_eq!((task.history[0].kind.as_str(), task.history[0].actor), ("created", Actor::Human));
    assert!(read::<Value>(&board.engine.directory.join("state.json")).unwrap()["tasks"][0]["id"] == id);
}

#[test]
fn definitions_require_title_outcome_and_an_existing_absolute_cwd() {
    let mut board = Board::new();
    assert!(board.fail(board.spec("   ")).contains("Title, outcome and cwd are required"));
    assert!(board.fail(board.with("Title", json!({"outcome":""}))).contains("Title, outcome and cwd are required"));
    let mut missing = board.spec("Title");
    missing.as_object_mut().unwrap().remove("cwd");
    assert!(board.fail(missing).contains("cwd is required"));
    assert!(board.fail(board.with("Title", json!({"cwd":"relative\\dir"}))).contains("existing absolute directory"));
    assert!(board.fail(board.with("Title", json!({"cwd":board.ws.join("absent")}))).contains("existing absolute directory"));
}

#[test]
fn title_word_limit_is_inclusive() {
    let mut board = Board::new();
    board.add(&"word ".repeat(12));
    assert!(board.fail(board.spec(&"word ".repeat(13))).contains("Title exceeds 12 words"));
}

#[test]
fn criteria_count_is_limited_and_criteria_must_not_be_blank() {
    let mut board = Board::new();
    board.create(board.with("Three criteria", json!({"acceptance":["a holds","b holds","c holds"]})));
    assert!(board.fail(board.with("Four criteria", json!({"acceptance":["a","b","c","d"]}))).contains("At most 3 acceptance criteria"));
    assert!(board.fail(board.with("Blank criterion", json!({"acceptance":["a","  "]}))).contains("must not be blank"));
}

#[test]
fn definition_word_limit_counts_title_outcome_and_criteria() {
    let mut board = Board::new();
    let fits = board.with("Two words", json!({"acceptance":["Three word criterion"],"outcome":"word ".repeat(115)}));
    let id = board.create(fits);
    assert_eq!(board.task(&id).definition_words(), 120);
    let over = board.with("Two words", json!({"acceptance":["Three word criterion"],"outcome":"word ".repeat(116)}));
    assert!(board.fail(over).contains("Definition exceeds 120 words"));
}

#[test]
fn coordination_prose_is_rejected_in_every_definition_field() {
    let mut board = Board::new();
    assert!(board.fail(board.spec("Follow AGENTS.md")).contains("belong in relations"));
    assert!(board.fail(board.with("Title", json!({"outcome":"After 1731456789012-1-0 lands"}))).contains("belong in relations"));
    assert!(board.fail(board.with("Title", json!({"acceptance":["a".repeat(64)]}))).contains("belong in relations"));
}

#[test]
fn size_artifact_priority_and_capability_rules() {
    let mut board = Board::new();
    assert!(board.fail(board.with("Large leaf", json!({"attributes":{"size":"large"}}))).contains("Large work must be a container"));
    board.create(merge(board.container("Large container"), json!({"attributes":{"size":"large"}})));
    assert!(board.fail(merge(board.container("Artifact container"), json!({"attributes":{"artifact":"report"}}))).contains("Containers produce no artifact"));
    assert!(board.fail(board.with("Priority", json!({"attributes":{"priority":9}}))).contains("Priority must be"));
    assert!(board.fail(board.with("Deploy", json!({"attributes":{"deploy":true}}))).contains("deploy requires artifact candidate"));
    assert!(board.fail(board.with("Gpu", json!({"attributes":{"needs":["gpu"]}}))).contains("No worker provides [\"gpu\"]"));
    let id = board.create(board.with("Network", json!({"attributes":{"needs":["network","network"],"lane":"docs"}})));
    assert_eq!(board.task(&id).attributes.needs.len(), 1);
    assert!(board.task(&id).attributes.lane == Lane::Docs);
}

#[test]
fn workspace_limits_override_defaults_only_for_their_workspace() {
    let mut board = Board::new();
    let other = board.temp.dir("other");
    let ws = board.ws.clone();
    board.mutate(|state| {
        let limits = history::Limits { criteria: 1, title_words: 3, words: 10, ..Default::default() };
        state.config.workspaces.insert(ws, history::WorkspaceSettings { limits, ..Default::default() });
    });
    assert!(board.fail(board.with("Two criteria", json!({"acceptance":["a","b"]}))).contains("At most 1 acceptance criteria"));
    assert!(board.fail(board.spec("Four words title here")).contains("Title exceeds 3 words"));
    assert!(board.fail(board.with("Short", json!({"outcome":"word ".repeat(9)}))).contains("Definition exceeds 10 words"));
    board.create(board.with("Four words title here", json!({"cwd":other,"acceptance":["a","b"]})));
    let id = board.add("Short");
    let limits = board.engine.state.limits(board.task(&id));
    assert_eq!((limits.criteria, limits.summary_words), (1, 40));
}

#[test]
fn unknown_fields_and_attributes_are_rejected() {
    let mut board = Board::new();
    let error = board.fail(board.with("Title", json!({"notes":"free text"})));
    assert!(error.contains("unknown field `notes`") && error.contains("coordination belongs in typed fields"), "{error}");
    assert!(board.fail(board.with("Title", json!({"attributes":{"color":"red"}}))).contains("Unknown attribute"));
    assert!(board.fail(board.with("Title", json!({"attributes":{"languages":["cobol"]}}))).contains("Unknown classification cobol"));
    let id = board.add("Target");
    assert!(board.fail(board.with("Title", json!({"dependencies":[{"target":id,"reason":"uses_api","why":"x"}]}))).contains("unknown field `why`"));
    assert!(board.edit_error(&id, json!({"status":"done"})).contains("unknown field `status`"));
}

#[test]
fn grouping_requires_exactly_one_of_parent_or_standalone() {
    let mut board = Board::new();
    let container = board.create(board.container("Group"));
    let mut neither = board.spec("Neither");
    neither.as_object_mut().unwrap().remove("standalone");
    assert!(board.fail(neither).contains("Choose exactly one grouping"));
    assert!(board.fail(board.with("Both", json!({"parent":container}))).contains("Choose exactly one grouping"));
    assert!(board.fail(board.with("False", json!({"standalone":false}))).contains("Choose exactly one grouping"));
    assert!(board.fail(board.with("Null", json!({"standalone":null}))).contains("grouping"));
    let child = board.create(board.child("Child", &container));
    assert_eq!(board.task(&child).parent.as_deref(), Some(container.as_str()));
    assert!(!board.task(&child).standalone);
    assert!(board.edit_error(&child, json!({"parent":null})).contains("Choose exactly one grouping"));
    board.engine.edit(&child, json!({"parent":null,"standalone":true})).unwrap();
    assert!(board.task(&child).standalone && board.task(&child).parent.is_none());
}

#[test]
fn api_actor_must_be_human_or_planner() {
    let mut board = Board::new();
    assert!(board.fail(board.with("Title", json!({"actor":"worker"}))).contains("actor must be human or planner"));
    let id = board.add("Planner default");
    assert_eq!(board.task(&id).history[0].actor, Actor::Planner);
}

#[test]
fn dependencies_require_a_known_typed_reason() {
    let mut board = Board::new();
    let target = board.add("Target");
    assert!(board.fail(board.with("Untyped", json!({"dependencies":[{"target":target}]}))).contains("typed reason"));
    assert!(board.fail(board.with("Null reason", json!({"dependencies":[{"target":target,"reason":null}]}))).contains("typed reason"));
    assert!(board.fail(board.with("Unknown reason", json!({"dependencies":[{"target":target,"reason":"because"}]}))).contains("use after, or nothing"));
    assert!(board.fail(board.with("String", json!({"dependencies":[target]}))).contains("use after, or nothing"));
    for reason in DEPENDENCY_REASONS {
        let id = board.create(board.with("Typed", json!({"dependencies":[{"target":target,"reason":reason}]})));
        assert_eq!(board.task(&id).dependencies, [Dependency { target: target.clone(), reason: Some(reason) }]);
    }
}

#[test]
fn keyed_create_revises_the_task_with_the_same_key_in_the_workspace() {
    let mut board = Board::new();
    let first = board.create(board.with("Keyed", json!({"key":" alpha "})));
    assert_eq!(board.task(&first).key.as_deref(), Some("alpha"));
    let again = board.create(board.with("Keyed", json!({"key":"alpha"})));
    assert_eq!(again, first);
    assert_eq!(board.task(&first).revision, 0);
    let revised = board.create(board.with("Keyed", json!({"key":"alpha","outcome":"A different outcome"})));
    assert_eq!(revised, first);
    assert_eq!(board.engine.state.tasks.len(), 1);
    assert_eq!((board.task(&first).revision, board.task(&first).outcome.as_str()), (1, "A different outcome"));
    assert_eq!(board.task(&first).history.last().unwrap().kind, "revised");
    let other = board.temp.dir("other");
    let elsewhere = board.create(board.with("Keyed", json!({"key":"alpha","cwd":other})));
    assert_ne!(elsewhere, first);
    assert!(board.fail(board.with("Blank key", json!({"key":"  "}))).contains("key must not be blank"));
    assert!(board.fail(board.with("Keyed", json!({"key":"alpha","supersedes":[]}))).contains("supersedes applies only when creating"));
}

#[test]
fn writable_paths_resolve_inside_the_workspace_and_reject_parent_components() {
    let mut board = Board::new();
    let id = board.create(board.with("Writable", json!({"writable":["sub/dir","sub/dir","sub\\dir"]})));
    assert_eq!(board.task(&id).writable, [board.ws.join("sub").join("dir")]);
    assert!(board.fail(board.with("Escape", json!({"writable":["../escape"]}))).contains("without '..'"));
    assert!(board.fail(board.with("Empty", json!({"writable":[""]}))).contains("without '..'"));
    let absolute = board.temp.dir("shared");
    let id = board.create(board.with("Absolute", json!({"writable":[absolute]})));
    assert_eq!(board.task(&id).writable, [absolute]);
}

#[test]
fn scope_paths_cover_case_insensitively_and_ignore_trailing_dots() {
    assert!(covers(Path::new("C:\\Work\\Repo"), Path::new("c:\\work\\repo\\src\\main.rs")));
    assert!(covers(Path::new("C:\\Work\\Repo."), Path::new("C:\\Work\\Repo\\x")));
    assert!(covers(Path::new("C:\\Work\\Repo"), Path::new("C:\\Work\\Repo")));
    assert!(!covers(Path::new("C:\\Work\\Repo"), Path::new("C:\\Work\\Repository")));
    assert!(!covers(Path::new("C:\\Work\\Repo\\src"), Path::new("C:\\Work\\Repo")));
    let temp = Temp::new();
    let missing = temp.path().join("a").join("b");
    assert_eq!(resolve_scope_path(&missing).unwrap(), missing);
    assert_eq!(resolve_scope_path(temp.path()).unwrap(), temp.path());
}

#[test]
fn relations_reject_self_unknown_and_duplicate_targets() {
    let mut board = Board::new();
    let a = board.add("Alpha");
    assert!(board.edit_error(&a, json!({"after":[a]})).contains("cannot relate to itself"));
    assert!(board.edit_error(&a, json!({"conflicts":[a]})).contains("cannot relate to itself"));
    assert!(board.fail(board.with("Unknown", json!({"after":["missing"]}))).contains("Related task not found: missing"));
    assert!(board.fail(board.with("Unknown", json!({"related":["missing"]}))).contains("Related task not found"));
    assert!(board.fail(board.with("Unknown", json!({"conflicts":["missing"]}))).contains("Related task not found"));
    let duplicate = json!([{"target":a,"reason":"uses_api"},{"target":a,"reason":"uses_data"}]);
    assert!(board.fail(board.with("Duplicate", json!({"dependencies":duplicate}))).contains("Duplicate dependency target"));
}

#[test]
fn parent_must_be_an_open_container() {
    let mut board = Board::new();
    let leaf = board.add("Leaf");
    assert!(board.fail(board.child("Child", &leaf)).contains("Parent must be a container"));
    let container = board.create(board.container("Group"));
    let mover = board.add("Mover");
    board.mutate(|state| state.tasks.iter_mut().filter(|t| t.id == container).for_each(|t| t.stage = "done".into()));
    assert!(board.fail(board.child("Late child", &container)).contains("Cannot add work under a completed container"));
    assert!(board.edit_error(&mover, json!({"parent":container,"standalone":false})).contains("Cannot add work under a completed container"));
}

#[test]
fn parent_cycles_and_children_waiting_on_their_container_are_rejected() {
    let mut board = Board::new();
    let outer = board.create(board.container("Outer"));
    let inner = board.create(merge(board.child("Inner", &outer), json!({"acceptance":[]})));
    assert!(board.edit_error(&outer, json!({"parent":inner,"standalone":false})).contains("Parent cycle"));
    assert!(board.fail(merge(board.child("Waits", &outer), json!({"after":[outer]}))).contains("cannot wait on its own container"));
    let dependency = json!([{"target":outer,"reason":"uses_api"}]);
    assert!(board.fail(merge(board.child("Grandchild", &inner), json!({"dependencies":dependency}))).contains("cannot wait on its own container"));
}

#[test]
fn dependency_and_ordering_cycles_are_rejected() {
    let mut board = Board::new();
    let a = board.add("Alpha");
    let b = board.create(board.with("Beta", json!({"after":[a]})));
    let c = board.create(board.with("Gamma", json!({"dependencies":[{"target":b,"reason":"uses_data"}]})));
    assert!(board.edit_error(&a, json!({"after":[b]})).contains("Dependency or ordering cycle"));
    assert!(board.edit_error(&a, json!({"dependencies":[{"target":c,"reason":"uses_api"}]})).contains("Dependency or ordering cycle"));
    board.engine.edit(&a, json!({"conflicts":[c],"related":[b]})).unwrap();
    assert_eq!(board.task(&a).conflicts, [c]);
}

#[test]
fn supersedes_rewires_dependents_and_archives_the_old_task() {
    let mut board = Board::new();
    let old = board.add("Old approach");
    let dependent = board.create(board.with("Dependent", json!({"dependencies":[{"target":old,"reason":"uses_api"}],"after":[old]})));
    let replacement = board.create(board.with("New approach", json!({"supersedes":[old]})));
    assert!(board.engine.task(&old).is_err());
    let archived = board.engine.state.archived.iter().find(|t| t.id == old).unwrap();
    assert_eq!(archived.history.last().unwrap().kind, "superseded");
    assert_eq!(archived.history.last().unwrap().evidence["by"], json!(replacement));
    let task = board.task(&dependent);
    assert_eq!((task.dependencies[0].target.as_str(), task.after[0].as_str()), (replacement.as_str(), replacement.as_str()));
    assert!(board.kinds(&dependent).contains(&"rewired".into()));
    assert_eq!(board.task(&replacement).history[0].evidence["supersedes"], json!([old]));
}

#[test]
fn supersedes_requires_idle_childless_existing_tasks() {
    let mut board = Board::new();
    let busy = board.add("Busy");
    board.dispatch(&busy);
    assert!(board.fail(board.with("Replace busy", json!({"supersedes":[busy]}))).contains("must be idle and have no children"));
    let container = board.create(board.container("Group"));
    board.create(board.child("Child", &container));
    assert!(board.fail(board.with("Replace group", json!({"supersedes":[container]}))).contains("must be idle and have no children"));
    assert!(board.fail(board.with("Replace missing", json!({"supersedes":["missing"]}))).contains("Task not found"));
    assert!(board.edit_error(&busy, json!({"supersedes":[container]})).contains("supersedes applies only when creating"));
}

#[test]
fn remove_archives_leaves_and_refuses_referenced_or_active_tasks() {
    let mut board = Board::new();
    let a = board.add("Alpha");
    let b = board.create(board.with("Beta", json!({"after":[a]})));
    let c = board.create(board.with("Gamma", json!({"conflicts":[a],"related":[a]})));
    assert!(board.engine.remove(&a, Actor::Human).unwrap_err().to_string().contains("children or dependents"));
    board.engine.remove(&b, Actor::Human).unwrap();
    assert_eq!(board.engine.state.archived[0].history.last().unwrap().kind, "abandoned");
    board.engine.remove(&a, Actor::Planner).unwrap();
    assert!(board.task(&c).conflicts.is_empty() && board.task(&c).related.is_empty());
    board.accept(&c);
    board.engine.remove(&c, Actor::Human).unwrap();
    assert_eq!(board.engine.state.archived.last().unwrap().history.last().unwrap().kind, "archived");
    let d = board.add("Delta");
    board.dispatch(&d);
    assert!(board.engine.remove(&d, Actor::Human).unwrap_err().to_string().contains("active worker"));
    assert!(board.engine.remove("missing", Actor::Human).is_err());
}

#[test]
fn waiting_reasons_cover_dependencies_ordering_conflicts_and_holds() {
    let mut board = Board::new();
    let a = board.add("Alpha");
    let b = board.create(board.with("Beta", json!({"dependencies":[{"target":a,"reason":"uses_api"}]})));
    let c = board.create(board.with("Gamma", json!({"after":[a]})));
    let d = board.create(board.with("Delta", json!({"conflicts":[a]})));
    assert_eq!(board.waiting(&a), None);
    assert_eq!(board.waiting(&b).as_deref(), Some("Waiting for result from Alpha"));
    assert_eq!(board.waiting(&c).as_deref(), Some("Waiting for Alpha to settle"));
    assert_eq!(board.waiting(&d), None);
    board.dispatch(&a);
    assert_eq!(board.waiting(&d).as_deref(), Some("Conflicts with running Alpha"));
    assert_eq!(board.waiting(&a), None);
    board.finish(&a, json!({"stopped":true,"code":1,"ended":now()}), None);
    board.engine.control_as(true, Some(&a), Actor::Human, "pause alpha").unwrap();
    assert_eq!(board.waiting(&c), None);
    assert_eq!(board.waiting(&b).as_deref(), Some("Waiting for result from Alpha"));
    let future = now() + 60_000;
    board.mutate(|state| state.hold_until = future);
    assert_eq!(board.waiting(&c), Some(format!("Usage limit; dispatch resumes at {future}")));
    board.mutate(|state| {
        state.hold_until = 0;
        state.tasks.iter_mut().filter(|t| t.id == c).for_each(|t| t.retry_at = future);
    });
    assert_eq!(board.waiting(&c), Some(format!("Retry scheduled at {future}")));
    board.engine.control_as(true, None, Actor::Human, "pause board").unwrap();
    assert_eq!(board.waiting(&b), None);
}

#[test]
fn children_inherit_their_containers_prerequisites() {
    let mut board = Board::new();
    let prerequisite = board.add("Prerequisite");
    let container = board.create(merge(board.container("Group"), json!({"dependencies":[{"target":prerequisite,"reason":"uses_schema"}]})));
    let child = board.create(board.child("Child", &container));
    assert_eq!(board.waiting(&child).as_deref(), Some("Waiting for result from Prerequisite"));
    board.accept(&prerequisite);
    assert_eq!(board.waiting(&child), None);
}

#[test]
fn containers_wait_for_children_and_settle_when_children_finish() {
    let mut board = Board::new();
    let container = board.create(board.container("Group"));
    assert_eq!(board.waiting(&container).as_deref(), Some("Container has no child tasks"));
    let x = board.create(board.child("First child", &container));
    let y = board.create(board.child("Second child", &container));
    assert_eq!(board.waiting(&container).as_deref(), Some("Waiting for child tasks"));
    board.accept(&x);
    assert!(!board.engine.state.clone().settle_containers());
    board.accept(&y);
    let mut state = board.engine.state.clone();
    assert!(state.settle_containers());
    board.engine.commit(state).unwrap();
    assert_eq!(board.task(&container).stage, "done");
    assert!(board.kinds(&container).contains(&"children_done".into()));
    assert!(!board.engine.state.clone().settle_containers());
}

#[test]
fn editing_a_done_child_reopens_its_done_container() {
    let mut board = Board::new();
    let container = board.create(board.container("Group"));
    let child = board.create(board.child("Only child", &container));
    board.accept(&child);
    let mut state = board.engine.state.clone();
    state.settle_containers();
    board.engine.commit(state).unwrap();
    board.engine.edit(&child, json!({"outcome":"A corrected outcome"})).unwrap();
    let mut state = board.engine.state.clone();
    assert!(state.settle_containers());
    assert_eq!(state.tasks[state.index(&container).unwrap()].stage, "implementation");
}

#[test]
fn editing_a_prerequisite_reopens_its_dependent_inside_a_done_container() {
    let mut board = Board::new();
    let prerequisite = board.add("Prerequisite");
    board.accept(&prerequisite);
    let container = board.create(board.container("Group"));
    let mut child = board.child("Consumer", &container);
    child["dependencies"] = json!([{"target":prerequisite,"reason":"uses_api"}]);
    let child = board.create(child);
    board.accept(&child);
    let mut state = board.engine.state.clone();
    state.settle_containers();
    board.engine.commit(state).unwrap();
    assert_eq!(board.task(&container).stage, "done");
    board.engine.edit(&prerequisite, json!({"outcome":"A corrected prerequisite"})).unwrap();
    assert_eq!(board.task(&child).stage, "implementation");
    let mut state = board.engine.state.clone();
    assert!(state.settle_containers());
    assert_eq!(state.tasks[state.index(&container).unwrap()].stage, "implementation");
}

#[test]
fn implementation_then_review_acceptance_reaches_done() {
    let mut board = Board::new();
    let id = board.add("Alpha");
    board.run(&id, outcome("completed", &["passed"]));
    assert_eq!((board.task(&id).stage.as_str(), board.task(&id).status()), ("review", "Review"));
    assert_eq!(board.task(&id).feedback, ["A finding"]);
    board.run(&id, outcome("accepted", &["passed"]));
    let task = board.task(&id);
    assert_eq!((task.stage.as_str(), task.status(), task.strikes()), ("done", "Done", 0));
    let kinds = board.kinds(&id);
    for kind in ["created", "dispatched", "completed", "phase_changed", "accepted"] {
        assert!(kinds.contains(&kind.to_string()), "{kind}");
    }
    let value = board.engine.task_value(board.task(&id), true).unwrap();
    assert!(value["analytics"]["acceptedAt"].is_u64());
    assert!(value["history"].is_array());
    assert!(board.engine.notices.is_empty() && board.engine.pending_deploy.is_none());
    assert!(!board.engine.attempt_path(&board.latest(&id).id).join("scratch").exists());
}

#[test]
fn two_rejections_of_one_criterion_send_the_task_to_replan() {
    let mut board = Board::new();
    let id = board.add("Alpha");
    board.run(&id, outcome("completed", &["passed"]));
    board.run(&id, rejected());
    let task = board.task(&id);
    assert_eq!((task.stage.as_str(), task.strikes()), ("implementation", 1));
    assert_eq!(task.feedback, ["Failed criterion: First criterion holds", "A finding"]);
    board.run(&id, outcome("completed", &["passed"]));
    board.run(&id, rejected());
    let task = board.task(&id);
    assert_eq!((task.stage.as_str(), task.strikes(), task.blocked, task.status()), ("replan", 2, false, "In progress"));
    assert_eq!(task.brief()["strikes"], 2);
}

fn plan(acceptance: &str) -> Value {
    json!({"status":"replanned","summary":"New approach","plan":{"title":"Revised title","outcome":"Revised outcome","acceptance":[acceptance],"lane":"code","needs":[],"size":"small"}})
}

#[test]
fn successful_replan_revises_the_task_and_a_second_failure_is_held() {
    let mut board = Board::new();
    let id = board.add("Alpha");
    for _ in 0..2 {
        board.run(&id, outcome("completed", &["passed"]));
        board.run(&id, rejected());
    }
    board.run(&id, plan("A different criterion holds"));
    let task = board.task(&id);
    assert_eq!((task.stage.as_str(), task.revision, task.replanned, task.strikes()), ("implementation", 1, true, 0));
    assert_eq!((task.title.as_str(), task.acceptance[0].as_str()), ("Revised title", "A different criterion holds"));
    assert!(task.inherited.is_empty());
    assert!(task.history.iter().any(|e| e.kind == "revised" && e.actor == Actor::Planner));
    board.run(&id, json!({"status":"completed","summary":"Could not","criteria":["blocked"],"findings":["Missing access"]}));
    let task = board.task(&id);
    assert_eq!((task.stage.as_str(), task.blocked), ("replan", true));
    assert_eq!(task.failure, "Replanned work failed again; a decision is needed");
    assert_eq!(board.engine.notices.last().unwrap()["event"], "held");
}

#[test]
fn replan_keeping_a_criterion_with_one_failure_inherits_the_strike() {
    let mut board = Board::new();
    let id = board.create(board.with("Alpha", json!({"acceptance":["Kept criterion","Dropped criterion"]})));
    board.run(&id, outcome("completed", &["passed", "passed"]));
    board.run(&id, outcome("rejected", &["failed", "passed"]));
    board.run(&id, outcome("completed", &["passed", "passed"]));
    board.run(&id, outcome("rejected", &["passed", "failed"]));
    assert_eq!(board.task(&id).stage, "implementation");
    board.engine.request_replan(&id, json!({"reason":"Try another way"})).unwrap();
    board.run(&id, plan("Kept criterion"));
    let task = board.task(&id);
    assert_eq!(task.inherited, BTreeMap::from([("Kept criterion".to_string(), 1)]));
    assert_eq!(task.strikes(), 1);
    board.run(&id, outcome("completed", &["passed"]));
    board.run(&id, rejected());
    assert_eq!((board.task(&id).stage.as_str(), board.task(&id).blocked), ("replan", true));
}

#[test]
fn replans_repeating_a_twice_failed_criterion_are_refused_then_held() {
    let mut board = Board::new();
    let id = board.add("Alpha");
    for _ in 0..2 {
        board.run(&id, outcome("completed", &["passed"]));
        board.run(&id, rejected());
    }
    board.run(&id, plan("First criterion holds"));
    let task = board.task(&id);
    assert_eq!((task.stage.as_str(), task.blocked, task.failed_replans()), ("replan", false, 1));
    assert!(task.feedback[0].starts_with("Plan repeats a criterion"));
    assert!(board.latest(&id).result.as_ref().unwrap()["planError"].is_string());
    board.run(&id, plan("First   criterion holds"));
    let task = board.task(&id);
    assert!(task.blocked);
    assert_eq!(task.failure, "Replanning failed twice; a decision is needed");
    board.mutate(|state| state.tasks[0].attempts.iter_mut().for_each(|a| a.started -= 10_000));
    board.engine.control_as(false, Some(&id), Actor::Human, "decided").unwrap();
    assert!(!board.task(&id).blocked);
    board.run(&id, plan("First criterion holds"));
    assert!(!board.task(&id).blocked);
}

#[test]
fn held_results_block_the_task_and_notify() {
    let mut board = Board::new();
    let id = board.add("Alpha");
    board.engine.request_replan(&id, json!({})).unwrap();
    board.run(&id, json!({"status":"held","summary":"Needs a product decision"}));
    assert!(board.latest(&id).error.as_deref().unwrap().contains("require findings"));
    assert!(!board.task(&id).blocked);
    board.run(&id, json!({"status":"held","summary":"Needs a product decision","findings":["Two viable options"]}));
    let task = board.task(&id);
    assert!(task.blocked);
    assert_eq!(task.failure, "Needs a product decision");
    let notice = board.engine.notices.last().unwrap();
    assert_eq!((notice["event"].as_str(), notice["taskId"].as_str()), (Some("held"), Some(id.as_str())));
}

#[test]
fn result_validation_coerces_inconsistent_statuses() {
    let mut board = Board::new();
    let unmet = board.add("Unmet");
    board.run(&unmet, outcome("completed", &["failed"]));
    assert_eq!(board.task(&unmet).stage, "replan");
    assert_eq!(board.latest(&unmet).status(), Some("blocked"));
    assert_eq!(board.task(&unmet).feedback[0], "Unmet criterion (failed): First criterion holds");
    let accepted_failed = board.add("Accepted failed");
    board.run(&accepted_failed, outcome("completed", &["passed"]));
    board.run(&accepted_failed, outcome("accepted", &["failed"]));
    assert_eq!((board.latest(&accepted_failed).status(), board.task(&accepted_failed).stage.as_str()), (Some("rejected"), "implementation"));
    let accepted_blocked = board.add("Accepted blocked");
    board.run(&accepted_blocked, outcome("completed", &["passed"]));
    board.run(&accepted_blocked, outcome("accepted", &["blocked"]));
    assert_eq!((board.latest(&accepted_blocked).status(), board.task(&accepted_blocked).stage.as_str()), (Some("blocked"), "replan"));
    let empty_rejection = board.add("Empty rejection");
    board.run(&empty_rejection, outcome("completed", &["passed"]));
    board.run(&empty_rejection, outcome("rejected", &["passed"]));
    assert!(board.latest(&empty_rejection).error.as_deref().unwrap().contains("at least one criterion failed"));
    assert_eq!(board.task(&empty_rejection).stage, "review");
}

#[test]
fn results_are_clipped_to_workspace_limits() {
    let mut board = Board::new();
    let id = board.add("Alpha");
    let findings: Vec<String> = (0..5).map(|i| format!("finding {i} {}", "word ".repeat(40))).collect();
    board.run(&id, json!({"status":"completed","summary":"word ".repeat(50),"criteria":["passed"],"findings":findings,"checks":["", "ok"]}));
    let result = board.latest(&id).result.clone().unwrap();
    assert_eq!(history::words(result["summary"].as_str().unwrap()), 40);
    assert_eq!(result["findings"].as_array().unwrap().len(), 3);
    assert_eq!(history::words(result["findings"][0].as_str().unwrap()), 30);
    assert_eq!(result["checks"], json!(["ok"]));
}

#[test]
fn invalid_results_record_errors_and_schedule_backoff() {
    let mut board = Board::new();
    let id = board.add("Alpha");
    board.dispatch(&id);
    board.finish(&id, json!({"code":1,"ended":now()}), None);
    let task = board.task(&id);
    assert!(task.failure.contains("Worker exited 1"));
    assert!(task.retry_at > now());
    assert!(board.waiting(&id).unwrap().starts_with("Retry scheduled at"));
    assert!(board.kinds(&id).contains(&"attempt_failed".into()));
    board.dispatch(&id);
    board.finish(&id, json!({"code":1,"ended":now()}), None);
    assert!(board.task(&id).retry_at >= now() + 1_500);
    let cases = [
        (json!({"status":"done","summary":"s","criteria":["passed"]}), "Invalid worker status"),
        (json!({"status":"completed","summary":"  ","criteria":["passed"]}), "Worker summary is required"),
        (json!({"status":"completed","summary":"s"}), "Worker must report criteria outcomes"),
        (json!({"status":"completed","summary":"s","criteria":["passed","passed"]}), "one outcome per acceptance criterion"),
        (json!({"status":"completed","summary":"s","criteria":["skipped"]}), "one outcome per acceptance criterion"),
        (json!({"status":"blocked","summary":"s","criteria":["blocked"]}), "require findings"),
    ];
    for (result, message) in cases {
        board.run(&id, result);
        assert!(board.latest(&id).error.as_deref().unwrap().contains(message), "{message}");
        assert_eq!(board.task(&id).stage, "implementation");
    }
    board.dispatch(&id);
    board.finish(&id, json!({"error":"spawn failed","ended":now()}), None);
    assert_eq!(board.latest(&id).error.as_deref(), Some("spawn failed"));
    board.dispatch(&id);
    board.finish(&id, json!({"code":0,"ended":now()}), None);
    assert!(board.latest(&id).error.is_some());
    board.run(&id, outcome("completed", &["passed"]));
    assert_eq!((board.task(&id).failure.as_str(), board.task(&id).retry_at), ("", 0));
}

#[test]
fn stopped_attempts_and_stale_revisions_do_not_advance() {
    let mut board = Board::new();
    let id = board.add("Alpha");
    board.dispatch(&id);
    board.finish(&id, json!({"stopped":true,"code":1,"ended":now()}), Some(outcome("completed", &["passed"])));
    let task = board.task(&id);
    assert_eq!((task.latest_error(), task.failure.as_str(), task.retry_at, task.stage.as_str()), (Some("Stopped by user"), "", 0, "implementation"));
    assert!(board.kinds(&id).contains(&"attempt_stopped".into()));
    board.dispatch(&id);
    board.mutate(|state| state.tasks[0].revision += 1);
    board.complete(&id, outcome("completed", &["passed"]));
    let task = board.task(&id);
    assert_eq!(task.stage, "implementation");
    assert_eq!(task.attempts.last().unwrap().status(), Some("completed"));
    assert_eq!(task.history.last().unwrap().evidence["currentRevision"], false);
    let before = board.engine.state.sequence;
    board.engine.poll_attempt(0).unwrap();
    assert_eq!(board.engine.state.sequence, before);
}

impl Task {
    fn latest_error(&self) -> Option<&str> {
        self.attempts.last()?.error.as_deref()
    }
}

#[test]
fn usage_limit_logs_hold_dispatch_without_counting_failures() {
    let mut board = Board::new();
    let id = board.add("Alpha");
    board.dispatch(&id);
    fs::write(board.directory(&id).join("log.txt"), "working\nYou've hit your usage limit. Upgrade or try again at Jan 2nd, 2099 3:04 PM.\n").unwrap();
    board.finish(&id, json!({"code":1,"ended":now()}), None);
    assert!(board.engine.state.hold_until > now() + 365 * 24 * 3_600_000);
    assert_eq!(board.engine.notices.last().unwrap()["event"], "usage_limit");
    assert_eq!((board.task(&id).failure.as_str(), board.task(&id).retry_at), ("", 0));
    assert!(board.waiting(&id).unwrap().starts_with("Usage limit"));
    board.engine.control_as(false, None, Actor::Human, "resume").unwrap();
    assert_eq!(board.engine.state.hold_until, 0);
    board.dispatch(&id);
    fs::write(board.directory(&id).join("log.txt"), "usage limit reached\n").unwrap();
    let start = now();
    board.finish(&id, json!({"code":1,"ended":now()}), None);
    let hold = board.engine.state.hold_until;
    assert!(hold >= start + 3_600_000 && hold <= now() + 3_600_000);
}

#[test]
fn declared_artifacts_are_required_recovered_and_verified_at_review() {
    let mut board = Board::new();
    let id = board.create(board.with("Report", json!({"attributes":{"artifact":"report"}})));
    board.run(&id, outcome("completed", &["passed"]));
    assert!(board.latest(&id).error.as_deref().unwrap().contains("Declared artifact missing"));
    board.dispatch(&id);
    let directory = board.directory(&id);
    fs::create_dir_all(directory.join("scratch").join("deliverables")).unwrap();
    fs::write(directory.join("scratch").join("deliverables").join("report.md"), "report body").unwrap();
    board.complete(&id, outcome("completed", &["passed"]));
    assert_eq!(board.task(&id).stage, "review");
    let artifact = board.latest(&id).evidence.configuration.as_ref().unwrap()["artifact"].clone();
    let path = PathBuf::from(artifact["path"].as_str().unwrap());
    assert_eq!(path, directory.join("deliverables/report.md"));
    assert_eq!(artifact["digest"], json!(evidence::digest(&path).unwrap()));
    assert!(!directory.join("scratch").exists());
    board.dispatch(&id);
    fs::write(&path, "tampered").unwrap();
    board.complete(&id, outcome("accepted", &["passed"]));
    assert_eq!(board.latest(&id).status(), Some("rejected"));
    assert_eq!(board.task(&id).feedback, ["Artifact changed after implementation; rebuild it"]);
    assert_eq!(board.task(&id).stage, "implementation");
}

#[test]
fn accepted_deployable_candidate_requests_deployment() {
    let mut board = Board::new();
    let id = board.create(board.with("Candidate", json!({"attributes":{"artifact":"candidate","deploy":true}})));
    board.dispatch(&id);
    let directory = board.directory(&id);
    fs::create_dir_all(directory.join("deliverables")).unwrap();
    fs::write(directory.join("deliverables").join("candidate.exe"), "binary").unwrap();
    board.complete(&id, outcome("completed", &["passed"]));
    assert!(board.engine.pending_deploy.is_none());
    board.run(&id, outcome("accepted", &["passed"]));
    assert_eq!(board.engine.pending_deploy.as_deref(), Some(id.as_str()));
    assert!(crate::web::deployment_reviewed(board.task(&id)));
}

#[test]
fn editing_a_prerequisite_reopens_dependents_transitively() {
    let mut board = Board::new();
    let a = board.add("Alpha");
    let b = board.create(board.with("Beta", json!({"dependencies":[{"target":a,"reason":"uses_api"}]})));
    let c = board.create(board.with("Gamma", json!({"dependencies":[{"target":b,"reason":"uses_data"}]})));
    let ordered = board.create(board.with("Ordered", json!({"after":[a]})));
    let idle = board.create(board.with("Idle", json!({"dependencies":[{"target":a,"reason":"uses_schema"}]})));
    board.accept(&a);
    board.run(&b, outcome("completed", &["passed"]));
    board.run(&c, outcome("completed", &["passed"]));
    board.accept(&ordered);
    board.engine.edit(&a, json!({"outcome":"A corrected outcome","actor":"human"})).unwrap();
    let task = board.task(&a);
    assert_eq!((task.stage.as_str(), task.revision), ("implementation", 1));
    assert_eq!(task.history.last().unwrap().kind, "phase_changed");
    assert!(board.kinds(&a).contains(&"revised".into()));
    for id in [&b, &c] {
        let task = board.task(id);
        assert_eq!((task.stage.as_str(), task.revision), ("implementation", 1));
        let reopened = task.history.iter().find(|e| e.kind == "reopened").unwrap();
        assert_eq!(reopened.evidence["causedBy"], json!(a));
    }
    assert_eq!((board.task(&ordered).stage.as_str(), board.task(&ordered).revision), ("done", 0));
    assert_eq!(board.task(&idle).revision, 0);
}

#[test]
fn edits_are_refused_while_the_task_or_a_dependent_is_active() {
    let mut board = Board::new();
    let a = board.add("Alpha");
    let b = board.create(board.with("Beta", json!({"dependencies":[{"target":a,"reason":"uses_api"}]})));
    board.dispatch(&a);
    assert!(board.edit_error(&a, json!({"outcome":"Changed"})).contains("Stop the task"));
    board.finish(&a, json!({"stopped":true,"ended":now()}), None);
    board.dispatch(&b);
    assert!(board.edit_error(&a, json!({"outcome":"Changed"})).contains("Stop dependent task \"Beta\""));
    assert!(board.engine.edit("missing", json!({})).is_err());
}

#[test]
fn noncorrective_and_identical_edits_do_not_reset_work() {
    let mut board = Board::new();
    let a = board.add("Alpha");
    let b = board.add("Beta");
    board.accept(&a);
    board.engine.edit(&a, json!({"related":[b],"conflicts":[b]})).unwrap();
    let task = board.task(&a);
    assert_eq!((task.stage.as_str(), task.revision), ("done", 0));
    assert_eq!(task.history.last().unwrap().kind, "updated");
    let sequence = board.engine.state.sequence;
    let length = board.task(&a).history.len();
    board.engine.edit(&a, json!({"title":"  Alpha ","related":[b]})).unwrap();
    assert_eq!((board.engine.state.sequence, board.task(&a).history.len()), (sequence, length));
    board.engine.edit(&a, json!({"attributes":{"languages":["rust"]}})).unwrap();
    assert_eq!(board.task(&a).stage, "done");
    board.engine.edit(&a, json!({"attributes":{"lane":"docs"}})).unwrap();
    assert_eq!(board.task(&a).stage, "implementation");
}

#[test]
fn request_replan_validates_target_and_clips_reason() {
    let mut board = Board::new();
    let container = board.create(board.container("Group"));
    assert!(board.engine.request_replan(&container, json!({})).is_err());
    let done = board.add("Done");
    board.accept(&done);
    assert!(board.engine.request_replan(&done, json!({})).is_err());
    let active = board.add("Active");
    board.dispatch(&active);
    assert!(board.engine.request_replan(&active, json!({})).is_err());
    let id = board.add("Alpha");
    assert!(board.engine.request_replan(&id, json!({"actor":"worker"})).is_err());
    let value = board.engine.request_replan(&id, json!({"reason":"word ".repeat(45)})).unwrap();
    assert_eq!(value["status"], "In progress");
    let task = board.task(&id);
    assert_eq!(task.stage, "replan");
    assert_eq!(history::words(&task.feedback[0]), 30);
    assert_eq!(task.history.iter().rev().nth(1).unwrap().kind, "replan_requested");
    board.engine.request_replan(&id, json!({"reason":"  "})).unwrap();
    assert_eq!(board.task(&id).feedback.len(), 1);
}

#[test]
fn control_requires_reasons_and_respects_user_stops() {
    let mut board = Board::new();
    let id = board.add("Alpha");
    assert!(board.engine.control_as(true, None, Actor::Planner, "pause").unwrap_err().to_string().contains("explicit user action"));
    assert!(board.engine.control_as(true, Some(&id), Actor::Human, " ").unwrap_err().to_string().contains("requires a reason"));
    board.engine.control_as(true, Some(&id), Actor::Human, "hold on").unwrap();
    assert!(board.task(&id).stopped);
    assert!(board.engine.control_as(false, Some(&id), Actor::Planner, "go").unwrap_err().to_string().contains("user resume authority"));
    board.engine.control_as(false, Some(&id), Actor::Human, "go").unwrap();
    assert!(!board.task(&id).stopped);
    board.engine.control_as(true, Some(&id), Actor::Planner, "planner hold").unwrap();
    board.engine.control_as(false, Some(&id), Actor::Planner, "planner go").unwrap();
    let events = board.kinds(&id).iter().filter(|k| *k == "stopped" || *k == "resumed").count();
    assert_eq!(events, 4);
    board.engine.control_as(false, Some(&id), Actor::Planner, "again").unwrap();
    assert_eq!(board.kinds(&id).iter().filter(|k| *k == "resumed").count(), 2);
    board.engine.control_as(true, None, Actor::Human, "board pause").unwrap();
    board.engine.control_as(true, None, Actor::Human, "board pause again").unwrap();
    assert!(board.engine.state.paused);
    assert_eq!(board.engine.state.controls.len(), 1);
    assert_eq!(board.task(&id).history.last().unwrap().evidence["scope"], "board");
    board.engine.control_as(false, None, Actor::Human, "board resume").unwrap();
    assert!(!board.engine.state.paused);
    assert!(board.engine.control_as(false, Some("missing"), Actor::Planner, "x").is_err());
}

#[test]
fn stopping_an_unknown_task_is_rejected() {
    let mut board = Board::new();
    assert!(board.engine.control_as(true, Some("missing"), Actor::Human, "stop").is_err());
}

#[test]
fn stopping_an_active_task_signals_its_attempt() {
    let mut board = Board::new();
    let id = board.add("Alpha");
    let attempt = board.dispatch(&id);
    board.engine.control_as(true, Some(&id), Actor::Human, "halt").unwrap();
    assert!(board.engine.attempt_path(&attempt).join("stop").exists());
}

#[test]
fn sync_waiting_records_reason_changes_once() {
    let mut board = Board::new();
    let a = board.add("Alpha");
    let b = board.create(board.with("Beta", json!({"dependencies":[{"target":a,"reason":"uses_api"}]})));
    board.engine.sync_waiting().unwrap();
    assert_eq!(board.task(&b).recorded_wait.as_deref(), Some("Waiting for result from Alpha"));
    assert_eq!(board.task(&a).recorded_wait, None);
    let sequence = board.engine.state.sequence;
    board.engine.sync_waiting().unwrap();
    assert_eq!(board.engine.state.sequence, sequence);
    assert_eq!(board.kinds(&b).iter().filter(|k| *k == "waiting_changed").count(), 1);
}

fn batch(board: &Board) -> Value {
    json!({"requestKey":"req-1","actor":"human","container":board.container("Feature"),"children":[
        {"localKey":"schema","title":"Define schema","outcome":"Schema exists","acceptance":["Schema validates"]},
        {"localKey":"api","title":"Serve api","outcome":"Api serves","acceptance":["Api responds"],
            "dependencies":[{"target":{"local":"schema"},"reason":"uses_schema"}],"after":[{"local":"schema"}],"related":[{"local":"schema"}]}]})
}

#[test]
fn batch_creates_a_container_with_locally_linked_children() {
    let mut board = Board::new();
    let result = board.engine.create_batch(batch(&board)).unwrap();
    let container = result["container"].as_str().unwrap().to_owned();
    let schema = result["children"]["schema"].as_str().unwrap().to_owned();
    let api = result["children"]["api"].as_str().unwrap().to_owned();
    assert_eq!(board.engine.state.tasks.len(), 3);
    assert!(board.task(&container).container());
    for id in [&schema, &api] {
        let task = board.task(id);
        assert_eq!((task.parent.as_deref(), task.cwd.as_str()), (Some(container.as_str()), board.ws.to_str().unwrap()));
        assert_eq!(task.history.len(), 1);
        assert_eq!((task.history[0].kind.as_str(), task.history[0].actor), ("created", Actor::Human));
    }
    let task = board.task(&api);
    assert_eq!(task.dependencies, [Dependency { target: schema.clone(), reason: Some(DependencyReason::Schema) }]);
    assert_eq!((task.after.clone(), task.related.clone()), (vec![schema.clone()], vec![schema.clone()]));
    assert_eq!(board.task(&api).history[0].evidence["dependencies"][0]["target"], json!(schema));
    assert_eq!(board.waiting(&api).as_deref(), Some("Waiting for result from Define schema"));
}

#[test]
fn batch_replay_is_idempotent_and_conflicting_replay_is_rejected() {
    let mut board = Board::new();
    let input = batch(&board);
    let first = board.engine.create_batch(input.clone()).unwrap();
    let sequence = board.engine.state.sequence;
    assert_eq!(board.engine.create_batch(input.clone()).unwrap(), first);
    assert_eq!((board.engine.state.tasks.len(), board.engine.state.sequence), (3, sequence));
    let mut changed = input.clone();
    changed["children"][0]["title"] = json!("Define the schema");
    assert!(board.engine.create_batch(changed).unwrap_err().to_string().contains("Conflicting request replay"));
    let mut actor = input.clone();
    actor["actor"] = json!("planner");
    assert!(board.engine.create_batch(actor).unwrap_err().to_string().contains("Conflicting request replay"));
    let mut other = input;
    other["requestKey"] = json!("req-2");
    let second = board.engine.create_batch(other).unwrap();
    assert_ne!(second["container"], first["container"]);
    assert_eq!(board.engine.state.tasks.len(), 6);
    let reloaded = Engine::new(board.engine.directory.clone()).unwrap();
    assert_eq!(reloaded.state.batches.len(), 2);
}

#[test]
fn batch_rejects_invalid_shapes_atomically() {
    let mut board = Board::new();
    let valid = batch(&board);
    let mut cases: Vec<(Value, &str)> = Vec::new();
    cases.push((merge(valid.clone(), json!({"extra":1})), "Unknown batch field"));
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("requestKey");
    cases.push((missing, "requestKey is required"));
    cases.push((merge(valid.clone(), json!({"requestKey":"  "})), "requestKey is required"));
    cases.push((merge(valid.clone(), json!({"actor":"worker"})), "actor must be human or planner"));
    cases.push((merge(valid.clone(), json!({"container":board.spec("Leafy")})), "Container must have no acceptance criteria"));
    cases.push((merge(valid.clone(), json!({"container":merge(board.container("Keyed"), json!({"key":"k"}))})), "cannot replace existing tasks"));
    cases.push((merge(valid.clone(), json!({"children":[]})), "Children are required"));
    cases.push((merge(valid.clone(), json!({"children":{}})), "Children must be an array"));
    cases.push((merge(valid.clone(), json!({"children":["text"]})), "Child must be an object"));
    cases.push((merge(valid.clone(), json!({"children":[{"title":"No key","outcome":"o","acceptance":["c"]}]})), "localKey is required"));
    let twice = json!({"localKey":"same","title":"Twice","outcome":"o","acceptance":["c"]});
    cases.push((merge(valid.clone(), json!({"children":[twice.clone(),twice]})), "nonblank and unique"));
    for field in [json!({"parent":"x"}), json!({"standalone":true}), json!({"key":"k"}), json!({"supersedes":[]})] {
        let child = merge(json!({"localKey":"a","title":"Child","outcome":"o","acceptance":["c"]}), field);
        cases.push((merge(valid.clone(), json!({"children":[child]})), "Children inherit grouping"));
    }
    let mut unknown = valid.clone();
    unknown["children"][1]["after"] = json!([{"local":"nobody"}]);
    cases.push((unknown, "Unknown sibling local key"));
    let mut extra = valid.clone();
    extra["children"][1]["after"] = json!([{"local":"schema","id":"x"}]);
    cases.push((extra, "only local"));
    let mut number = valid.clone();
    number["children"][1]["conflicts"] = json!([7]);
    cases.push((number, "Reference must be a task ID"));
    let mut invalid = valid.clone();
    invalid["children"][1]["acceptance"] = json!(["a", "b", "c", "d"]);
    cases.push((invalid, "At most 3 acceptance criteria"));
    for (input, message) in cases {
        let error = board.engine.create_batch(input).unwrap_err().to_string();
        assert!(error.contains(message), "{message}: {error}");
        assert!(board.engine.state.tasks.is_empty() && board.engine.state.batches.is_empty());
    }
    assert_eq!(board.engine.state.sequence, 0);
}

#[test]
fn updates_cursor_returns_only_tasks_changed_after_the_sequence() {
    let mut board = Board::new();
    assert_eq!(board.engine.state.sequence, 0);
    let a = board.add("Alpha");
    let after_a = board.engine.state.sequence;
    assert_eq!(after_a, 1);
    let b = board.add("Beta");
    let updates = board.engine.updates(None, Some(after_a), "boom");
    assert_eq!(updates["sequence"], 2);
    assert_eq!(updates["error"], "boom");
    assert_eq!(updates["ids"], json!([a, b]));
    assert_eq!(updates["tasks"].as_array().unwrap().iter().map(|t| t["id"].clone()).collect::<Vec<_>>(), [json!(b)]);
    assert_eq!(board.engine.updates(None, None, "")["tasks"].as_array().unwrap().len(), 2);
    assert!(board.engine.updates(None, Some(2), "")["tasks"].as_array().unwrap().is_empty());
    assert!(!board.engine.changed_since(None, Some(2)));
    assert!(board.engine.changed_since(None, Some(1)));
    assert!(board.engine.changed_since(None, None));
    board.engine.edit(&a, json!({"title":"Alpha"})).unwrap();
    assert_eq!(board.engine.state.sequence, 2);
    board.engine.edit(&a, json!({"outcome":"Changed"})).unwrap();
    assert_eq!(board.engine.state.sequence, 3);
    assert_eq!(board.task(&a).changed, 3);
    assert_eq!(board.task(&b).changed, 2);
    board.engine.remove(&b, Actor::Human).unwrap();
    assert_eq!(board.engine.updates(None, Some(3), "")["ids"], json!([a]));
    assert!(board.engine.changed_since(None, Some(3)));
}

#[test]
fn updates_are_scoped_by_workspace_with_board_wide_changes_visible_everywhere() {
    let mut board = Board::new();
    let other = board.temp.dir("other");
    let ws = board.ws.clone();
    let a = board.add("Alpha");
    let start = board.engine.state.sequence;
    board.create(board.with("Beta", json!({"cwd":other})));
    assert!(board.engine.changed_since(Some(&other), Some(start)));
    assert!(!board.engine.changed_since(Some(&ws), Some(start)));
    let scoped = board.engine.updates(Some(&ws), Some(0), "");
    assert_eq!(scoped["ids"], json!([a]));
    assert_eq!(scoped["tasks"].as_array().unwrap().len(), 1);
    assert!(board.engine.updates(Some(&ws), Some(start), "")["tasks"].as_array().unwrap().is_empty());
    let paused = board.engine.state.sequence;
    board.engine.control_as(true, None, Actor::Human, "pause").unwrap();
    assert!(board.engine.changed_since(Some(&ws), Some(paused)));
    assert_eq!(board.engine.updates(Some(&ws), None, "")["paused"], true);
    let settings = board.engine.state.sequence;
    board.mutate(|state| {
        state.config.workspaces.insert(other.clone(), history::WorkspaceSettings { updates: history::Updates::Events, ..Default::default() });
    });
    assert!(board.engine.changed_since(Some(&ws), Some(settings)));
    assert_eq!(board.engine.updates(Some(&other), None, "")["updates"], "events");
    assert_eq!(board.engine.updates(Some(&ws), None, "")["updates"], "poll");
}

#[test]
fn configure_defaults_replan_and_rejects_empty_commands() {
    let mut board = Board::new();
    board.engine.configure(json!({"implementation":["worker","{model}","{effort}"],"review":["reviewer"]})).unwrap();
    let config = &board.engine.state.config;
    assert_eq!(config.replan, config.implementation);
    assert!(config.capabilities.contains(&Need::Network));
    assert!(board.engine.configure(json!({"implementation":[],"review":["r"]})).is_err());
    assert!(board.engine.configure(json!({"implementation":["i"],"review":[" "]})).is_err());
    assert!(board.engine.configure(json!({"implementation":["i"]})).is_err());
    board.engine.configure(json!({"implementation":["i"],"review":["r"],"replan":["p"],"capabilities":["gpu"]})).unwrap();
    assert_eq!(board.engine.state.config.replan, ["p"]);
    assert!(board.fail(board.with("Needs network", json!({"attributes":{"needs":["network"]}}))).contains("No worker provides [\"network\"]"));
}

fn legacy(id: &str, cwd: &Path, patch: Value) -> Value {
    merge(
        json!({"id":id,"title":"Legacy task","outcome":"Legacy outcome","cwd":cwd,"acceptance":["Legacy criterion"],"dependencies":[],
            "stage":"implementation","stopped":false,"attempts":[],"active":null,"feedback":[],"failure":"","retryAt":0}),
        patch,
    )
}
fn legacy_state(version: Value, tasks: Vec<Value>) -> Value {
    json!({"schemaVersion":version,"paused":false,"tasks":tasks,"config":{"implementation":["impl"],"review":["review"],"workspaces":{}}})
}

#[test]
fn pre_schema_seven_state_without_workspaces_loads() {
    let temp = Temp::new();
    let ws = temp.dir("ws");
    let mut state = legacy_state(json!(6), vec![legacy("a", &ws, json!({"standalone":true}))]);
    state["config"].as_object_mut().unwrap().remove("workspaces");
    let engine = load(&temp, &state).unwrap();
    assert!(engine.state.config.workspaces.is_empty());
}
fn keyed(path: &Path, value: Value) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(path.to_string_lossy().into_owned(), value);
    Value::Object(map)
}
fn load(temp: &Temp, state: &Value) -> Result<Engine> {
    let directory = temp.dir(&format!("legacy-{}", super::id()));
    write(&directory.join("state.json"), state)?;
    Engine::new(directory)
}

#[test]
fn fresh_boards_start_at_the_current_schema() {
    let temp = Temp::new();
    let engine = temp.engine();
    assert_eq!(engine.state.schema_version, Some(history::SCHEMA_VERSION));
    assert!(!engine.state.config.replan.is_empty());
    assert_eq!(engine.state.config.capabilities.len(), 1);
    let saved: Value = read(&engine.directory.join("state.json")).unwrap();
    assert_eq!(saved["schemaVersion"], json!(history::SCHEMA_VERSION));
    let reloaded = temp.engine();
    assert_eq!(reloaded.state.config.implementation, engine.state.config.implementation);
}

#[test]
fn schema_three_string_dependencies_become_unclassified_links() {
    let temp = Temp::new();
    let ws = temp.dir("ws");
    for version in [json!(3), Value::Null] {
        let state = legacy_state(version, vec![legacy("a", &ws, json!({})), legacy("b", &ws, json!({"dependencies":["a"]}))]);
        let engine = load(&temp, &state).unwrap();
        let b = &engine.state.tasks[1];
        assert_eq!(b.dependencies, [Dependency { target: "a".into(), reason: None }]);
        assert_eq!(engine.state.schema_version, Some(history::SCHEMA_VERSION));
        assert_eq!(engine.state.waiting_reason(b).as_deref(), Some("Unresolved grouping; choose parent container or standalone true"));
        assert_eq!(engine.state.config.replan, ["impl"]);
        let saved: Value = read(&engine.directory.join("state.json")).unwrap();
        assert_eq!(saved["schemaVersion"], json!(history::SCHEMA_VERSION));
        assert_eq!(saved["tasks"][1]["dependencies"], json!([{"target":"a","reason":null}]));
        let reloaded = Engine::new(engine.directory.clone()).unwrap();
        assert_eq!(reloaded.state.tasks[1].dependencies, b.dependencies);
    }
}

#[test]
fn unclassified_dependencies_wait_even_when_grouped() {
    let temp = Temp::new();
    let ws = temp.dir("ws");
    let state = legacy_state(json!(3), vec![legacy("a", &ws, json!({"standalone":true})), legacy("b", &ws, json!({"standalone":true,"dependencies":["a"]}))]);
    let engine = load(&temp, &state).unwrap();
    assert_eq!(engine.state.waiting_reason(&engine.state.tasks[1]).as_deref(), Some("Unresolved dependency; classify explicitly, use after, or nothing"));
    assert_eq!(engine.state.waiting_reason(&engine.state.tasks[0]), None);
}

#[test]
fn relationship_plan_classifies_schema_three_tasks() {
    let temp = Temp::new();
    let ws = temp.dir("ws");
    let directory = temp.dir("board-plan");
    let state = legacy_state(json!(3), vec![legacy("a", &ws, json!({})), legacy("b", &ws, json!({"dependencies":["a"]}))]);
    write(&directory.join("state.json"), &state).unwrap();
    write(&directory.join("relationship-plan.json"), &json!({"tasks":[
        {"id":"a","standalone":true,"dependencies":[],"after":[]},
        {"id":"b","standalone":true,"dependencies":[{"target":"a","reason":"uses_data"}],"after":[]},
        {"id":"absent","standalone":true,"dependencies":[],"after":[]}]})).unwrap();
    let engine = Engine::new(directory).unwrap();
    let b = &engine.state.tasks[1];
    assert!(b.standalone);
    assert_eq!(b.dependencies, [Dependency { target: "a".into(), reason: Some(DependencyReason::Data) }]);
    let event = b.history.last().unwrap();
    assert_eq!((event.kind.as_str(), event.actor), ("relationships_classified", Actor::Human));
    assert_eq!(engine.state.waiting_reason(b).as_deref(), Some("Waiting for result from Legacy task"));
}

#[test]
fn malformed_or_newer_states_fail_to_load() {
    let temp = Temp::new();
    let ws = temp.dir("ws");
    let missing_reason = legacy_state(json!(5), vec![legacy("a", &ws, json!({"standalone":true})), legacy("b", &ws, json!({"standalone":true,"dependencies":[{"target":"a"}]}))]);
    assert!(load(&temp, &missing_reason).err().unwrap().to_string().contains("Dependency reason missing"));
    let newer = legacy_state(json!(history::SCHEMA_VERSION + 1), vec![]);
    assert!(load(&temp, &newer).err().unwrap().to_string().contains("newer than this coordinator"));
    let dangling = legacy_state(json!(9), vec![legacy("b", &ws, json!({"standalone":true,"dependencies":[{"target":"gone","reason":"uses_api"}]}))]);
    assert!(load(&temp, &dangling).err().unwrap().to_string().contains("Related task not found: gone"));
    let unknown_reason = legacy_state(json!(9), vec![legacy("b", &ws, json!({"dependencies":[{"target":"a","reason":"vibes"}]}))]);
    assert!(load(&temp, &unknown_reason).is_err());
}

#[test]
fn legacy_flags_migrate_to_typed_attributes() {
    let temp = Temp::new();
    let ws = temp.dir("ws");
    let state = legacy_state(json!(6), vec![
        legacy("audit", &ws, json!({"standalone":true,"audit":true})),
        legacy("candidate", &ws, json!({"standalone":true,"evidenceRequirements":{"kind":"candidate build"}})),
        legacy("plain", &ws, json!({"standalone":true,"evidenceRequirements":"tests"})),
    ]);
    let engine = load(&temp, &state).unwrap();
    assert!(engine.state.tasks[0].attributes.lane == Lane::Audit && engine.state.tasks[0].read_only());
    assert!(engine.state.tasks[1].attributes.artifact == Artifact::Candidate);
    assert!(engine.state.tasks[2].attributes.artifact == Artifact::None);
    let saved = fs::read_to_string(engine.directory.join("state.json")).unwrap();
    assert!(!saved.contains("\"audit\":") &&!saved.contains("evidenceRequirements"));
}

#[test]
fn legacy_definitions_violating_limits_move_to_replan() {
    let temp = Temp::new();
    let ws = temp.dir("ws");
    let long = "word ".repeat(20);
    let state = legacy_state(json!(8), vec![
        legacy("long", &ws, json!({"standalone":true,"title":long})),
        legacy("stopped", &ws, json!({"standalone":true,"title":long,"stopped":true})),
        legacy("done", &ws, json!({"standalone":true,"title":long,"stage":"done"})),
        legacy("notes", &ws, json!({"standalone":true,"refinements":["Old note"]})),
        legacy("valid", &ws, json!({"standalone":true})),
    ]);
    let engine = load(&temp, &state).unwrap();
    let stages: Vec<_> = engine.state.tasks.iter().map(|t| t.stage.as_str()).collect();
    assert_eq!(stages, ["replan", "implementation", "done", "replan", "implementation"]);
    let task = &engine.state.tasks[0];
    assert_eq!(task.feedback, ["Legacy definition violates current limits; rewrite it with typed attributes and relations"]);
    assert_eq!(task.history.last().unwrap().evidence, json!({"from":"implementation","to":"replan"}));
}

#[test]
fn legacy_limits_are_adopted_into_workspace_settings() {
    let temp = Temp::new();
    let ws = temp.dir("ws");
    let mut state = legacy_state(json!(8), vec![legacy("two", &ws, json!({"standalone":true,"acceptance":["a","b"]}))]);
    state["config"]["limits"] = keyed(&ws, json!({"criteria":1}));
    let engine = load(&temp, &state).unwrap();
    assert_eq!(engine.state.config.workspaces[&ws].limits.criteria, 1);
    assert_eq!(engine.state.tasks[0].stage, "replan");
    let saved: Value = read(&engine.directory.join("state.json")).unwrap();
    assert!(saved["config"].get("limits").is_none());
    assert_eq!(saved["config"]["workspaces"][ws.to_str().unwrap()]["limits"]["criteria"], 1);
    state["config"]["workspaces"] = keyed(&ws, json!({"limits":{"criteria":2}}));
    assert!(load(&temp, &state).err().unwrap().to_string().contains("conflicting limits"));
    state["config"]["workspaces"] = keyed(&ws, json!({"limits":{"criteria":1}}));
    load(&temp, &state).unwrap();
}

fn model_board() -> Board {
    let mut board = Board::new();
    let key = workspace(&board.ws).unwrap();
    board.mutate(move |state| {
        let settings: history::WorkspaceSettings = serde_json::from_value(json!({"planner":"sol_6_1_medium","worker":"sol_6_1_light"})).unwrap();
        state.config.workspaces.insert(key, settings);
    });
    board
}

#[test]
fn tasks_may_choose_an_enabled_model_and_default_to_the_workspace_worker() {
    let mut board = model_board();
    let chosen = board.create(board.with("Rename helper", json!({"attributes":{"model":"sol_6_1_medium"}})));
    let plain = board.add("Plain task");
    let family = board.engine.state.family;
    let settings = board.engine.state.settings(board.task(&chosen).workspace.as_deref());
    let (id, agent) = settings.task_agent(board.task(&chosen).attributes.model.as_deref(), family).unwrap();
    assert_eq!((id.as_str(), agent.model.as_str()), ("sol_6_1_medium", "gpt-6.1-sol"));
    let (id, agent) = settings.task_agent(board.task(&plain).attributes.model.as_deref(), family).unwrap();
    assert_eq!((id.as_str(), agent.model.as_str()), ("sol_6_1_light", "gpt-6.1-sol"));
    let saved = board.engine.task_value(board.task(&chosen), false).unwrap();
    assert_eq!(saved["attributes"]["model"], "sol_6_1_medium");
    assert!(board.engine.task_value(board.task(&plain), false).unwrap()["attributes"].get("model").is_none());
}

#[test]
fn task_models_must_be_enabled_for_the_family_and_workspace() {
    let mut board = model_board();
    assert!(board.fail(board.with("Crossed", json!({"attributes":{"model":"claude_haiku_5_5_low"}}))).contains("not enabled"));
    assert!(board.fail(board.with("Unknown", json!({"attributes":{"model":"no_such_model"}}))).contains("not enabled"));
    assert!(board.fail(board.with("Retired", json!({"attributes":{"model":"sol_light"}}))).contains("not enabled"));
    assert!(board.fail(board.with("Blank", json!({"attributes":{"model":" "}}))).contains("model"));
    assert!(board.fail(board.with("Group", json!({"acceptance":[],"attributes":{"model":"luna_6_high"}}))).contains("Containers run no worker"));
    let key = workspace(&board.ws).unwrap();
    board.mutate(move |state| {
        let settings = state.config.workspaces.get_mut(&key).unwrap();
        settings.agents = BTreeMap::from([("luna_6_high".to_string(), crate::agents::catalog()["luna_6_high"].clone())]);
    });
    assert!(board.fail(board.with("Not listed", json!({"attributes":{"model":"sol_6_1_medium"}}))).contains("not enabled"));
    board.create(board.with("Listed", json!({"attributes":{"model":"luna_6_high"}})));
    board.create(board.with("Default by name", json!({"attributes":{"model":"sol_6_1_light"}})));
}

#[test]
fn editing_a_task_model_is_a_corrective_revision() {
    let mut board = model_board();
    let id = board.add("Summarize module");
    let revision = board.task(&id).revision;
    board.engine.edit(&id, json!({"attributes":{"model":"luna_6_high"}})).unwrap();
    assert_eq!(board.task(&id).attributes.model.as_deref(), Some("luna_6_high"));
    assert!(board.task(&id).revision >= revision);
    assert!(board.edit_error(&id, json!({"attributes":{"model":"claude_opus_5_5_medium"}})).contains("not enabled"));
}

#[test]
fn model_listing_reports_enabled_options_and_the_default() {
    let board = model_board();
    let listed = board.engine.state.models(Some(&workspace(&board.ws).unwrap()));
    assert_eq!(listed["default"], "sol_6_1_light");
    let ids: Vec<_> = listed["agents"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap().to_owned()).collect();
    assert!(ids.contains(&"luna_6_high".to_string()) && ids.contains(&"sol_6_1_medium".to_string()));
    assert!(!ids.iter().any(|id| id.starts_with("claude_") || id == "sol_light"));
    let hosted = listed["agents"].as_array().unwrap().iter().find(|a| a["id"] == "sol_6_1_light").unwrap();
    assert_eq!(hosted["model"], "gpt-6.1-sol");
}

#[test]
fn patching_one_attribute_keeps_the_others() {
    let mut board = model_board();
    let id = board.create(board.with("Tune parser", json!({"attributes":{"lane":"data","size":"medium","needs":["network"],"priority":1}})));
    board.engine.edit(&id, json!({"attributes":{"model":"luna_6_high"}})).unwrap();
    let attributes = &board.task(&id).attributes;
    assert!(attributes.size == Size::Medium);
    assert_eq!((attributes.lane, attributes.priority, attributes.model.as_deref()), (Lane::Data, Some(1), Some("luna_6_high")));
    assert_eq!(attributes.needs.len(), 1);
    board.engine.edit(&id, json!({"attributes":{"model":null}})).unwrap();
    assert_eq!((board.task(&id).attributes.model.as_deref(), board.task(&id).attributes.lane), (None, Lane::Data));
}

#[test]
fn only_implementation_runs_on_the_chosen_model() {
    let mut board = model_board();
    let id = board.create(board.with("Summarize", json!({"attributes":{"model":"luna_6_high"}})));
    assert_eq!(board.task(&id).worker_model(), Some("luna_6_high"));
    board.run(&id, outcome("completed", &["passed"]));
    assert_eq!(board.task(&id).stage, "review");
    assert_eq!(board.task(&id).worker_model(), None);
    let family = board.engine.state.family;
    let settings = board.engine.state.settings(board.task(&id).workspace.as_deref());
    assert_eq!(settings.task_agent(board.task(&id).worker_model(), family).unwrap().0, "sol_6_1_light");
}

#[test]
fn wipe_deletes_every_task_workspace_and_attempt_after_explicit_confirmation() {
    let temp = crate::test_support::Temp::new();
    let mut engine = temp.engine();
    let ws = temp.dir("workspace");
    fs::write(ws.join("keep.txt"), "x").unwrap();
    let key = workspace(&ws).unwrap();
    let settings: history::WorkspaceSettings = serde_json::from_value(json!({"planner":"sol_6_1_medium","worker":"sol_6_1_light"})).unwrap();
    let mut state = engine.state.clone();
    state.config.workspaces.insert(key.clone(), settings);
    engine.commit(state).unwrap();
    engine.create(json!({"title":"Task","outcome":"Done","cwd":ws,"acceptance":["Works"],"standalone":true})).unwrap();
    fs::create_dir_all(engine.attempt_path("old")).unwrap();
    assert!(engine.wipe(json!({"actor":"human"})).unwrap_err().to_string().contains("wipe everything"));
    assert!(engine.wipe(json!({"confirm":"wipe everything"})).unwrap_err().to_string().contains("explicit user"));
    assert_eq!(engine.state.tasks.len(), 1);
    let report = engine.wipe(json!({"actor":"human","confirm":"wipe everything"})).unwrap();
    assert_eq!((report["deleted"]["tasks"].as_u64(), report["deleted"]["workspaces"].as_u64()), (Some(1), Some(1)));
    assert_eq!(report["errors"], json!([]));
    assert!(!ws.exists() && !engine.directory.join("attempts").exists());
    assert!(engine.state.tasks.is_empty() && engine.state.config.workspaces.is_empty());
    let reloaded = Engine::new(engine.directory.clone()).unwrap();
    assert!(reloaded.state.tasks.is_empty() && reloaded.state.config.workspaces.is_empty());
}
