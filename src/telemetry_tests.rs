use super::*;
use crate::test_support::Temp;

fn events(lines: &[&str]) -> (Temp, std::path::PathBuf) {
    let temp = Temp::new();
    let path = temp.path().join("events.jsonl");
    std::fs::write(&path, lines.join("\n")).unwrap();
    (temp, path)
}

#[test]
fn resource_usage_sums_turn_completed_events() {
    let (_temp, path) = events(&[
        "not json",
        r#"{"type":"turn.started"}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":5,"reasoning_output_tokens":1}}"#,
        r#"{"type":"item.completed","usage":{"input_tokens":1000}}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":20,"output_tokens":7}}"#,
    ]);
    let usage = resource_usage(&path).unwrap();
    assert_eq!(usage["source"], "events.jsonl:turn.completed");
    let fields = &usage["fields"];
    assert_eq!(fields["input_tokens"], json!({"value":30,"measuredTurns":2,"turns":2}));
    assert_eq!(fields["output_tokens"]["value"], 12);
    assert_eq!(fields["cached_input_tokens"], json!({"value":null,"measuredTurns":1,"turns":2}));
    assert_eq!(fields["cache_write_input_tokens"], json!({"value":null,"measuredTurns":0,"turns":2}));
    assert_eq!(total(&usage), Some(42));
}

#[test]
fn resource_usage_is_absent_without_completed_turns() {
    let (temp, path) = events(&[r#"{"type":"turn.started"}"#, ""]);
    assert!(resource_usage(&path).is_none());
    assert!(resource_usage(&temp.path().join("missing.jsonl")).is_none());
}

#[test]
fn total_requires_input_and_output_without_overflow() {
    let value = |input: Value, output: Value| json!({"fields":{"input_tokens":{"value":input},"output_tokens":{"value":output}}});
    assert_eq!(total(&value(json!(1), json!(2))), Some(3));
    assert_eq!(total(&value(json!(1), Value::Null)), None);
    assert_eq!(total(&value(json!(u64::MAX), json!(1))), None);
    assert_eq!(total(&json!({})), None);
}

#[test]
fn usage_reset_reads_the_last_usage_limit_line() {
    assert_eq!(usage_reset("all good\nnothing here"), None);
    assert_eq!(usage_reset("You've hit your usage limit."), Some(None));
    assert_eq!(usage_reset("usage limit; try again at someday."), Some(None));
    let first = usage_reset("usage limit, try again at Jan 2nd, 2026 1:05 PM.").unwrap().unwrap();
    let last = usage_reset("usage limit, try again at Jan 2nd, 2026 1:05 PM.\nusage limit, try again at Jan 2nd, 2026 2:05 PM.\ntrailing").unwrap().unwrap();
    assert_eq!(last - first, 3_600_000);
}

#[test]
fn local_times_parse_twelve_hour_clock_and_month_names() {
    let at = |text: &str| parse_local(text).unwrap();
    assert_eq!(at("Jan 2nd, 2026 1:05 PM") - at("Jan 2nd, 2026 12:05 PM"), 3_600_000);
    assert_eq!(at("Jan 2nd, 2026 12:05 PM") - at("Jan 2nd, 2026 12:05 AM"), 12 * 3_600_000);
    assert_eq!(at("Jan 2, 2026 13:05"), at("january 2nd 2026 1:05 pm"));
    assert_eq!(at("Sept 3rd, 2026 8:00 AM") - at("Sep 2nd, 2026 8:00 AM"), 24 * 3_600_000);
    let noon = at("Jan 2nd, 2026 12:00 PM");
    let utc_noon = 1_767_355_200_000_u64;
    assert!(noon.abs_diff(utc_noon) <= 14 * 3_600_000);
    for invalid in ["", "Smarch 2, 2026 1:00 PM", "Jan x, 2026 1:00 PM", "Jan 2, year 1:00 PM", "Jan 2, 2026 1pm", "Jan 2, 2026"] {
        assert_eq!(parse_local(invalid), None, "{invalid}");
    }
}
