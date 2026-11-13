use super::*;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("coordinator-adapters-{}", crate::engine::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn claude_stream_results_prefer_structured_output_and_sum_cached_input() {
    let events = [
        json!({"type":"system","subtype":"init","model":"claude-haiku-5-5","claude_code_version":"2.1.323"}),
        json!({"type":"assistant","message":{}}),
        json!({"type":"result","is_error":false,"num_turns":3,"result":"ignored","structured_output":{"status":"completed"},
            "usage":{"input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":5,"output_tokens":7}}),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n");
    let (output, components, init) = claude_result(&events).unwrap();
    assert_eq!(output, json!({"status":"completed"}));
    assert_eq!(components["fields"]["input_tokens"]["value"], 115);
    assert_eq!(components["fields"]["cached_input_tokens"]["value"], 100);
    assert_eq!(crate::telemetry::total(&components), Some(122));
    assert_eq!(init.unwrap()["model"], "claude-haiku-5-5");
}

#[test]
fn claude_results_fall_back_to_json_in_text_and_report_errors() {
    let text = json!({"type":"result","is_error":false,"result":"Done.\n```json\n{\"status\":\"blocked\"}\n```","usage":{}}).to_string();
    assert_eq!(claude_result(&text).unwrap().0, json!({"status":"blocked"}));
    let failed = json!({"type":"result","is_error":true,"result":"auth"}).to_string();
    assert!(claude_result(&failed).is_err());
    assert!(claude_result("not json").is_err());
    let prose = json!({"type":"result","is_error":false,"result":"no object here","usage":{}}).to_string();
    assert!(claude_result(&prose).is_err());
}

#[test]
fn hosted_session_variables_are_not_inherited_by_child_claude_workers() {
    for name in ["CLAUDECODE", "CLAUDE_CODE_SESSION_ID", "CLAUDE_CODE_MESSAGING_TOKEN", "CLAUDE_AGENT_SDK_VERSION", "CLAUDE_PID", "CLAUDE_EFFORT"] {
        assert!(hosted_claude_environment(name), "{name}");
    }
    for name in ["ANTHROPIC_API_KEY", "PATH", "CODEX_HOME"] {
        assert!(!hosted_claude_environment(name), "{name}");
    }
}

#[test]
fn replies_are_parsed_from_the_outermost_json_object() {
    assert_eq!(json_object("x {\"a\":{\"b\":1}} y").unwrap(), json!({"a":{"b":1}}));
    assert!(json_object("} {").is_err());
}

#[test]
fn claude_executable_is_found_as_exe_or_npm_shim_on_path() {
    let root = Temp::new();
    let (first, second) = (root.0.join("a"), root.0.join("b"));
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    std::fs::write(second.join("claude.cmd"), "").unwrap();
    let path = std::env::join_paths([&first, &second]).unwrap();
    assert_eq!(executable("claude", Some(path.clone())), Some(second.join("claude.cmd")));
    let shim = second.join("node_modules/@anthropic-ai/claude-code/bin");
    std::fs::create_dir_all(&shim).unwrap();
    std::fs::write(shim.join("claude.exe"), "").unwrap();
    assert_eq!(executable("claude", Some(path.clone())), Some(shim.join("claude.exe")));
    std::fs::write(first.join("claude.exe"), "").unwrap();
    assert_eq!(executable("claude", Some(path)), Some(first.join("claude.exe")));
    assert_eq!(executable("claude", None), None);
}
