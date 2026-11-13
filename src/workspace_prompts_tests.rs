use super::*;

#[test]
fn workspace_agents_are_explicit_and_preserve_exact_model_effort_pairs() {
    let pairs = [("luna_6_high", "gpt-6-luna", "high"), ("sol_6_1_light", "gpt-6.1-sol", "low"), ("sol_6_1_medium", "gpt-6.1-sol", "medium")];
    for (name, model, effort) in pairs {
        let candidate: crate::history::AgentCandidate = serde_json::from_value(json!(name)).unwrap();
        candidate.validate().unwrap();
        assert_eq!((candidate.model(), candidate.effort()), (model, effort));
    }
    let historical: crate::history::AgentCandidate = serde_json::from_value(json!("sol_light")).unwrap();
    assert_eq!(historical.model(), "gpt-6-sol");
    assert!(historical.validate().is_err());
    assert!(WorkspaceSettings::default().validate().is_err());
    assert_eq!(crate::history::agent_candidates().as_array().unwrap().len(), 3);
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("workspace-prompts-{}", crate::engine::id()));
        fs::create_dir_all(path.join("project_planner")).unwrap();
        fs::create_dir_all(path.join("worker_protocol")).unwrap();
        Self(path)
    }
    fn engine(&self) -> Engine { Engine::new(self.0.join("board")).unwrap() }
    fn input(&self, name: &str) -> Value {
        json!({"path":self.0.join(name),"project_planner":"selected.md","worker_protocol":"selected.md",
            "settings":{"planner":"sol_6_1_light","worker":"sol_6_1_light"}})
    }
    fn candidates(&self) {
        fs::write(self.0.join("project_planner/selected.md"), "Planner candidate unique instructions.").unwrap();
        fs::write(self.0.join("worker_protocol/selected.md"), "Worker candidate unique instructions.").unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self.0.canonicalize().unwrap().starts_with(std::env::temp_dir().canonicalize().unwrap()));
        fs::remove_dir_all(&self.0).unwrap();
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
fn creation_copies_both_selections_and_freezes_their_hashes() {
    let fixture = Fixture::new();
    fixture.candidates();
    let mut engine = fixture.engine();
    let result = create(&mut engine, fixture.input("first"), &fixture.0).unwrap();
    let directory = PathBuf::from(result["workspace"].as_str().unwrap());
    let settings: WorkspaceSettings = serde_json::from_value(result["settings"].clone()).unwrap();
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
    assert_eq!(worker(&directory, &settings).unwrap()["text"], "Worker candidate unique instructions.");
    assert_eq!(engine.state.config.workspaces[&directory], settings);
    fs::write(fixture.0.join("worker_protocol/selected.md"), "new ".repeat(200)).unwrap();
    verify(&directory, &settings).unwrap();
    let second = create(&mut engine, fixture.input("second"), &fixture.0).unwrap();
    assert_ne!(result["settings"]["prompts"]["worker_protocol"]["sha256"], second["settings"]["prompts"]["worker_protocol"]["sha256"]);
    assert!(create(&mut engine, fixture.input("first"), &fixture.0).is_err());
    fs::write(directory.join("WORKER_PROTOCOL.md"), "Changed workspace content.").unwrap();
    assert!(worker(&directory, &settings).is_err());
    let loaded = fixture.engine();
    assert_eq!(loaded.state.config.workspaces[&directory], settings);
}
