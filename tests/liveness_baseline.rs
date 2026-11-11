use relay_coordinator::{engine, external, model::*, runner::Runner, store::Store};
use rusqlite::Connection;
use serde_json::json;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

fn task(state: &mut Snapshot, title: &str, dependencies: &[String]) -> Task {
    engine::create_task(
        state,
        json!({"title":title,"acceptance":"Fixture checked","dependencies":dependencies,"tags":["demo"]}),
    )
    .unwrap()
}

fn worker_claim(
    state: &mut Snapshot,
    worker: &str,
    stage: &str,
    request: &str,
    task_id: Option<&str>,
) -> Option<Attempt> {
    external::claim(
        state,
        serde_json::from_value(json!({
            "source":"liveness-test","agent_id":worker,"name":worker,"stages":[stage],
            "request_id":request,"task_id":task_id
        }))
        .unwrap(),
    )
    .unwrap()
}

fn result(outcome: &str, reason: &str) -> AgentResult {
    serde_json::from_value(json!({
        "outcome":outcome,"summary":"Fixture result","reason":reason,
        "artifacts":[{"name":"evidence","content":"Checked fixture evidence"}]
    }))
    .unwrap()
}

#[test]
fn authorized_task_pause_survives_prerequisite_completion_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay.sqlite");
    let store = Store::open(&path).unwrap();
    let (prerequisite_id, paused_id) = store
        .mutate(|state| {
            let prerequisite = task(state, "Prerequisite", &[]);
            let paused = task(
                state,
                "Held follow-on",
                std::slice::from_ref(&prerequisite.id),
            );
            engine::action(
                state,
                &paused.id,
                json!({"action":"pause","reason":"Wait for prerequisite","revision":paused.revision}),
            )?;
            Ok((prerequisite.id, paused.id))
        })
        .unwrap();
    store
        .mutate(|state| {
            for (worker, stage) in [
                ("builder", "planning"),
                ("builder", "implementation"),
                ("reviewer", "review"),
            ] {
                let run =
                    worker_claim(state, worker, stage, stage, Some(&prerequisite_id)).unwrap();
                engine::finish(state, &run.id, result("complete", ""), String::new())?;
            }
            assert_eq!(state.tasks[0].status, "completed");
            let sibling = task(state, "Unrelated work", &[]);
            let run = worker_claim(state, "builder", "planning", "sibling", None).unwrap();
            assert_eq!(run.task_id, sibling.id);
            Ok(())
        })
        .unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    store
        .mutate(|state| {
            for _ in 0..3 {
                engine::reconcile(state)?;
            }
            let paused = state.tasks.iter().find(|t| t.id == paused_id).unwrap();
            assert_eq!(paused.status, "paused");
            assert!(engine::dependency_reason(state, paused).is_none());
            let revision = paused.revision;
            assert!(state.events.iter().any(|event| {
                event.task_id.as_deref() == Some(&paused_id) && event.kind == "pause"
            }));
            assert!(worker_claim(state, "new-builder", "planning", "held", None).is_none());
            assert_eq!(engine::claim(state, Some(&paused_id)).unwrap_err().0, 409);
            engine::action(
                state,
                &paused_id,
                json!({"action":"resume","reason":"Operator releases hold","revision":revision}),
            )?;
            assert!(
                worker_claim(
                    state,
                    "new-builder",
                    "planning",
                    "released",
                    Some(&paused_id)
                )
                .is_some()
            );
            Ok(())
        })
        .unwrap();
}

async fn wait_until(mut observed: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !observed() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("scheduler did not reach the expected checkpoint");
}

#[tokio::test]
async fn invalid_persisted_stage_recovers_without_blocking_unrelated_progress() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path().join("relay.sqlite")).unwrap());
    let (sibling_id, broken_id) = store
        .mutate(|state| {
            let broken = task(state, "Task with inconsistent persisted stage", &[]);
            let run = worker_claim(
                state,
                "fixture-worker",
                "planning",
                "block",
                Some(&broken.id),
            )
            .unwrap();
            engine::finish(
                state,
                &run.id,
                result("blocked", "Fixture capability denied"),
                String::new(),
            )?;
            state.attempts.last_mut().unwrap().finished_at = Some("2000-01-01T00:00:00Z".into());
            let broken_id = broken.id.clone();
            let broken = state.tasks.iter_mut().find(|t| t.id == broken_id).unwrap();
            broken.stage = "unrecognized-stage".into();
            let sibling = task(state, "Independent runnable task", &[]);
            // A fault discovered after promoting another task must not roll it back.
            state.tasks.swap(0, 1);
            Ok((sibling.id, broken_id))
        })
        .unwrap();
    configure_local_planner(&store);
    let before = store.read().unwrap();
    let runner = Runner::new(store.clone(), dir.path().to_owned());
    runner.start_scheduler();
    wait_until(|| {
        store
            .read()
            .unwrap()
            .tasks
            .iter()
            .any(|task| task.id == sibling_id && task.stage == "implementation")
    })
    .await;
    runner.stopping.store(true, Ordering::Relaxed);
    let after = store.read().unwrap();
    assert!(after.revision > before.revision);
    assert!(runner.health.lock().unwrap().1.is_none());
    let broken = after.tasks.iter().find(|t| t.id == broken_id).unwrap();
    assert_eq!(broken.stage, "planning");
    assert!(after.events.iter().any(
        |event| event.task_id.as_deref() == Some(&broken_id) && event.kind == "stage_recovered"
    ));
    assert!(after.tasks.iter().any(|t| {
        t.recovery_of
            .as_ref()
            .is_some_and(|incident| incident.task_id == broken_id)
    }));
    assert!(before.attempts.iter().all(|previous| {
        after
            .attempts
            .iter()
            .any(|attempt| attempt.id == previous.id)
    }));
}

fn configure_local_planner(store: &Store) {
    store.mutate(|state| {
        engine::save_agent(state, None, json!({
            "name":"Fixture local planner","adapter":"cli","command":env!("CARGO_BIN_EXE_relay-coordinator"),
            "args":["example-agent"],"stages":["planning"]
        }))?;
        Ok(())
    }).unwrap();
}

#[tokio::test]
async fn store_write_failure_retries_without_a_partial_claim() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay.sqlite");
    let store = Arc::new(Store::open(&path).unwrap());
    configure_local_planner(&store);
    store
        .mutate(|state| {
            task(state, "Retry after store recovers", &[]);
            Ok(())
        })
        .unwrap();
    let before = store.read().unwrap();
    let fault = Connection::open(&path).unwrap();
    fault
        .execute_batch(
            "CREATE TRIGGER fail_store_write BEFORE UPDATE ON meta
         BEGIN SELECT RAISE(ABORT, 'fixture store unavailable'); END;",
        )
        .unwrap();
    let runner = Runner::new(store.clone(), dir.path().to_owned());
    runner.start_scheduler();
    wait_until(|| {
        runner
            .health
            .lock()
            .unwrap()
            .1
            .as_deref()
            .is_some_and(|error| error.contains("fixture store unavailable"))
    })
    .await;
    let after = store.read().unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.events.len(), before.events.len());
    assert_eq!(after.tasks[0].stage, "backlog");
    assert!(after.tasks[0].current_attempt.is_none());
    assert!(after.attempts.is_empty());
    fault
        .execute_batch("DROP TRIGGER fail_store_write")
        .unwrap();
    wait_until(|| store.read().unwrap().tasks[0].stage == "implementation").await;
    runner.stopping.store(true, Ordering::Relaxed);
    let recovered = store.read().unwrap();
    assert_eq!(recovered.attempts.len(), 1);
    assert_eq!(recovered.attempts[0].number, 1);
    assert_eq!(
        recovered
            .events
            .iter()
            .filter(|e| e.kind == "attempt_started")
            .count(),
        1
    );
}
