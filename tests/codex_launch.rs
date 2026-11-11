use serde_json::json;
use std::{fs, path::PathBuf, process::Command};

#[test]
fn codex_child_excludes_registered_core_and_preserves_worker_context() {
    let directory = std::env::current_exe().unwrap().parent().unwrap().join(format!("codex-launch-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let request = directory.join("request.json");
    let result = directory.join("result.json");
    let schema = directory.join("schema.json");
    let shim = directory.join("codex.cmd");
    fs::write(
        &shim,
        concat!(
            "@echo off\r\n",
            "if defined CODEX_WINDOWS_REGISTERED_CORE (\r\n",
            "    echo CODEX_WINDOWS_REGISTERED_CORE reached the child\r\n",
            "    exit /b 42\r\n",
            ")\r\n",
            "if /I not \"%CODEX_HOME%\"==\"%EXPECTED_CODEX_HOME%\" exit /b 43\r\n",
            "echo %COORDINATOR_REQUEST%\r\n",
            "echo %COORDINATOR_RESULT%\r\n",
            "echo %*\r\n",
            "echo analytics HTTP 400\r\n",
            "echo {\"type\":\"item.started\",\"item\":{\"type\":\"mcp_tool_call\"}}\r\n",
            "echo {\"type\":\"item.completed\",\"item\":{\"type\":\"command_execution\",\"exit_code\":9,\"aggregated_output\":\"assertion failed\"}}\r\n",
            "echo {\"type\":\"item.completed\",\"item\":{\"type\":\"mcp_tool_call\",\"result\":null,\"error\":null}}\r\n",
            ">&2 echo OpenAI Codex v0.167.0\r\n",
            ">&2 echo model: observed-model\r\n",
            ">&2 echo reasoning effort: medium\r\n",
            ">&2 echo user\r\n",
            ">&2 echo model: prompt-content\r\n",
            ">&2 echo tokens used\r\n",
            ">&2 echo 1,234\r\n",
        ),
    )
    .unwrap();
    fs::write(&request, json!({"stage":"implementation","task":{"model":"worker-model","effort":"low"}}).to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_agent-coordinator"))
        .arg("--codex")
        .env("CODEX_BIN", &shim)
        .env("CODEX_WINDOWS_REGISTERED_CORE", "1")
        .env_remove("CODEX_HOME")
        .env("USERPROFILE", &directory)
        .env("EXPECTED_CODEX_HOME", directory.join(".codex"))
        .env("COORDINATOR_REQUEST", &request)
        .env("COORDINATOR_RESULT", &result)
        .output();
    let schema_value: serde_json::Value = serde_json::from_slice(&fs::read(&schema).unwrap()).unwrap();
    let launch = directory.join("launch-settings.json");
    let evidence: serde_json::Value = serde_json::from_slice(&fs::read(&launch).unwrap()).unwrap();
    let usage = directory.join("usage.json");
    let measured: serde_json::Value = serde_json::from_slice(&fs::read(&usage).unwrap()).unwrap();
    let events = directory.join("events.jsonl");
    let trace = directory.join("review-trace.jsonl");
    let stdout = fs::read_to_string(&events).unwrap();
    let substantive = fs::read_to_string(&trace).unwrap();
    for path in [&shim, &request, &schema, &launch, &usage, &events, &trace].into_iter().filter(|path| path.exists()) {
        fs::remove_file(path).unwrap();
    }
    fs::remove_dir(&directory).unwrap();

    let output = output.unwrap();
    assert!(output.status.success(), "{stdout}{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(schema_value["properties"]["status"]["enum"], json!(["completed", "blocked"]));
    assert_eq!(evidence["forwardedModel"], "worker-model");
    assert_eq!(evidence["forwardedEffort"], "low");
    assert_eq!(evidence["model"], "observed-model");
    assert_eq!(evidence["effort"], "medium");
    assert_eq!(measured, json!({"totalTokens":1234,"source":"codex_token_footer"}));
    assert!(stdout.contains("analytics HTTP 400"));
    assert_eq!(substantive.lines().count(), 1);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&substantive).unwrap()["item"]["exit_code"], 9);
    let mut lines = stdout.lines();
    assert_eq!(PathBuf::from(lines.next().unwrap()), request);
    assert_eq!(PathBuf::from(lines.next().unwrap()), result);
    let args = lines.next().unwrap().replace("\\\"", "\"").replace('"', "");
    for fragment in ["exec --model worker-model", "-c model_reasoning_effort=low", "--sandbox workspace-write", "--json", "-c approval_policy=never"] {
        assert!(args.contains(fragment), "{args}");
    }
    for (flag, path) in [("--add-dir", &directory), ("--output-schema", &schema), ("--output-last-message", &result)] {
        assert!(args.contains(&format!("{flag} {}", path.display())), "{args}");
    }
}
