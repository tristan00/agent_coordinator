use relay_coordinator::{engine::*, external, model::*, runner::Runner, store::Store};
use serde_json::json;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
fn task(s: &mut Snapshot, title: &str) -> Task {
    create_task(
        s,
        json!({"title":title,"acceptance":"Result checked","max_attempts":2}),
    )
    .unwrap()
}
fn claim_worker(s: &mut Snapshot, agent: &str, request: &str) -> Option<Attempt> {
    external::claim(s,serde_json::from_value(json!({"source":"test-provider","agent_id":agent,"name":agent,"stages":["planning","implementation","review"],"request_id":request})).unwrap()).unwrap()
}
fn result() -> AgentResult {
    serde_json::from_value(json!({"outcome":"complete","summary":"Checked","artifacts":[{"name":"evidence","content":"Observed test evidence"}]})).unwrap()
}
#[test]
fn external_workers_advance_pipeline_and_dependent_tasks_without_manual_actions() {
    let mut s = Snapshot::default();
    s.settings.paused = false;
    let a = task(&mut s, "First");
    let b = create_task(
        &mut s,
        json!({"title":"Second","acceptance":"Checked","dependencies":[a.id]}),
    )
    .unwrap();
    for (i, stage) in ["planning", "implementation", "review"].iter().enumerate() {
        let run = claim_worker(
            &mut s,
            if i == 2 { "reviewer" } else { "builder" },
            &i.to_string(),
        )
        .unwrap();
        assert_eq!(run.task_id, a.id);
        assert_eq!(&run.stage, stage);
        finish(&mut s, &run.id, result(), String::new()).unwrap();
    }
    let run = claim_worker(&mut s, "builder", "next-task").unwrap();
    assert_eq!(run.task_id, b.id);
    assert_eq!(run.stage, "planning");
    assert_eq!(s.attempts.len(), 4);
    assert_eq!(s.agents.len(), 2);
}
#[test]
fn first_failure_starts_recovery_and_repeated_failures_keep_it_actionable() {
    let mut s = Snapshot::default();
    s.settings.retry_delay_seconds = 0;
    let original = task(&mut s, "Continuous recovery");
    let first = claim_worker(&mut s, "any-provider", "original").unwrap();
    fail(&mut s, &first.id, "Original operation failed", "").unwrap();
    reconcile(&mut s).unwrap();
    assert_eq!(s.tasks.len(), 2);
    let repair_id = s
        .tasks
        .iter()
        .find(|t| t.recovery_of.is_some())
        .unwrap()
        .id
        .clone();
    for n in 0..25 {
        let run = claim_worker(&mut s, "any-provider", &format!("repair-{n}")).unwrap();
        assert_eq!(run.task_id, repair_id);
        assert_eq!(run.input["recovery_context"]["owner"], "coordinator");
        assert_eq!(
            run.input["recovery_context"]["previous_repair_attempts"]
                .as_array()
                .unwrap()
                .len(),
            n
        );
        fail(&mut s, &run.id, "Investigate the next alternative", "").unwrap();
        reconcile(&mut s).unwrap();
        let repair = s.tasks.iter().find(|t| t.id == repair_id).unwrap();
        assert_eq!(repair.status, "ready");
        assert_eq!(repair.max_attempts, 0);
        assert_eq!(repair.max_sendbacks, 0);
    }
    assert_eq!(
        s.tasks.len(),
        2,
        "Recovery must reuse its incident instead of spawning recursive tasks"
    );
    assert_eq!(
        s.tasks.iter().find(|t| t.id == original.id).unwrap().status,
        "blocked"
    );
    assert_eq!(
        claim_worker(&mut s, "any-provider", "still-working")
            .unwrap()
            .task_id,
        repair_id
    );
}
#[test]
fn restart_keeps_external_ownership_and_running_coordination() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("relay.sqlite");
    let db = Store::open(&file).unwrap();
    let run = db
        .mutate(|s| {
            s.settings.paused = false;
            task(s, "Surviving run");
            Ok(claim_worker(s, "worker", "request").unwrap())
        })
        .unwrap();
    drop(db);
    let db = Store::open(file).unwrap();
    assert_eq!(db.mutate(recover).unwrap(), 0);
    db.mutate(|s| {
        assert!(!s.settings.paused);
        assert_eq!(claim_worker(s, "worker", "request").unwrap().id, run.id);
        finish(s, &run.id, result(), String::new())
    })
    .unwrap();
    assert_eq!(db.read().unwrap().tasks[0].stage, "implementation");
}
#[test]
fn stale_worker_is_fenced_until_acknowledged_then_retried() {
    let mut s = Snapshot::default();
    s.settings.paused = false;
    s.settings.retry_delay_seconds = 0;
    task(&mut s, "Lease");
    let run = claim_worker(&mut s, "worker", "first").unwrap();
    s.attempts[0].external.as_mut().unwrap().heartbeat_at = "2000-01-01T00:00:00Z".into();
    reconcile(&mut s).unwrap();
    assert_eq!(s.attempts[0].status, "lost");
    assert_eq!(
        finish(&mut s, &run.id, result(), String::new())
            .unwrap_err()
            .0,
        409
    );
    assert_eq!(s.tasks[0].stage, "planning");
    assert!(claim_worker(&mut s, "worker", "second").is_none());
    assert_eq!(
        external::heartbeat(&mut s, &run.id).unwrap()["cancel"],
        true
    );
    external::acknowledge_stop(&mut s, &run.id).unwrap();
    let next = claim_worker(&mut s, "worker", "second").unwrap();
    assert_ne!(next.id, run.id);
    assert_eq!(next.number, 1);
    assert_eq!(
        next.input["recovery_context"]["incident"]["task_id"],
        run.task_id
    );
}
#[test]
fn sendback_is_dispatched_without_a_human_and_duplicate_results_are_safe() {
    let mut s = Snapshot::default();
    s.settings.paused = false;
    task(&mut s, "Rework");
    let first = claim_worker(&mut s, "planner", "plan").unwrap();
    finish(&mut s, &first.id, result(), String::new()).unwrap();
    let second = claim_worker(&mut s, "builder", "build").unwrap();
    let mut r = result();
    r.outcome = "send_back".into();
    r.send_back_to = Some("planning".into());
    r.reason = "Missing requirement".into();
    finish(&mut s, &second.id, r.clone(), String::new()).unwrap();
    finish(&mut s, &second.id, r, String::new()).unwrap();
    let third = claim_worker(&mut s, "planner", "replan").unwrap();
    assert_eq!(third.stage, "planning");
    assert_eq!(s.tasks[0].sendbacks, 1);
}
#[test]
fn human_approval_and_pauses_are_never_automatically_bypassed() {
    let mut s = Snapshot::default();
    s.settings.paused = false;
    let gate = create_task(&mut s, json!({"title":"Approve","human_approval":true})).unwrap();
    create_task(
        &mut s,
        json!({"title":"Depends","acceptance":"Checked","dependencies":[gate.id]}),
    )
    .unwrap();
    assert!(claim_worker(&mut s, "worker", "a").is_none());
    assert_eq!(s.tasks[1].stage, "backlog");
    s.settings.paused = true;
    task(&mut s, "Paused");
    assert!(claim_worker(&mut s, "worker", "b").is_none());
}
#[test]
fn reporting_registers_any_provider_without_claiming_task_completion() {
    let mut s = Snapshot::default();
    let t = task(&mut s, "Tracked");
    let report=serde_json::from_value(json!({"source":"arbitrary-runtime","agent_id":"123","name":"Worker A","status":"running","task_id":t.id})).unwrap();
    external::report(&mut s, report).unwrap();
    assert_eq!(s.agents.len(), 1);
    assert!(s.attempts.is_empty());
    assert_eq!(s.tasks[0].stage, "backlog");
}

#[test]
fn new_projects_keep_eligible_work_moving_without_a_start_click() {
    let mut s = Snapshot::default();
    assert!(!s.settings.paused);
    let t = task(&mut s, "Independent work");
    let gate = create_task(
        &mut s,
        json!({"title":"Actual human decision","human_approval":true}),
    )
    .unwrap();
    create_task(
        &mut s,
        json!({"title":"Gated work","acceptance":"Checked","dependencies":[gate.id]}),
    )
    .unwrap();
    assert_eq!(dispatch_status(&s)["mode"], "waiting_for_workers");
    assert_eq!(dispatch_status(&s)["eligible"], 1);
    let report = serde_json::from_value(json!({"source":"other-runtime","agent_id":"observer","name":"Reported worker","status":"running"})).unwrap();
    external::report(&mut s, report).unwrap();
    assert_eq!(dispatch_status(&s)["mode"], "waiting_for_workers");
    for n in 0..3 {
        let run = claim_worker(
            &mut s,
            if n == 2 { "reviewer" } else { "builder" },
            &format!("run-{n}"),
        )
        .unwrap();
        assert_eq!(run.task_id, t.id);
        assert_eq!(dispatch_status(&s)["mode"], "running");
        finish(&mut s, &run.id, result(), String::new()).unwrap();
    }
    assert_eq!(s.tasks[0].status, "completed");
    assert_eq!(s.tasks[1].stage, "backlog");
    assert_eq!(s.tasks[2].stage, "backlog");
    assert!(!s.settings.paused);
    assert_eq!(dispatch_status(&s)["mode"], "blocked");
    s.settings.paused = true;
    assert_eq!(dispatch_status(&s)["mode"], "paused");
}

#[test]
fn manually_resuming_a_lost_run_cannot_start_a_second_worker() {
    let mut s = Snapshot::default();
    s.settings.retry_delay_seconds = 0;
    let t = task(&mut s, "Fenced task without a shared workspace");
    let run = claim_worker(&mut s, "first-worker", "first").unwrap();
    s.attempts[0].external.as_mut().unwrap().heartbeat_at = "2000-01-01T00:00:00Z".into();
    reconcile(&mut s).unwrap();
    let revision = s.tasks[0].revision;
    action(
        &mut s,
        &t.id,
        json!({"action":"resume","revision":revision}),
    )
    .unwrap();
    assert!(claim_worker(&mut s, "second-worker", "second").is_none());
    assert_eq!(s.attempts.len(), 1);
    assert_eq!(dispatch_status(&s)["mode"], "blocked");
    external::acknowledge_stop(&mut s, &run.id).unwrap();
    let next = claim_worker(&mut s, "second-worker", "second").unwrap();
    assert_eq!(next.number, 1);
    assert_eq!(
        next.input["recovery_context"]["incident"]["task_id"],
        run.task_id
    );
}

#[test]
fn stopped_and_failed_workers_stop_reporting_running_immediately() {
    for cancel in [true, false] {
        let mut s = Snapshot::default();
        s.settings.paused = false;
        let t = task(&mut s, "Worker lifecycle");
        let run = claim_worker(&mut s, "worker", "run").unwrap();
        if cancel {
            let revision = s.tasks[0].revision;
            action(&mut s, &t.id, json!({"action":"pause","revision":revision})).unwrap();
            assert_eq!(s.attempts[0].status, "cancelling");
            external::acknowledge_stop(&mut s, &run.id).unwrap();
        } else {
            fail(&mut s, &run.id, "Provider failed", "").unwrap();
        }
        let p = s.agents[0].presence.as_ref().unwrap();
        assert_eq!(p.status, if cancel { "stopped" } else { "failed" });
        assert_eq!(Some(&p.seen_at), s.attempts[0].finished_at.as_ref());
        // Repair old persisted reports without changing task outcomes or history.
        s.agents[0].presence.as_mut().unwrap().status = "running".into();
        let tasks = serde_json::to_value(&s.tasks).unwrap();
        let attempts = serde_json::to_value(&s.attempts).unwrap();
        external::reconcile_presence(&mut s);
        assert_eq!(
            s.agents[0].presence.as_ref().unwrap().status,
            if cancel { "stopped" } else { "failed" }
        );
        assert_eq!(serde_json::to_value(&s.tasks).unwrap(), tasks);
        assert_eq!(serde_json::to_value(&s.attempts).unwrap(), attempts);
        let settled = serde_json::to_value(&s).unwrap();
        external::reconcile_presence(&mut s);
        assert_eq!(serde_json::to_value(&s).unwrap(), settled);
    }
}

#[test]
fn completing_an_attempt_preserves_presence_for_a_different_run() {
    let mut s = Snapshot::default();
    s.settings.paused = false;
    task(&mut s, "Separate presence");
    let run = claim_worker(&mut s, "worker", "run").unwrap();
    let report = serde_json::from_value(json!({"source":"test-provider","agent_id":"worker","name":"worker","status":"running","run_id":"other-work"})).unwrap();
    external::report(&mut s, report).unwrap();
    finish(&mut s, &run.id, result(), String::new()).unwrap();
    assert_eq!(s.agents[0].presence.as_ref().unwrap().status, "running");
    assert_eq!(
        s.agents[0].presence.as_ref().unwrap().run_id.as_deref(),
        Some("other-work")
    );
}

#[test]
fn workflow_upgrade_preserves_legacy_run_and_finishes_in_combined_review() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("old.sqlite");
    let db = Store::open(&file).unwrap();
    let run = db
        .mutate(|s| {
            s.settings.paused = false;
            task(s, "Migrated task");
            let run = claim_worker(s, "legacy-worker", "old-run").unwrap();
            s.tasks[0].stage = "verification".into();
            s.attempts[0].stage = "verification".into();
            s.settings
                .stage_agents
                .insert("verification".into(), run.agent_id.clone());
            s.agents[0].stages = vec!["verification".into()];
            Ok(run)
        })
        .unwrap();
    drop(db);
    let db = Store::open(&file).unwrap();
    let s = db.read().unwrap();
    assert_eq!(s.tasks[0].stage, "review");
    assert_eq!(s.tasks[0].current_attempt.as_deref(), Some(run.id.as_str()));
    assert_eq!(s.attempts[0].stage, "verification");
    assert_eq!(s.agents[0].stages, vec!["review"]);
    assert!(s.settings.stage_agents.contains_key("review"));
    db.mutate(|s| finish(s, &run.id, result(), String::new()))
        .unwrap();
    assert_eq!(db.read().unwrap().tasks[0].stage, "done");
    let revision = db.read().unwrap().revision;
    drop(db);
    assert_eq!(
        Store::open(file).unwrap().read().unwrap().revision,
        revision
    );
}
#[tokio::test]
async fn background_scheduler_completes_local_pipeline_without_clients() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Arc::new(Store::open(tmp.path().join("db")).unwrap());
    db.mutate(|s|{
        for name in ["builder","reviewer"] { save_agent(s,None,json!({"name":name,"adapter":"cli","command":env!("CARGO_BIN_EXE_relay-coordinator"),"args":["example-agent"],"stages":["planning","implementation","review"]}))?; }
        create_task(s,json!({"title":"Continuous pipeline","acceptance":"Fixture passed","tags":["demo"]}))?;Ok(())
    }).unwrap();
    let runner = Runner::new(db.clone(), tmp.path().to_owned());
    runner.start_scheduler();
    let complete = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if db.read().unwrap().tasks[0].status == "completed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    runner.stopping.store(true, Ordering::Relaxed);
    assert!(complete.is_ok());
    assert_eq!(db.read().unwrap().attempts.len(), 3);
}
#[test]
fn epic_rollup_unblocks_dependencies_automatically() {
    let mut s = Snapshot::default();
    s.settings.paused = false;
    let epic = create_task(&mut s, json!({"title":"Epic","kind":"epic"})).unwrap();
    let child = create_task(
        &mut s,
        json!({"title":"Child","parent_id":epic.id,"acceptance":"Done"}),
    )
    .unwrap();
    let dependent = create_task(
        &mut s,
        json!({"title":"Next","dependencies":[epic.id],"acceptance":"Done"}),
    )
    .unwrap();
    let t = s.tasks.iter_mut().find(|t| t.id == child.id).unwrap();
    t.stage = "done".into();
    t.status = "completed".into();
    reconcile(&mut s).unwrap();
    assert_eq!(
        s.tasks.iter().find(|t| t.id == epic.id).unwrap().status,
        "completed"
    );
    assert_eq!(
        s.tasks.iter().find(|t| t.id == dependent.id).unwrap().stage,
        "planning"
    );
}
