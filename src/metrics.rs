use crate::engine::Task;
use crate::history::Event;
use serde_json::{Value, json};
use std::collections::BTreeMap;
pub fn lifecycle(task: &Task) -> Value {
    let created = task.history.iter().find(|event| event.kind == "created");
    let start = created.map(|event| event.at);
    let end = task
        .history
        .iter()
        .rev()
        .find(|event| event.kind == "accepted" && event.revision == task.revision && event.evidence["currentRevision"] == true && event.evidence["ended"] == event.at)
        .filter(|_| task.stage == "done")
        .map(|event| event.at);
    let execution = ["implementation", "review"].map(|stage| {
        let attempts = task.attempts.iter().filter(|attempt| attempt.stage == stage);
        let count = attempts.clone().count();
        let mut durations = attempts.filter_map(|attempt| attempt.ended?.checked_sub(attempt.evidence.launch.as_ref()?.started));
        let measured = durations.clone().count();
        json!({"stage":stage,"attempts":count,"measuredAttempts":measured,"unmeasuredAttempts":count-measured,
            "recordedWorkerMs":(measured != 0).then(|| durations.try_fold(0_u64, u64::checked_add)).flatten()})
    });
    json!({"createdAt":start,"acceptedAt":end,"leadMs":start.zip(end).and_then(|(start,end)| end.checked_sub(start)),
        "rightCensored":start.is_some() && task.stage != "done" && !task.history.iter().any(|event| event.kind == "abandoned"),"execution":execution,
        "intervals":created.map(|event| intervals(event, task, end).0)})
}
pub fn intervals(created: &Event, task: &Task, through: Option<u64>) -> (Value, (&'static str, u64, bool, bool)) {
    let mut totals = BTreeMap::from([("queue", 0_u64), ("dependency", 0), ("resource", 0), ("retry", 0), ("blocked", 0), ("stopped", 0)]);
    let (mut stopped, mut blocked, mut active) = (false, false, false);
    let (mut stage, mut revision) = ("implementation", created.revision);
    let mut paused = created.evidence["boardPaused"].as_bool().unwrap_or(false);
    let mut waiting = "queue";
    let mut since = created.at;
    for event in task.history.iter().skip_while(|event| !std::ptr::eq(*event, created)).skip(1) {
        let at = event.at.min(through.unwrap_or(u64::MAX)).max(since);
        let waits = [(stopped || paused, "stopped"), (blocked, "blocked"), (!active, waiting)];
        if stage != "done"
            && let Some((_, bucket)) = waits.into_iter().find(|(applies, _)| *applies)
        {
            *totals.get_mut(bucket).unwrap() += at - since;
        }
        since = at;
        match event.kind.as_str() {
            "waiting_changed" => waiting = wait_bucket(event.evidence["reason"].as_str().unwrap_or_default()),
            "stopped" | "resumed" if event.evidence["scope"] == "board" => paused = event.kind == "stopped",
            "stopped" => stopped = true,
            "resumed" => (stopped, blocked) = (false, false),
            "dispatched" => (active, waiting) = (true, "queue"),
            "blocked" if event.evidence["currentRevision"] == true => (active, blocked) = (false, true),
            "completed" | "accepted" | "rejected" if event.evidence["currentRevision"] == true => {
                stage = crate::engine::next_stage(&event.kind).unwrap();
                (active, blocked) = (false, false);
            }
            "reopened" | "revised" => (stage, revision, blocked) = ("implementation", event.revision, false),
            "evidence_blocked" => blocked = true,
            "evidence_restored" => blocked = false,
            "accepted" | "completed" | "rejected" | "blocked" | "attempt_stopped" | "attempt_failed" => active = false,
            _ => {}
        }
    }
    (json!({"through":since,"initialBoardPauseKnown":created.evidence["boardPaused"].is_boolean(),"milliseconds":totals}), (stage, revision, stopped, blocked))
}
fn wait_bucket(reason: &str) -> &'static str {
    [("Waiting for ", "dependency"), ("Retry scheduled at ", "retry")].into_iter().find_map(|(prefix, bucket)| reason.starts_with(prefix).then_some(bucket)).unwrap_or("queue")
}

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod tests;
