use relay_coordinator::{callbacks, engine, runner::Runner, store::Store};
use serde_json::{Value, json};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

// This worker performs an actual repair in an isolated directory. Its planning
// role never writes the prerequisite and cannot satisfy an execution gate.
#[test]
fn repair_worker_process() {
    let Ok(path) = std::env::var("RELAY_INPUT") else {
        return;
    };
    let input: Value = serde_json::from_reader(std::fs::File::open(path).unwrap()).unwrap();
    let stage = input["stage"].as_str().unwrap();
    let recovery = !input["recovery_context"].is_null();
    let prerequisite = std::path::Path::new("prerequisite.txt");
    let mut outcome = "complete";
    let detail;
    if stage == "planning" {
        detail = "The prerequisite file is missing. Implementation should create it, then read it back. This plan does not execute or verify the repair.".to_string();
    } else if recovery && stage == "implementation" {
        let counter = std::path::Path::new("repair-round.txt");
        let round = std::fs::read_to_string(counter)
            .unwrap_or_else(|_| "0".into())
            .parse::<u32>()
            .unwrap()
            + 1;
        std::fs::write(counter, round.to_string()).unwrap();
        if round <= 4 {
            outcome = "blocked";
            detail = format!(
                "Repair attempt {round} could not complete the operation; another recovery action is needed"
            );
        } else {
            std::fs::write(prerequisite, "working").unwrap();
            assert_eq!(std::fs::read_to_string(prerequisite).unwrap(), "working");
            detail = "Created prerequisite.txt and read back the expected contents".into();
        }
    } else {
        match std::fs::read_to_string(prerequisite) {
            Ok(value) if value == "working" => {
                detail = "Executed the original read operation successfully".into()
            }
            other => {
                outcome = "blocked";
                detail = format!("Original read operation failed: {other:?}");
            }
        }
    }
    let result = json!({"outcome":outcome,"summary":detail,"reason":if outcome=="complete"{""}else{&detail},
        "artifacts":[{"name":"operation.txt","content":detail}]});
    std::fs::write(std::env::var("RELAY_OUTPUT").unwrap(), result.to_string()).unwrap();
}

#[tokio::test]
async fn background_scheduler_keeps_repairing_then_resumes_real_operation_without_intervention() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("source");
    std::fs::create_dir(&workspace).unwrap();
    let store = Arc::new(Store::open(directory.path().join("relay.sqlite")).unwrap());
    store.mutate(|s| {
        s.settings.retry_delay_seconds=0;
        for (name,stage) in [("Read-only planner","planning"),("Repair implementer","implementation"),("Independent reviewer","review")] {
            engine::save_agent(s,None,json!({"name":name,"adapter":"cli","command":std::env::current_exe().unwrap(),
                "args":["--exact","repair_worker_process","--nocapture"],"stages":[stage]}))?;
        }
        callbacks::register(s,serde_json::from_value(json!({"source":"runtime-test","agent_id":"origin","name":"Originating planner","workspace_default":true})).unwrap())?;
        engine::create_task(s,json!({"title":"Read the actual prerequisite","acceptance":"The file can be read and its contents match", "workspace":workspace}))?;
        Ok(())
    }).unwrap();
    let runner = Runner::new(store.clone(), directory.path().to_owned());
    runner.start_scheduler();
    let completed = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let s = store.read().unwrap();
            if s.tasks[0].status == "completed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    runner.stopping.store(true, Ordering::Relaxed);
    let s = store.read().unwrap();
    assert!(
        completed.is_ok(),
        "No progress: {:?}",
        s.tasks
            .iter()
            .map(|t| (&t.stage, &t.status, &t.blocked_reason))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("prerequisite.txt")).unwrap(),
        "working"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("repair-round.txt")).unwrap(),
        "5"
    );
    assert_eq!(s.tasks.len(), 2);
    assert!(s.tasks.iter().all(|t| t.status == "completed"));
    assert_eq!(
        s.events
            .iter()
            .filter(|e| e.kind == "recovery_continued")
            .count(),
        4
    );
    assert!(s.planner_notifications.len() >= 5);
    assert!(
        s.planner_notifications
            .iter()
            .all(|n| n.acknowledged_at.is_none()),
        "Recovery must not wait for callback receipt"
    );
    let repair = &s.tasks[1];
    let implementer = s
        .attempts
        .iter()
        .find(|a| a.task_id == repair.id && a.stage == "implementation" && a.status == "completed")
        .unwrap();
    let reviewer = s
        .attempts
        .iter()
        .find(|a| a.task_id == repair.id && a.stage == "review" && a.status == "completed")
        .unwrap();
    assert_ne!(implementer.agent_id, reviewer.agent_id);
}

#[tokio::test]
async fn completed_worker_result_survives_temporary_database_write_failure() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("relay.sqlite");
    let store = Arc::new(Store::open(&path).unwrap());
    let (attempt, agent) = store.mutate(|s| {
        engine::save_agent(s,None,json!({"name":"Worker","adapter":"cli","command":env!("CARGO_BIN_EXE_relay-coordinator"),"args":["example-agent"],"stages":["planning"]}))?;
        engine::create_task(s,json!({"title":"Retain outcome","acceptance":"Outcome saved","tags":["demo"]}))?;
        engine::reconcile(s)?;
        Ok(engine::claim(s,None)?.unwrap())
    }).unwrap();
    let fault = rusqlite::Connection::open(&path).unwrap();
    fault.execute_batch("CREATE TRIGGER fail_outcome BEFORE UPDATE ON meta BEGIN SELECT RAISE(ABORT, 'temporary write outage'); END;").unwrap();
    let runner = Runner::new(store.clone(), directory.path().to_owned());
    let result_path = directory
        .path()
        .join("runs")
        .join(&attempt.id)
        .join("result.json");
    runner.launch(attempt, agent);
    tokio::time::timeout(Duration::from_secs(10), async {
        while !result_path.exists() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(store.read().unwrap().attempts[0].status, "running");
    fault.execute_batch("DROP TRIGGER fail_outcome").unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while store.read().unwrap().attempts[0].status == "running" {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    let state = store.read().unwrap();
    assert_eq!(state.attempts.len(), 1);
    assert_eq!(state.attempts[0].status, "completed");
    assert_eq!(state.tasks[0].stage, "implementation");
    runner.stopping.store(true, Ordering::Relaxed);
}
