use super::*;

#[test]
fn workspace_agents_are_explicit_and_preserve_exact_model_effort_pairs() {
    let family = Some(crate::agents::Family::Codex);
    for (name, model, effort) in [("luna_6_high", "gpt-6-luna", "high"), ("sol_6_1_light", "gpt-6.1-sol", "low"), ("sol_6_1_medium", "gpt-6.1-sol", "medium")] {
        let settings: WorkspaceSettings = serde_json::from_value(json!({"planner":"sol_6_1_light","worker":name})).unwrap();
        settings.validate(family).unwrap();
        let agent = settings.agent("worker", family).unwrap();
        assert_eq!((agent.model.as_str(), agent.effort()), (model, effort));
    }
    let historical: WorkspaceSettings = serde_json::from_value(json!({"planner":"sol_light","worker":"sol_light"})).unwrap();
    assert!(historical.validate(family).is_err());
    assert!(WorkspaceSettings::default().validate(family).is_err());
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("workspace-prompts-{}", crate::engine::id())).join("prompt_candidate");
        fs::create_dir_all(path.join("project_planner")).unwrap();
        for kind in kinds() {
            fs::create_dir_all(path.join(kind)).unwrap();
        }
        Self(path)
    }
    fn engine(&self) -> Engine { Engine::new(self.0.join("board")).unwrap() }
    fn input(&self, name: &str) -> Value {
        json!({"path":self.0.join(name),"project_planner":"selected.md","worker_protocol":"selected.md","roles":role_kinds().into_iter().map(|kind| (kind.to_owned(), json!("selected.md"))).collect::<serde_json::Map<_, _>>(),
            "settings":{"planner":"sol_6_1_light","worker":"sol_6_1_light"}})
    }
    fn candidates(&self) {
        fs::write(self.0.join("project_planner/selected.md"), "Planner candidate unique instructions.").unwrap();
        fs::write(self.0.join("worker_protocol/selected.md"), "Worker candidate unique instructions.").unwrap();
        for kind in role_kinds() {
            fs::write(self.0.join(kind).join("selected.md"), format!("{kind} candidate unique instructions.")).unwrap();
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let parent = self.0.parent().unwrap();
        assert!(parent.canonicalize().unwrap().starts_with(std::env::temp_dir().canonicalize().unwrap()));
        fs::remove_dir_all(parent).unwrap();
    }
}
#[test]
fn empty_catalog_and_invalid_candidates_do_not_create_workspaces() {
    let fixture = Fixture::new();
    let mut engine = fixture.engine();
    assert_eq!(catalog(&fixture.0).unwrap()["project_planner"], json!([]));
    assert!(create(&mut engine, fixture.input("missing"), &fixture.0).is_err());
    assert!(!fixture.0.join("missing").exists());
    fixture.candidates();
    let mut input = fixture.input("escape");
    input["project_planner"] = json!("../worker_protocol/selected.md");
    assert!(create(&mut engine, input, &fixture.0).is_err());
    fs::write(fixture.0.join("worker_protocol/selected.md"), "word ".repeat(201)).unwrap();
    assert!(create(&mut engine, fixture.input("oversize"), &fixture.0).is_err());
    assert!(!fixture.0.join("oversize").exists());
    assert_eq!(catalog(&fixture.0).unwrap()["invalid"].as_array().unwrap().len(), 1);
    fs::write(fixture.0.join("worker_protocol/selected.md"), " \n\t").unwrap();
    assert!(create(&mut engine, fixture.input("blank"), &fixture.0).is_err());
}
#[test]
fn creation_keeps_the_workspace_empty_and_freezes_selection_hashes() {
    let fixture = Fixture::new();
    fixture.candidates();
    let mut engine = fixture.engine();
    let result = create(&mut engine, fixture.input("first"), &fixture.0).unwrap();
    let directory = PathBuf::from(result["workspace"].as_str().unwrap());
    let settings: WorkspaceSettings = serde_json::from_value(result["settings"].clone()).unwrap();
    assert_eq!(engine.state.workspace_ids[&directory].abbreviation, "FIR");
    assert_eq!(engine.state.workspace_ids[&directory].next, 0);
    assert_eq!(fs::read_dir(&directory).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect::<Vec<_>>(), [AGENTS_FILE, LIMITS_FILE]);
    let guide = fs::read_to_string(directory.join(AGENTS_FILE)).unwrap();
    let shown = directory.to_string_lossy().trim_start_matches(r"\\?\").to_owned();
    assert!(guide.starts_with(&format!("# Workspace {shown}\n")) && guide.contains(&format!("Keep everything you create under `{shown}`")));
    for route in ["/api/task-schema?workspace=W", "/api/tasks?workspace=W", "PATCH /api/tasks/{id}", "/api/updates?workspace=W&after=SEQ&wait=S", "/api/sessions/{id}/deliver"] {
        assert!(guide.contains(route), "{route}");
    }
    for admin in ["/api/deployment", "/api/wipe", "/api/config", "/api/state", "/api/analytics", "/api/workspaces"] {
        assert!(!guide.contains(admin), "{admin}");
    }
    assert!(guide.contains("answers immediately") == (settings.updates == crate::history::Updates::Poll));
    for delivery in ["## Delivering results", "must contain everything the result needs and work without anything outside it","POST /api/sessions/{id}/deliver` with the body `{}`", "$COORDINATOR_SESSION", "only what was already delivered is handed in", "`node_modules`"] {
        assert!(guide.contains(delivery), "{delivery}");
    }
    let generic = guide.replace(&shown, "").to_lowercase();
    for specific in ["benchmark", "submission", "harness.json", "judge", "score"] {
        assert!(!generic.contains(specific), "{specific}");
    }
    assert_eq!(worker(&fixture.0, &settings, crate::history::Role::Worker, "implementation").unwrap()["text"], "Worker candidate unique instructions.");
    assert!(!planner(&fixture.0, &settings).unwrap().is_empty());
    assert_eq!(engine.state.config.workspaces[&directory], settings);
    verify(&fixture.0, &settings).unwrap();
    fs::write(fixture.0.join("worker_protocol/selected.md"), "new ".repeat(200)).unwrap();
    assert!(worker(&fixture.0, &settings, crate::history::Role::Worker, "implementation").unwrap_err().to_string().contains("changed since the workspace selected it"));
    let second = create(&mut engine, fixture.input("second"), &fixture.0).unwrap();
    assert_ne!(result["settings"]["prompts"]["worker_protocol"]["sha256"], second["settings"]["prompts"]["worker_protocol"]["sha256"]);
    assert!(create(&mut engine, fixture.input("first"), &fixture.0).is_err());
    let loaded = fixture.engine();
    assert_eq!(loaded.state.config.workspaces[&directory], settings);
}

#[test]
fn generation_picks_each_role_independently_and_is_reproducible_by_seed() {
    let fixture = Fixture::new();
    for index in 0..3 {
        fs::write(fixture.0.join(format!("project_planner/p{index}.md")), format!("Planner candidate {index}.")).unwrap();
        fs::write(fixture.0.join(format!("worker_protocol/w{index}.md")), format!("Worker candidate {index}.")).unwrap();
        for role in role_kinds() {
            fs::write(fixture.0.join(format!("{role}/r{index}.md")), format!("{role} candidate {index}.")).unwrap();
        }
    }
    let run = |prefix: &str, seed: u64| {
        let mut engine = fixture.engine();
        let paths: Vec<_> = (0..30).map(|i| fixture.0.join(format!("{prefix}{i}"))).collect();
        generate(&mut engine, json!({"paths":paths,"seed":seed}), &fixture.0).unwrap()
    };
    let first = run("a", 42);
    let pick = |result: &Value, role: &str| -> Vec<String> {
        result["workspaces"].as_array().unwrap().iter().map(|w| {
            let prompts = &w["settings"]["prompts"];
            prompts.get(role).filter(|p| p.is_object()).unwrap_or(&prompts["roles"][role])["candidate"].as_str().unwrap().to_owned()
        }).collect()
    };
    for workspace in first["workspaces"].as_array().unwrap() {
        let mut expected = role_kinds();
        expected.sort();
        assert_eq!(workspace["settings"]["prompts"]["roles"].as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>(), expected);
    }
    let all: Vec<&str> = ["project_planner", "worker_protocol"].into_iter().chain(role_kinds()).collect();
    for role in &all {
        let names = pick(&first, role);
        assert_eq!(names.iter().collect::<std::collections::BTreeSet<_>>().len(), 3, "{role}: {names:?}");
    }
    let pairs = |a: &str, b: &str| pick(&first, a).into_iter().zip(pick(&first, b)).collect::<std::collections::BTreeSet<_>>().len();
    assert!(pairs("worker_protocol", "reviewer") > 3 && pairs("reviewer", "test") > 3 && pairs("project_planner", "worker_protocol") > 3);
    let second = run("b", 42);
    let models = |result: &Value| -> Vec<(Value, Value, Value)> { result["workspaces"].as_array().unwrap().iter().map(|w| (w["settings"]["planner"].clone(), w["settings"]["worker"].clone(), w["settings"]["limits"].clone())).collect() };
    assert_eq!(models(&first), models(&second));
    for role in &all {
        assert_eq!(pick(&first, role), pick(&second, role), "{role}");
    }
    assert_ne!(pick(&first, "worker_protocol"), pick(&run("c", 7), "worker_protocol"));
    for workspace in first["workspaces"].as_array().unwrap() {
        assert_ne!(workspace["settings"]["planner"], "luna_6_high");
    }
    let mut engine = fixture.engine();
    assert!(generate(&mut engine, json!({"paths":[fixture.0.join("a0")]}), &fixture.0).is_err());
    assert!(generate(&mut engine, json!({"paths":[]}), &fixture.0).is_err());
}

#[test]
fn roles_use_their_own_prompt_and_the_defaults_fill_the_rest() {
    let fixture = Fixture::new();
    fixture.candidates();
    let defaults = defaults_root(&fixture.0);
    fs::create_dir_all(&defaults).unwrap();
    for kind in kinds() {
        fs::write(defaults.join(default_file(kind)), format!("{kind} default instructions.")).unwrap();
    }
    fs::write(defaults.join("settings.json"), r#"{"planner":"sol_6_1_light","worker":"sol_6_1_light","updates":"events"}"#).unwrap();
    fs::write(fixture.0.join("ui-code/visual.md"), "Build interfaces people enjoy.").unwrap();
    let mut engine = fixture.engine();
    let mut input = fixture.input("roles");
    input["roles"] = json!({"ui-code":"visual.md"});
    let result = create(&mut engine, input, &fixture.0).unwrap();
    let settings: WorkspaceSettings = serde_json::from_value(result["settings"].clone()).unwrap();
    let ui = worker(&fixture.0, &settings, crate::history::Role::UiCode, "implementation").unwrap();
    assert_eq!((ui["role"].as_str(), ui["text"].as_str()), (Some("ui-code"), Some("Build interfaces people enjoy.")));
    let backend = worker(&fixture.0, &settings, crate::history::Role::BackendCode, "implementation").unwrap();
    assert_eq!((backend["candidate"].as_str(), backend["text"].as_str()), (Some("backend-code.txt"), Some("backend-code default instructions.")));
    assert_eq!(worker(&fixture.0, &settings, crate::history::Role::Worker, "implementation").unwrap()["role"], "worker_protocol");
    let plain = create(&mut engine, json!({"path":fixture.0.join("plain")}), &fixture.0).unwrap();
    let settings: WorkspaceSettings = serde_json::from_value(plain["settings"].clone()).unwrap();
    assert_eq!((settings.updates, settings.planner.as_deref()), (crate::history::Updates::Events, Some("sol_6_1_light")));
    assert_eq!(planner(&fixture.0, &settings).unwrap(), "project_planner default instructions.");
    fs::write(defaults.join("planner.txt"), "Edited later.").unwrap();
    assert!(planner(&fixture.0, &settings).unwrap_err().to_string().contains("planner.txt"));
    let mut unknown = fixture.input("unknown-role");
    unknown["roles"] = json!({"marketing":"visual.md"});
    assert!(create(&mut engine, unknown, &fixture.0).is_err());
}

#[test]
fn the_shipped_catalog_lists_every_prompt_candidate() {
    let listed = catalog(&root()).unwrap();
    assert_eq!(listed["invalid"], json!([]));
    for kind in kinds() {
        let files = fs::read_dir(root().join(kind)).unwrap().filter(|e| e.as_ref().unwrap().file_type().unwrap().is_file()).count();
        assert!(files >= 2, "{kind}");
        assert_eq!(listed[kind].as_array().unwrap().len(), files, "{kind}");
    }
}

#[test]
fn review_and_replan_stages_use_their_own_prompts_instead_of_the_task_role() {
    let fixture = Fixture::new();
    fixture.candidates();
    fs::create_dir_all(fixture.0.join("ui-code")).unwrap();
    fs::write(fixture.0.join("ui-code/selected.md"), "Build interfaces people enjoy.").unwrap();
    let mut engine = fixture.engine();
    let mut input = fixture.input("staged");
    input["roles"]["ui-code"] = json!("selected.md");
    let settings: WorkspaceSettings = serde_json::from_value(create(&mut engine, input, &fixture.0).unwrap()["settings"].clone()).unwrap();
    let pick = |role, stage: &str| worker(&fixture.0, &settings, role, stage).unwrap()["role"].as_str().unwrap().to_owned();
    use crate::history::Role::{UiCode, Worker};
    assert_eq!([pick(UiCode, "implementation"), pick(UiCode, "review"), pick(UiCode, "replan")], ["ui-code", "reviewer", "replanner"]);
    assert_eq!([pick(Worker, "implementation"), pick(Worker, "review"), pick(Worker, "replan")], ["worker_protocol", "reviewer", "replanner"]);
    for kind in ["reviewer", "replanner"] {
        let mut missing = fixture.input(&format!("no-{kind}"));
        missing["roles"].as_object_mut().unwrap().remove(kind);
        assert!(create(&mut engine, missing, &fixture.0).unwrap_err().to_string().contains(kind));
    }
}
