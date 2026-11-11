use relay_coordinator::{engine, model::Snapshot, runner::Runner, store::Store};
use serde_json::{Value, json};
use std::{io::Write, sync::Arc, time::Duration};

// The integration test executable doubles as a real CLI worker. No Python,
// shell, model provider, or installed application is needed by these tests.
#[test]
fn output_fixture() {
    let Ok(input_path) = std::env::var("RELAY_INPUT") else {
        return;
    };
    let input: Value = serde_json::from_reader(std::fs::File::open(input_path).unwrap()).unwrap();
    let mode = input["task"]["description"].as_str().unwrap();
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    writeln!(stdout).unwrap();
    if mode == "large_logs" || mode == "large_file" {
        // A single long line exercises the disk parser without allocating that
        // diagnostic line. Both streams exceed the former failure threshold.
        for _ in 0..300 {
            stdout.write_all(&[b'x'; 8192]).unwrap();
            stderr.write_all(&[b'e'; 8192]).unwrap();
        }
        writeln!(stdout).unwrap();
        writeln!(stderr, "FINAL DIAGNOSTIC").unwrap();
    }
    stdout.flush().unwrap();
    stderr.flush().unwrap();
    std::thread::sleep(Duration::from_millis(250));
    let summary = if mode == "large_file" || mode == "large_stdout_result" {
        "s".repeat(2_100_000)
    } else {
        "Finished after verbose output".to_owned()
    };
    let result = json!({"outcome":"complete", "summary":summary,
        "artifacts":[{"name":"evidence.txt", "content":"Worker process finished"}]});
    if mode == "large_file" {
        serde_json::to_writer(
            std::fs::File::create(std::env::var("RELAY_OUTPUT").unwrap()).unwrap(),
            &result,
        )
        .unwrap();
    } else if mode == "pretty_stdout" {
        writeln!(
            stdout,
            "```json\n{}\n```",
            serde_json::to_string_pretty(&result).unwrap()
        )
        .unwrap();
    } else {
        writeln!(stdout, "{result}").unwrap();
    }
}

async fn run_fixture(mode: &str, agent_timeout: u64) -> (tempfile::TempDir, Snapshot) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path().join("relay.sqlite")).unwrap());
    let (attempt, agent) = store
        .mutate(|s| {
            let agent = engine::save_agent(
                s,
                None,
                json!({
                    "name":"Output fixture", "adapter":"cli",
                    "command":std::env::current_exe().unwrap(),
                    "args":["--exact", "output_fixture", "--nocapture"],
                    "stages":["planning"], "timeout_seconds":agent_timeout,
                }),
            )?;
            let task = engine::create_task(
                s,
                json!({"title":"Output regression", "description":mode,
            "acceptance":"The process completes and returns its result", "timeout_seconds":0}),
            )?;
            engine::action(
                s,
                &task.id,
                json!({"action":"start", "revision":task.revision}),
            )?;
            let (attempt, _) = engine::claim(s, Some(&task.id))?.unwrap();
            Ok((attempt, agent))
        })
        .unwrap();
    let runner = Runner::new(store.clone(), dir.path().to_owned());
    runner.launch(attempt, agent);
    let state = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let state = store.read().unwrap();
            if state.attempts[0].status != "running" {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("Fixture execution did not finish");
    assert_eq!(
        state.attempts[0].status, "completed",
        "{}",
        state.attempts[0].error
    );
    assert_eq!(state.tasks[0].stage, "implementation");
    (dir, state)
}

#[tokio::test]
async fn verbose_stdout_and_stderr_are_preserved_without_failing_work() {
    let (dir, state) = run_fixture("large_logs", 0).await;
    let run = dir.path().join("runs").join(&state.attempts[0].id);
    for name in ["stdout.log", "stderr.log"] {
        assert!(std::fs::metadata(run.join(name)).unwrap().len() > 2_000_000);
    }
    assert!(state.attempts[0].log.contains("FINAL DIAGNOSTIC"));
    assert!(state.attempts[0].log.len() < 70_000);
    assert!(state.attempts[0].log.contains("complete log:"));
    assert!(state.attempts[0].duration_ms.unwrap() >= 200);
}

#[tokio::test]
async fn result_file_larger_than_two_megabytes_is_accepted() {
    let (dir, state) = run_fixture("large_file", 0).await;
    let result = dir
        .path()
        .join("runs")
        .join(&state.attempts[0].id)
        .join("agent-result.json");
    assert!(std::fs::metadata(result).unwrap().len() > 2_000_000);
    assert_eq!(state.attempts[0].summary.len(), 2_100_000);
}

#[tokio::test]
async fn large_json_stdout_result_is_accepted() {
    let (_, state) = run_fixture("large_stdout_result", 0).await;
    assert_eq!(state.attempts[0].summary.len(), 2_100_000);
}

#[tokio::test]
async fn pretty_fenced_result_and_one_disabled_timeout_work() {
    let (_, state) = run_fixture("pretty_stdout", 5).await;
    assert!(state.attempts[0].duration_ms.unwrap() >= 200);
}

#[tokio::test]
async fn unrepresentable_timeout_deadline_does_not_crash_the_executor() {
    let (_, state) = run_fixture("pretty_stdout", u64::MAX).await;
    assert!(state.attempts[0].duration_ms.unwrap() >= 200);
}
