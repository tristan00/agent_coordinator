use relay_coordinator::{callbacks, engine, external, model::*};
use serde_json::json;

fn setup() -> Snapshot {
    let mut s = Snapshot::default();
    s.settings.retry_delay_seconds = 0;
    engine::create_task(
        &mut s,
        json!({"title":"Recover the real work","acceptance":"Deliver working result"}),
    )
    .unwrap();
    callbacks::register(&mut s,serde_json::from_value(json!({"source":"any-runtime","agent_id":"originating-planner","name":"Planner","workspace_default":true})).unwrap()).unwrap();
    s
}
fn claim(s: &mut Snapshot, request: &str) -> Option<Attempt> {
    external::claim(s,serde_json::from_value(json!({"source":"recovery-test","agent_id":"builder","name":"Builder","stages":["planning","implementation"],"request_id":request})).unwrap()).unwrap()
}
fn result(outcome: &str) -> AgentResult {
    serde_json::from_value(json!({"outcome":outcome,"summary":"Stage result","reason":if outcome=="complete"{""}else{"Operation needs an executable repair, not another read-only retry"},"artifacts":[{"name":"handoff","content":"An actionable plan; implementation still needs to execute it."}]})).unwrap()
}
fn block_implementation(s: &mut Snapshot, label: &str) {
    let plan = claim(s, &format!("{label}-plan")).unwrap();
    engine::finish(s, &plan.id, result("complete"), String::new()).unwrap();
    let run = claim(s, &format!("{label}-build")).unwrap();
    assert_eq!(run.stage, "implementation");
    engine::finish(s, &run.id, result("blocked"), String::new()).unwrap();
    engine::reconcile(s).unwrap();
}
fn finish_repair(s: &mut Snapshot, label: &str) {
    for stage in ["planning", "implementation"] {
        let a = claim(s, &format!("{label}-{stage}")).unwrap();
        assert_eq!(a.task_id, s.tasks[1].id);
        assert_eq!(a.stage, stage);
        engine::finish(s, &a.id, result("complete"), String::new()).unwrap();
    }
    let review=external::claim(s,serde_json::from_value(json!({"source":"recovery-test","agent_id":"reviewer","name":"Reviewer","stages":["review"],"request_id":format!("{label}-review")})).unwrap()).unwrap().unwrap();
    engine::finish(s, &review.id, result("complete"), String::new()).unwrap();
    engine::reconcile(s).unwrap();
}

#[test]
fn first_blocker_queues_callback_and_dedicated_repair_in_parallel() {
    let mut s = setup();
    block_implementation(&mut s, "first");
    assert_eq!(s.tasks.len(), 2);
    assert_eq!(s.tasks[0].stage, "implementation");
    assert_eq!(s.tasks[0].sendbacks, 0);
    assert_eq!(s.tasks[0].status, "blocked");
    assert_eq!(s.tasks[1].stage, "planning");
    assert_eq!(s.tasks[1].status, "ready");
    assert_eq!(s.tasks[1].max_attempts, 0);
    assert_eq!(s.tasks[1].max_sendbacks, 0);
    assert_eq!(s.tasks[1].timeout_seconds, 0);
    assert_eq!(s.planner_notifications.len(), 1);
    assert!(s.planner_notifications[0].acknowledged_at.is_none());
    let repair = claim(&mut s, "repair").unwrap();
    assert!(
        repair.input["recovery_context"]["instructions"]
            .as_str()
            .unwrap()
            .contains("Planning may complete")
    );
    assert!(
        repair.input["recovery_context"]["blocked_attempt"]["output"]["reason"]
            .as_str()
            .unwrap()
            .contains("executable repair")
    );
}

#[test]
fn planning_can_finish_with_a_plan_before_repair_is_executed() {
    let mut s = setup();
    block_implementation(&mut s, "first");
    let plan = claim(&mut s, "repair-plan").unwrap();
    engine::finish(&mut s, &plan.id, result("complete"), String::new()).unwrap();
    assert_eq!(s.tasks[1].stage, "implementation");
    let implementation = claim(&mut s, "repair-implementation").unwrap();
    assert_eq!(implementation.task_id, s.tasks[1].id);
    assert_eq!(s.tasks[0].status, "blocked");
}

#[test]
fn repair_that_cannot_act_keeps_routing_without_attempt_or_sendback_exhaustion() {
    let mut s = setup();
    block_implementation(&mut s, "first");
    let repair_id = s.tasks[1].id.clone();
    for n in 0..48 {
        let run = claim(&mut s, &format!("repair-{n}")).unwrap();
        assert_eq!(run.task_id, repair_id);
        assert_eq!(
            run.stage,
            if n % 2 == 0 {
                "planning"
            } else {
                "implementation"
            }
        );
        if n % 3 == 0 {
            engine::fail(&mut s, &run.id, "Provider transient failure", "").unwrap();
        } else {
            engine::finish(&mut s, &run.id, result("blocked"), String::new()).unwrap();
        }
        engine::reconcile(&mut s).unwrap();
        assert_eq!(s.tasks[1].status, "ready");
        assert_eq!(s.tasks.len(), 2);
        assert!(s.tasks.iter().all(|t| t.status != "completed"));
    }
    assert_eq!(
        s.events
            .iter()
            .filter(|e| e.kind == "recovery_continued")
            .count(),
        48
    );
    assert_eq!(s.planner_notifications.len(), 49);
    assert!(claim(&mut s, "still-working").is_some());
}

#[test]
fn every_recurrence_reuses_recovery_and_releases_original_again() {
    let mut s = setup();
    block_implementation(&mut s, "first");
    let original_id = s.tasks[0].id.clone();
    let repair_id = s.tasks[1].id.clone();
    for n in 0..4 {
        finish_repair(&mut s, &format!("cycle-{n}"));
        assert_eq!(s.tasks[0].status, "ready");
        assert_eq!(s.tasks[0].stage, "implementation");
        assert!(s.tasks[1].recovery_of.as_ref().unwrap().resumed);
        let run = claim(&mut s, &format!("original-{n}")).unwrap();
        assert_eq!(run.task_id, original_id);
        engine::finish(&mut s, &run.id, result("needs_input"), String::new()).unwrap();
        engine::reconcile(&mut s).unwrap();
        assert_eq!(s.tasks[1].id, repair_id);
        assert_eq!(s.tasks[1].status, "ready");
        assert_eq!(s.tasks[1].stage, "planning");
        assert!(!s.tasks[1].recovery_of.as_ref().unwrap().resumed);
        assert_eq!(s.tasks.len(), 2);
    }
}

#[test]
fn explicit_pause_cancel_archive_and_approval_still_stop_execution() {
    for mode in [
        "paused",
        "automatic_off",
        "archived",
        "human_approval",
        "cancelled",
        "task_paused",
    ] {
        let mut s = setup();
        let run = claim(&mut s, "first").unwrap();
        engine::finish(&mut s, &run.id, result("blocked"), String::new()).unwrap();
        match mode {
            "paused" => s.settings.paused = true,
            "automatic_off" => s.settings.auto_advance = false,
            "archived" => s.tasks[0].archived = true,
            "human_approval" => s.tasks[0].human_approval = true,
            "cancelled" => s.tasks[0].status = "cancelled".into(),
            _ => s.tasks[0].status = "paused".into(),
        }
        let task = serde_json::to_value(&s.tasks[0]).unwrap();
        engine::reconcile(&mut s).unwrap();
        assert_eq!(serde_json::to_value(&s.tasks[0]).unwrap(), task, "{mode}");
        assert_eq!(s.tasks.len(), 1);
    }
}

#[test]
fn fixed_retry_pacing_does_not_delay_first_callback_or_incident_creation() {
    let mut s = setup();
    s.settings.retry_delay_seconds = 300;
    block_implementation(&mut s, "first");
    assert_eq!(s.tasks.len(), 2);
    assert_eq!(s.planner_notifications.len(), 1);
    let run = claim(&mut s, "repair").unwrap();
    engine::finish(&mut s, &run.id, result("blocked"), String::new()).unwrap();
    engine::reconcile(&mut s).unwrap();
    assert_eq!(s.tasks[1].status, "blocked");
    s.attempts.last_mut().unwrap().finished_at = Some("2000-01-01T00:00:00Z".into());
    engine::reconcile(&mut s).unwrap();
    assert_eq!(s.tasks[1].status, "ready");
    assert!(claim(&mut s, "next-step").is_some());
}

#[test]
fn worker_failure_enters_investigation_immediately() {
    let mut s = setup();
    let run = claim(&mut s, "first").unwrap();
    engine::fail(&mut s, &run.id, "Cannot launch configured executable", "").unwrap();
    engine::reconcile(&mut s).unwrap();
    assert_eq!(s.tasks.len(), 2);
    assert_eq!(s.tasks[1].status, "ready");
    assert_eq!(s.planner_notifications.len(), 1);
}

#[test]
fn no_planning_agent_routes_investigation_to_implementation() {
    let mut s = setup();
    let run = claim(&mut s, "first").unwrap();
    s.agents[0].stages = vec!["implementation".into()];
    engine::finish(&mut s, &run.id, result("blocked"), String::new()).unwrap();
    engine::reconcile(&mut s).unwrap();
    assert_eq!(s.tasks[1].stage, "implementation");
}
