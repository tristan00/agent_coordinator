use super::*;
use std::collections::HashMap;

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    move |name| map.get(name).cloned()
}

#[test]
fn launcher_family_comes_from_explicit_setting_or_a_single_detected_tool() {
    assert_eq!(detect(env(&[("CLAUDECODE", "1")])), Some(Family::Claude));
    assert_eq!(detect(env(&[("CLAUDE_CODE_ENTRYPOINT", "cli")])), Some(Family::Claude));
    assert_eq!(detect(env(&[("CODEX_WINDOWS_REGISTERED_CORE", "x")])), Some(Family::Codex));
    assert_eq!(detect(env(&[("CODEX_THREAD_ID", "x")])), Some(Family::Codex));
    assert_eq!(detect(env(&[("CLAUDECODE", "1"), ("CODEX_THREAD_ID", "x")])), None);
    assert_eq!(detect(env(&[("CODEX_HOME", "C:/x")])), None);
    assert_eq!(detect(env(&[])), None);
    assert_eq!(detect(env(&[("AGENT_FAMILY", "Codex"), ("CLAUDECODE", "1")])), Some(Family::Codex));
    assert_eq!(detect(env(&[("AGENT_FAMILY", "claude")])), Some(Family::Claude));
    assert_eq!(detect(env(&[("AGENT_FAMILY", "other"), ("CLAUDECODE", "1")])), None);
}

#[test]
fn families_never_cross_hosted_vendors() {
    let defined = BTreeMap::new();
    for (id, agent) in catalog().into_iter().filter(|(_, a)| !a.retired) {
        let claude = resolve(&defined, &id, Some(Family::Claude));
        let codex = resolve(&defined, &id, Some(Family::Codex));
        let unknown = resolve(&defined, &id, None);
        match agent.provider {
            Provider::Anthropic => assert!(claude.is_ok() && codex.is_err() && unknown.is_err(), "{id}"),
            Provider::Openai => assert!(claude.is_err() && codex.is_ok() && unknown.is_err(), "{id}"),
        }
    }
}

#[test]
fn catalog_keeps_existing_codex_selections_exact() {
    let all = catalog();
    for (id, model, effort) in [("luna_6_high", "gpt-6-luna", "high"), ("sol_6_1_light", "gpt-6.1-sol", "low"), ("sol_6_1_medium", "gpt-6.1-sol", "medium")] {
        let agent = &all[id];
        assert_eq!((agent.provider, agent.harness, agent.model.as_str(), agent.effort()), (Provider::Openai, Harness::Codex, model, effort));
    }
    let retired = resolve(&BTreeMap::new(), "sol_light", Some(Family::Codex)).unwrap_err().to_string();
    assert!(retired.contains("retired"));
    assert!(resolve(&BTreeMap::new(), "missing", Some(Family::Codex)).unwrap_err().to_string().contains("Unknown"));
}

#[test]
fn workspace_definitions_override_the_catalog_and_are_validated() {
    let custom = Agent { provider: Provider::Anthropic, harness: Harness::ClaudeCode, model: "tiny".into(), effort: Some("low".into()), retired: false, note: None, worker_only: false };
    let mut defined = BTreeMap::from([("tiny".to_string(), custom.clone())]);
    assert_eq!(resolve(&defined, "tiny", Some(Family::Claude)).unwrap(), custom);
    assert!(resolve(&defined, "tiny", Some(Family::Codex)).is_err());
    let crossed = Agent { provider: Provider::Openai, harness: Harness::ClaudeCode, model: "gpt".into(), effort: None, retired: false, note: None, worker_only: false };
    defined.insert("crossed".into(), crossed);
    assert!(resolve(&defined, "crossed", Some(Family::Claude)).unwrap_err().to_string().contains("own vendor"));
    let blank = Agent { provider: Provider::Anthropic, harness: Harness::ClaudeCode, model: " ".into(), effort: None, retired: false, note: None, worker_only: false };
    defined.insert("blank".into(), blank);
    assert!(resolve(&defined, "blank", Some(Family::Claude)).is_err());
}

#[test]
fn agent_definitions_reject_unknown_fields_and_round_trip() {
    let value = json!({"provider":"anthropic","harness":"claude_code","model":"claude-haiku-5-5","effort":"low"});
    let agent: Agent = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&agent).unwrap(), value);
    assert!(serde_json::from_value::<Agent>(json!({"provider":"anthropic","harness":"claude_code","model":"m","extra":1})).is_err());
}

#[test]
fn listing_marks_what_the_launcher_family_allows() {
    let listed = listing(&BTreeMap::new(), Some(Family::Claude));
    let agents = listed["agents"].as_array().unwrap();
    assert!(agents.iter().all(|a| a["id"] != "sol_light"));
    let allowed = |id: &str| agents.iter().find(|a| a["id"] == id).unwrap()["allowed"].as_bool().unwrap();
    assert!(allowed("claude_haiku_5_5_low") && !allowed("sol_6_1_light"));
    assert_eq!(listed["family"], "claude");
}

#[test]
fn enabled_lists_workspace_definitions_or_the_family_catalog() {
    let all = enabled(&BTreeMap::new(), Some(Family::Claude));
    assert!(all.contains_key("claude_haiku_5_5_low") && all.contains_key("claude_opus_5_5_medium"));
    assert!(!all.contains_key("sol_6_1_light") && !all.contains_key("sol_light"));
    let defined = BTreeMap::from([("claude_haiku_5_5_low".to_string(), catalog()["claude_haiku_5_5_low"].clone()), ("sol_6_1_light".to_string(), catalog()["sol_6_1_light"].clone())]);
    let listed = enabled(&defined, Some(Family::Claude));
    assert_eq!(listed.keys().collect::<Vec<_>>(), ["claude_haiku_5_5_low"]);
}

#[test]
fn luna_is_a_worker_only_agent() {
    let family = Some(Family::Codex);
    let settings = |planner: &str, worker: &str| -> crate::history::WorkspaceSettings { serde_json::from_value(json!({"planner":planner,"worker":worker})).unwrap() };
    assert!(settings("luna_6_high", "sol_6_1_light").validate(family).unwrap_err().to_string().contains("only be a worker"));
    settings("sol_6_1_light", "luna_6_high").validate(family).unwrap();
    let listed = listing(&BTreeMap::new(), family);
    let luna = listed["agents"].as_array().unwrap().iter().find(|a| a["id"] == "luna_6_high").unwrap();
    assert_eq!((luna["planner"].as_bool(), luna["agent"]["workerOnly"].as_bool()), (Some(false), Some(true)));
    let roundtrip: Agent = serde_json::from_value(serde_json::to_value(&catalog()["luna_6_high"]).unwrap()).unwrap();
    assert!(roundtrip.worker_only);
}
