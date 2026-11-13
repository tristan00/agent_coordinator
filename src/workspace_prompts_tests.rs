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
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 3);
    let limits: serde_json::Value = serde_json::from_slice(&fs::read(directory.join(LIMITS_FILE)).unwrap()).unwrap();
    assert_eq!(limits, serde_json::to_value(&settings.limits).unwrap());
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

#[test]
fn generation_deals_every_prompt_before_repeating_and_is_reproducible_by_seed() {
    let fixture = Fixture::new();
    for index in 0..3 {
        fs::write(fixture.0.join(format!("project_planner/p{index}.md")), format!("Planner candidate {index}.")).unwrap();
        fs::write(fixture.0.join(format!("worker_protocol/w{index}.md")), format!("Worker candidate {index}.")).unwrap();
    }
    let run = |prefix: &str| {
        let mut engine = fixture.engine();
        let paths: Vec<_> = (0..6).map(|i| fixture.0.join(format!("{prefix}{i}"))).collect();
        generate(&mut engine, json!({"paths":paths,"seed":42}), &fixture.0).unwrap()
    };
    let first = run("a");
    let pick = |result: &Value, role: &str| -> Vec<String> { result["workspaces"].as_array().unwrap().iter().map(|w| w["settings"]["prompts"][role]["candidate"].as_str().unwrap().to_owned()).collect() };
    for role in ["project_planner", "worker_protocol"] {
        let names = pick(&first, role);
        assert_eq!(names[..3].iter().collect::<std::collections::BTreeSet<_>>().len(), 3, "{role}: {names:?}");
        assert_eq!(names[..3], names[3..]);
    }
    let second = run("b");
    let models = |result: &Value| -> Vec<(Value, Value, Value)> { result["workspaces"].as_array().unwrap().iter().map(|w| (w["settings"]["planner"].clone(), w["settings"]["worker"].clone(), w["settings"]["limits"].clone())).collect() };
    assert_eq!((models(&first), pick(&first, "project_planner")), (models(&second), pick(&second, "project_planner")));
    for workspace in first["workspaces"].as_array().unwrap() {
        assert_ne!(workspace["settings"]["planner"], "luna_6_high");
    }
    let mut engine = fixture.engine();
    assert!(generate(&mut engine, json!({"paths":[fixture.0.join("a0")]}), &fixture.0).is_err());
    assert!(generate(&mut engine, json!({"paths":[]}), &fixture.0).is_err());
}
