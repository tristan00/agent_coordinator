use super::*;

#[test]
fn words_counts_whitespace_separated_tokens() {
    assert_eq!(words(""), 0);
    assert_eq!(words(" \t\n "), 0);
    assert_eq!(words("one"), 1);
    assert_eq!(words("  one\ttwo\nthree  "), 3);
    assert_eq!(words("hyphen-ated words.count"), 2);
}

#[test]
fn clip_keeps_leading_words_and_normalizes_spacing() {
    assert_eq!(clip("a  b\tc d", 2), "a b");
    assert_eq!(clip("a b", 5), "a b");
    assert_eq!(clip(" a\n\nb ", 5), "a b");
    assert_eq!(clip("a b", 0), "");
    assert_eq!(clip("", 3), "");
}

#[test]
fn coordination_prose_detects_task_ids_digests_and_agents_file() {
    assert!(coordination_prose("Read AGENTS.md first"));
    assert!(coordination_prose("see agents.MD please"));
    assert!(coordination_prose("Depends on 1731456789012-1234-0 finishing"));
    assert!(coordination_prose("(1731456789012-1-2-3)"));
    assert!(coordination_prose(&format!("digest {}", "0123456789abcdef".repeat(3))));
    assert!(!coordination_prose("Range 1234-5678 is fine"));
    assert!(!coordination_prose("Short hex deadbeef"));
    assert!(!coordination_prose(&"g".repeat(40)));
    assert!(!coordination_prose(&"a".repeat(39)));
    assert!(!coordination_prose("173145678901-1234-0"));
    assert!(!coordination_prose("1731456789012-1234"));
    assert!(!coordination_prose("1731456789012--0"));
    assert!(!coordination_prose("agents file"));
}

#[test]
fn limits_defaults_validate_and_publish_strikes() {
    let limits = Limits::default();
    assert_eq!((limits.words, limits.title_words, limits.criteria, limits.summary_words, limits.findings, limits.finding_words), (120, 12, 3, 40, 3, 30));
    limits.validate().unwrap();
    let value = limits.value();
    assert_eq!(value["strikes"], json!(STRIKES));
    assert_eq!(value["titleWords"], 12);
    assert_eq!(value["findingWords"], 30);
}

#[test]
fn limits_reject_zero_values_and_unknown_fields() {
    for field in ["words", "titleWords", "criteria", "summaryWords", "findings", "findingWords"] {
        let mut value = json!({});
        value[field] = json!(0);
        let limits: Limits = serde_json::from_value(value).unwrap();
        assert!(limits.validate().is_err(), "{field}");
    }
    let partial: Limits = serde_json::from_value(json!({"words":5})).unwrap();
    assert_eq!(partial, Limits { words: 5, ..Limits::default() });
    assert!(serde_json::from_value::<Limits>(json!({"maxWords":5})).is_err());
    assert!(serde_json::from_value::<Limits>(json!({"words":-1})).is_err());
}

#[test]
fn workspace_settings_reject_unknown_fields_and_default_to_polling() {
    assert!(serde_json::from_value::<WorkspaceSettings>(json!({"bogus":1})).is_err());
    assert!(serde_json::from_value::<WorkspaceSettings>(json!({"limits":{"bogus":1}})).is_err());
    let settings: WorkspaceSettings = serde_json::from_value(json!({"limits":{"criteria":1},"updates":"events"})).unwrap();
    assert_eq!(settings.limits.criteria, 1);
    assert_eq!(settings.updates, Updates::Events);
    assert_eq!(WorkspaceSettings::default().updates, Updates::Poll);
    assert!(serde_json::from_value::<WorkspaceSettings>(json!({"updates":"push"})).is_err());
}

#[test]
fn attributes_validate_priority_deploy_and_classifications() {
    let parse = |value: Value| serde_json::from_value::<Attributes>(value);
    let defaults = parse(json!({})).unwrap();
    defaults.validate().unwrap();
    assert_eq!(defaults.priority(), 2);
    assert!(defaults.lane == Lane::Code && defaults.size == Size::Small && defaults.artifact == Artifact::None && !defaults.deploy);
    parse(json!({"priority":0})).unwrap().validate().unwrap();
    parse(json!({"priority":3})).unwrap().validate().unwrap();
    assert!(parse(json!({"priority":4})).unwrap().validate().is_err());
    assert!(parse(json!({"deploy":true})).unwrap().validate().is_err());
    assert!(parse(json!({"deploy":true,"artifact":"report"})).unwrap().validate().is_err());
    parse(json!({"deploy":true,"artifact":"candidate"})).unwrap().validate().unwrap();
    assert!(parse(json!({"languages":["cobol"]})).is_err());
    assert!(parse(json!({"functions":["Testing"]})).is_err());
    assert!(parse(json!({"lane":"marketing"})).is_err());
    assert!(parse(json!({"needs":["quantum"]})).is_err());
    let mut attributes = parse(json!({"languages":["rust","css","rust"],"functions":["testing"]})).unwrap();
    assert_eq!(attributes.languages.iter().map(String::as_str).collect::<Vec<_>>(), ["css", "rust"]);
    attributes.validate().unwrap();
    attributes.languages.insert("cobol".into());
    assert!(attributes.validate().is_err());
}

#[test]
fn classification_schema_lists_registries() {
    let schema = classification_schema();
    assert_eq!(schema["languages"]["items"]["enum"].as_array().unwrap().len(), LANGUAGES.len());
    assert_eq!(schema["functions"]["items"]["enum"].as_array().unwrap().len(), FUNCTIONS.len());
    assert!(LANGUAGES.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(FUNCTIONS.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn usage_tokens_prefer_reported_total_over_components() {
    let components = json!({"fields":{"input_tokens":{"value":10},"output_tokens":{"value":5}}});
    let usage = |total: Option<u64>, components: Option<Value>| Usage { total_tokens: total, components, source: "test".into() };
    assert_eq!(usage(Some(7), Some(components.clone())).tokens(), Some(7));
    assert_eq!(usage(None, Some(components)).tokens(), Some(15));
    assert_eq!(usage(None, None).tokens(), None);
}

#[test]
fn versions_publish_schema_reasons_and_limits() {
    let value = versions();
    assert_eq!(value["schemaVersion"], json!(SCHEMA_VERSION));
    assert_eq!(value["metricDefinitionVersion"], json!(METRIC_DEFINITION_VERSION));
    assert_eq!(value["dependencyReasons"], json!(["uses_artifact", "uses_api", "uses_schema", "uses_data"]));
    assert_eq!(value["limits"]["strikes"], json!(STRIKES));
    assert_eq!(value["workspace"]["updates"], "poll");
}

#[test]
fn events_number_sequentially_and_take_attempt_details() {
    let temp = crate::test_support::Temp::new();
    let mut task: crate::engine::Task = serde_json::from_value(json!({"id":"t","title":"T","outcome":"O","cwd":temp.path(),"acceptance":[],"dependencies":[],
        "stage":"implementation","stopped":false,"attempts":[],"active":"a1","feedback":[],"failure":"","retryAt":0,"revision":4})).unwrap();
    let first = Event::new(&task, "created", "test", json!({}));
    assert_eq!((first.sequence, first.revision, first.attempt_id.as_deref()), (1, 4, Some("a1")));
    task.history.push(first);
    let second = Event::new(&task, "started", "test", Value::Null).attempt("a2", 3, Some(42));
    assert_eq!((second.sequence, second.revision, second.attempt_id.as_deref(), second.at), (2, 3, Some("a2"), 42));
    let unchanged = Event::new(&task, "x", "test", Value::Null);
    assert_eq!(unchanged.clone().attempt("a3", 1, None).at, unchanged.at);
}
