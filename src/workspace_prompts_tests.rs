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
        let path = std::env::temp_dir().join(format!("workspace-prompts-{}", crate::engine::id())).join("prompts");
        fs::create_dir_all(&path).unwrap();
        for kind in kinds() {
            fs::write(path.join(default_file(kind)), format!("{kind} default instructions.")).unwrap();
        }
        fs::write(path.join("settings.json"), r#"{"planner":"sol_6_1_light","worker":"sol_6_1_light","updates":"events"}"#).unwrap();
        Self(path)
    }
    fn engine(&self) -> Engine { Engine::new(self.0.join("board")).unwrap() }
    fn input(&self, name: &str) -> Value {
        json!({"path":self.0.join(name),"settings":{"planner":"sol_6_1_light","worker":"sol_6_1_light"}})
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
fn missing_or_invalid_prompts_do_not_create_workspaces() {
    let fixture = Fixture::new();
    let mut engine = fixture.engine();
    fs::remove_file(fixture.0.join("worker.txt")).unwrap();
    assert!(create(&mut engine, fixture.input("missing"), &fixture.0).is_err());
    assert!(!fixture.0.join("missing").exists());
    fs::write(fixture.0.join("worker.txt"), "word ".repeat(201)).unwrap();
    assert!(create(&mut engine, fixture.input("oversize"), &fixture.0).is_err());
    assert!(!fixture.0.join("oversize").exists());
    fs::write(fixture.0.join("worker.txt"), " 
	").unwrap();
    assert!(create(&mut engine, fixture.input("blank"), &fixture.0).is_err());
}
#[test]
fn creation_keeps_the_workspace_empty_and_freezes_selection_hashes() {
    let fixture = Fixture::new();
    let mut engine = fixture.engine();
    let result = create(&mut engine, fixture.input("first"), &fixture.0).unwrap();
    let directory = PathBuf::from(result["workspace"].as_str().unwrap());
    let settings: WorkspaceSettings = serde_json::from_value(result["settings"].clone()).unwrap();
    assert_eq!(engine.state.workspace_ids[&directory].abbreviation, "FIR");
    assert_eq!(engine.state.workspace_ids[&directory].next, 0);
    assert_eq!(fs::read_dir(&directory).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect::<Vec<_>>(), [AGENTS_FILE, LIMITS_FILE]);
    let guide = fs::read_to_string(directory.join(AGENTS_FILE)).unwrap();
    let shown = directory.to_string_lossy().trim_start_matches(r"\\?\").to_owned();
    assert!(guide.contains(&shown));
    for route in ["/api/task-schema?workspace=W", "/api/tasks?workspace=W", "PATCH /api/tasks/{id}", "/api/updates?workspace=W&after=SEQ&wait=S", "/api/sessions/{id}"] {
        assert!(guide.contains(route), "{route}");
    }
    for admin in ["/api/deployment", "/api/wipe", "/api/config", "/api/state", "/api/analytics", "/api/workspaces"] {
        assert!(!guide.contains(admin), "{admin}");
    }
    assert_eq!(worker(&fixture.0, &settings, crate::history::Role::Worker, "implementation").unwrap()["text"], "worker_protocol default instructions.");
    assert!(!planner(&fixture.0, &settings).unwrap().is_empty());
    assert_eq!(engine.state.config.workspaces[&directory], settings);
    verify(&fixture.0, &settings).unwrap();
    fs::write(fixture.0.join("worker.txt"), "new ".repeat(200)).unwrap();
    assert!(worker(&fixture.0, &settings, crate::history::Role::Worker, "implementation").unwrap_err().to_string().contains("changed since the workspace selected it"));
    let second = create(&mut engine, fixture.input("second"), &fixture.0).unwrap();
    assert_ne!(result["settings"]["prompts"]["worker_protocol"]["sha256"], second["settings"]["prompts"]["worker_protocol"]["sha256"]);
    assert!(create(&mut engine, fixture.input("first"), &fixture.0).is_err());
    let loaded = fixture.engine();
    assert_eq!(loaded.state.config.workspaces[&directory], settings);
}

#[test]
fn every_role_uses_its_own_prompt_and_settings_default_from_the_prompts_folder() {
    let fixture = Fixture::new();
    let mut engine = fixture.engine();
    let plain = create(&mut engine, json!({"path":fixture.0.join("plain")}), &fixture.0).unwrap();
    let settings: WorkspaceSettings = serde_json::from_value(plain["settings"].clone()).unwrap();
    assert_eq!((settings.updates, settings.planner.as_deref()), (crate::history::Updates::Events, Some("sol_6_1_light")));
    let ui = worker(&fixture.0, &settings, crate::history::Role::UiCode, "implementation").unwrap();
    assert_eq!((ui["role"].as_str(), ui["text"].as_str()), (Some("ui-code"), Some("ui-code default instructions.")));
    let abstraction = worker(&fixture.0, &settings, crate::history::Role::Abstraction, "implementation").unwrap();
    assert_eq!((abstraction["candidate"].as_str(), abstraction["text"].as_str()), (Some("abstraction.txt"), Some("abstraction default instructions.")));
    assert_eq!(worker(&fixture.0, &settings, crate::history::Role::Worker, "implementation").unwrap()["role"], "worker_protocol");
    assert_eq!(planner(&fixture.0, &settings).unwrap(), "project_planner default instructions.");
    fs::write(fixture.0.join("planner.txt"), "Edited later.").unwrap();
    assert!(planner(&fixture.0, &settings).unwrap_err().to_string().contains("planner.txt"));
}

#[test]
fn workspaces_pinned_to_removed_candidates_are_refused() {
    let fixture = Fixture::new();
    let mut engine = fixture.engine();
    let mut settings: WorkspaceSettings = serde_json::from_value(create(&mut engine, fixture.input("old"), &fixture.0).unwrap()["settings"].clone()).unwrap();
    let prompts = settings.prompts.as_mut().unwrap();
    prompts.project_planner = Document { candidate: "mission.md".into(), sha256: "0".into(), default: false };
    assert!(planner(&fixture.0, &settings).unwrap_err().to_string().contains("removed prompt candidate project_planner/mission.md"));
}

#[test]
fn line_endings_do_not_change_a_prompt_fingerprint() {
    assert_eq!(digest("One line.\r\nTwo lines.\r\n"), digest("One line.\nTwo lines.\n"));
    assert_ne!(digest("One line.\n"), digest("One line, changed.\n"));
}

#[test]
fn every_shipped_prompt_is_valid() {
    let shipped = defaults(&root()).unwrap();
    assert_eq!(shipped["prompts"].as_object().unwrap().len(), kinds().len());
}

#[test]
fn review_and_replan_stages_use_their_own_prompts_instead_of_the_task_role() {
    let fixture = Fixture::new();
    let mut engine = fixture.engine();
    let settings: WorkspaceSettings = serde_json::from_value(create(&mut engine, fixture.input("staged"), &fixture.0).unwrap()["settings"].clone()).unwrap();
    let pick = |role, stage: &str| worker(&fixture.0, &settings, role, stage).unwrap()["role"].as_str().unwrap().to_owned();
    use crate::history::Role::{UiCode, Worker};
    assert_eq!([pick(UiCode, "implementation"), pick(UiCode, "review"), pick(UiCode, "replan")], ["ui-code", "reviewer", "replanner"]);
    assert_eq!([pick(Worker, "implementation"), pick(Worker, "review"), pick(Worker, "replan")], ["worker_protocol", "reviewer", "replanner"]);
}
