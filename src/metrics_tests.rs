use super::*;
use crate::engine::{Attempt, Task};
use crate::history::Launch;

fn task(stage: &str) -> Task {
    serde_json::from_value(json!({"id":"t","title":"T","outcome":"O","cwd":"C:\\","acceptance":["c"],"dependencies":[],
        "stage":stage,"stopped":false,"attempts":[],"active":null,"feedback":[],"failure":"","retryAt":0})).unwrap()
}
fn push(task: &mut Task, kind: &str, at: u64, evidence: Value) {
    let mut event = Event::new(task, kind, "test", evidence);
    event.at = at;
    task.history.push(event);
}
fn totals(task: &Task) -> Value {
    lifecycle(task)["intervals"]["milliseconds"].clone()
}

#[test]
fn wait_reasons_map_to_interval_buckets() {
    assert_eq!(wait_bucket("Waiting for result from Alpha"), "dependency");
    assert_eq!(wait_bucket("Waiting for Alpha to settle"), "dependency");
    assert_eq!(wait_bucket("Retry scheduled at 5"), "retry");
    assert_eq!(wait_bucket("Usage limit; dispatch resumes at 5"), "queue");
    assert_eq!(wait_bucket(""), "queue");
}

#[test]
fn lifecycle_attributes_wait_time_and_lead_time() {
    let mut task = task("done");
    push(&mut task, "created", 1_000, json!({"boardPaused":false}));
    push(&mut task, "waiting_changed", 2_000, json!({"reason":"Waiting for result from X"}));
    push(&mut task, "waiting_changed", 5_000, json!({"reason":null}));
    push(&mut task, "dispatched", 6_000, json!({}));
    push(&mut task, "completed", 8_000, json!({"currentRevision":true}));
    push(&mut task, "stopped", 9_000, json!({"scope":"task"}));
    push(&mut task, "resumed", 12_000, json!({"scope":"task"}));
    push(&mut task, "dispatched", 13_000, json!({}));
    push(&mut task, "accepted", 15_000, json!({"currentRevision":true,"ended":15_000}));
    let launch = Some(Launch { started: 6_100, worker: 1, supervisor: 2 });
    let attempt = |stage: &str, ended: u64, launch: Option<Launch>| Attempt {
        id: stage.into(), revision: 0, stage: stage.into(), started: 0, ended: Some(ended), result: None, error: None,
        evidence: crate::history::AttemptEvidence { launch, ..Default::default() },
    };
    task.attempts = vec![attempt("implementation", 8_000, launch), attempt("review", 15_000, None)];
    let value = lifecycle(&task);
    assert_eq!((value["createdAt"].as_u64(), value["acceptedAt"].as_u64(), value["leadMs"].as_u64()), (Some(1_000), Some(15_000), Some(14_000)));
    assert_eq!(value["rightCensored"], false);
    assert_eq!(value["intervals"]["through"], 15_000);
    assert_eq!(value["intervals"]["initialBoardPauseKnown"], true);
    assert_eq!(totals(&task), json!({"queue":4_000,"dependency":3_000,"resource":0,"retry":0,"blocked":0,"stopped":3_000}));
    assert_eq!(value["execution"][0], json!({"stage":"implementation","attempts":1,"measuredAttempts":1,"unmeasuredAttempts":0,"recordedWorkerMs":1_900}));
    assert_eq!(value["execution"][1]["unmeasuredAttempts"], 1);
    assert!(value["execution"][1]["recordedWorkerMs"].is_null());
}

#[test]
fn lifecycle_counts_board_pauses_blocks_and_censoring() {
    let mut paused = task("implementation");
    push(&mut paused, "created", 0, json!({"boardPaused":true}));
    push(&mut paused, "resumed", 500, json!({"scope":"board"}));
    push(&mut paused, "dispatched", 700, json!({}));
    push(&mut paused, "blocked", 1_000, json!({"currentRevision":true}));
    push(&mut paused, "resumed", 1_600, json!({"scope":"task"}));
    let value = lifecycle(&paused);
    assert_eq!(totals(&paused), json!({"queue":200,"dependency":0,"resource":0,"retry":0,"blocked":600,"stopped":500}));
    assert_eq!((value["acceptedAt"].is_null(), value["leadMs"].is_null(), value["rightCensored"].as_bool()), (true, true, Some(true)));
    push(&mut paused, "abandoned", 2_000, Value::Null);
    assert_eq!(lifecycle(&paused)["rightCensored"], false);
    let mut stale = task("done");
    push(&mut stale, "created", 0, json!({}));
    push(&mut stale, "accepted", 10, json!({"currentRevision":true,"ended":99}));
    assert!(lifecycle(&stale)["acceptedAt"].is_null());
    assert_eq!(lifecycle(&stale)["intervals"]["initialBoardPauseKnown"], false);
    assert!(lifecycle(&task("implementation"))["intervals"].is_null());
}
