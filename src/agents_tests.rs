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
fn families_never_cross_hosted_vendors_and_local_is_always_allowed() {
    let defined = BTreeMap::new();
    for (id, agent) in catalog().into_iter().filter(|(_, a)| !a.retired) {
        let claude = resolve(&defined, &id, Some(Family::Claude));
        let codex = resolve(&defined, &id, Some(Family::Codex));
        let unknown = resolve(&defined, &id, None);
        match agent.provider {
            Provider::Local => assert!(claude.is_ok() && codex.is_ok() && unknown.is_ok(), "{id}"),
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
    let custom = Agent { provider: Provider::Local, harness: Harness::OpenaiCompatible, model: "tiny".into(), effort: None, endpoint: Some("http://127.0.0.1:9999/v1".into()), retired: false, note: None, worker_only: false };
    let mut defined = BTreeMap::from([("tiny".to_string(), custom.clone())]);
    assert_eq!(resolve(&defined, "tiny", None).unwrap(), custom);
    defined.get_mut("tiny").unwrap().endpoint = Some("https://example.com/v1".into());
    assert!(resolve(&defined, "tiny", None).is_err());
    defined.get_mut("tiny").unwrap().endpoint = None;
    assert!(resolve(&defined, "tiny", None).is_err());
    let crossed = Agent { provider: Provider::Openai, harness: Harness::ClaudeCode, model: "gpt".into(), effort: None, endpoint: None, retired: false, note: None, worker_only: false };
    defined.insert("crossed".into(), crossed);
    assert!(resolve(&defined, "crossed", Some(Family::Claude)).unwrap_err().to_string().contains("own vendor"));
    let blank = Agent { provider: Provider::Anthropic, harness: Harness::ClaudeCode, model: " ".into(), effort: None, endpoint: None, retired: false, note: None, worker_only: false };
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
    assert!(allowed("claude_haiku_5_5_low") && allowed("local_qwen3_4b") && !allowed("sol_6_1_light"));
    assert_eq!(listed["family"], "claude");
}

#[test]
fn local_endpoints_must_be_loopback_http() {
    assert!(local_endpoint("http://127.0.0.1:8120/v1"));
    assert!(local_endpoint("http://localhost:11434/v1"));
    assert!(!local_endpoint("http://10.0.0.2:8120/v1"));
    assert!(!local_endpoint("https://127.0.0.1:8120/v1"));
}

#[test]
fn enabled_lists_workspace_definitions_or_the_family_catalog() {
    let all = enabled(&BTreeMap::new(), Some(Family::Claude));
    assert!(all.contains_key("claude_haiku_5_5_low") && all.contains_key("local_granite_3b"));
    assert!(!all.contains_key("sol_6_1_light") && !all.contains_key("sol_light"));
    let defined = BTreeMap::from([("local_qwen3_4b".to_string(), catalog()["local_qwen3_4b"].clone()), ("sol_6_1_light".to_string(), catalog()["sol_6_1_light"].clone())]);
    let listed = enabled(&defined, Some(Family::Claude));
    assert_eq!(listed.keys().collect::<Vec<_>>(), ["local_qwen3_4b"]);
}

#[test]
fn local_catalog_models_fit_the_forty_percent_gpu_budget_notes() {
    for (id, agent) in catalog().into_iter().filter(|(_, a)| a.provider == Provider::Local) {
        let gigabytes: f64 = agent.note.as_deref().unwrap().split("about ").nth(1).unwrap().split(' ').next().unwrap().parse().unwrap();
        assert!(gigabytes <= 12.8 * 0.75, "{id} weights leave too little cache inside 40% of a 32 GB GPU");
        assert_eq!(agent.endpoint.as_deref(), Some("http://127.0.0.1:8120/v1"));
    }
}

#[test]
fn served_models_are_read_from_the_openai_model_listing() {
    let models = json!({"object":"list","data":[{"id":"qwen3-4b-2507-fp8"},{"id":"other"}]});
    assert!(served(&models, "qwen3-4b-2507-fp8"));
    assert!(!served(&models, "granite4.2-3b-fp8"));
    assert!(!served(&json!({"error":"x"}), "other"));
    let unreachable = Agent { provider: Provider::Local, harness: Harness::OpenaiCompatible, model: "m".into(), effort: None, endpoint: Some("http://127.0.0.1:9/v1".into()), retired: false, note: None, worker_only: false };
    assert_eq!(available(&unreachable), Some(false));
    assert!(require_served("unreachable", &unreachable).unwrap_err().to_string().contains("127.0.0.1:9"));
    serve("http://127.0.0.1:9/v1", &["m"]);
    assert_eq!(available(&unreachable), Some(true));
    assert!(require_served("unreachable", &unreachable).is_ok());
    assert_eq!(available(&catalog()["sol_6_1_light"]), None);
    assert!(require_served("sol_6_1_light", &catalog()["sol_6_1_light"]).is_ok());
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
