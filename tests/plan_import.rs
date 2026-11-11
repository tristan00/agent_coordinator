use relay_coordinator::{engine, model::Snapshot, plan, store::Store};
use serde_json::{Value, json, to_value};

#[test]
fn documented_hierarchy_runs_children_then_parent_then_epic_without_manual_routing() {
    let dir = tempfile::tempdir().unwrap();
    let contract = include_str!("../docs/PROJECT_PLANNER.md").replace("\r\n", "\n");
    let example = contract
        .split("```json\n")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    let mut manifest: Value = serde_json::from_str(example).unwrap();
    manifest["workspaces"]["app"] = json!(dir.path());
    let mut state = Snapshot::default();
    plan::reconcile(&mut state, request(&manifest, json!({}), json!({})), false).unwrap();
    for stage in ["planning", "implementation", "review"] {
        engine::save_agent(
            &mut state,
            None,
            json!({"name":stage,"adapter":"demo","stages":[stage]}),
        )
        .unwrap();
    }
    for task in &mut state.tasks {
        task.tags.push("demo".into());
    }
    let id = |code: &str| {
        state
            .tasks
            .iter()
            .find(|t| t.imported.as_ref().unwrap().code == code)
            .unwrap()
            .id
            .clone()
    };
    let (leaf, dependent, parent, epic) = (id("S"), id("C"), id("T"), id("E"));
    let mut sequence = vec![];
    for _ in 0..12 {
        engine::reconcile(&mut state).unwrap();
        let Some((attempt, _)) = engine::claim(&mut state, None).unwrap() else {
            break;
        };
        if attempt.task_id == parent {
            assert_eq!(attempt.input["children"].as_array().unwrap().len(), 2);
            assert_eq!(
                attempt.input["child_artifacts"].as_array().unwrap().len(),
                6
            );
        }
        if attempt.task_id == dependent {
            assert_eq!(
                attempt.input["dependency_artifacts"]
                    .as_array()
                    .unwrap()
                    .len(),
                3
            );
            assert_eq!(attempt.input["parent"]["id"], parent);
        }
        sequence.push(attempt.task_id.clone());
        engine::finish(
            &mut state,
            &attempt.id,
            relay_coordinator::runner::demo_result(&attempt.input),
            String::new(),
        )
        .unwrap();
    }
    engine::reconcile(&mut state).unwrap();
    assert_eq!(
        sequence,
        vec![
            leaf.clone(),
            leaf.clone(),
            leaf,
            dependent.clone(),
            dependent.clone(),
            dependent,
            parent.clone(),
            parent.clone(),
            parent
        ]
    );
    assert!(state.tasks.iter().all(|t| t.status == "completed"));
    assert!(!state.attempts.iter().any(|a| a.task_id == epic));

    manifest["tasks"][1]["parent"] = json!("missing");
    assert!(
        plan::reconcile(
            &mut Snapshot::default(),
            request(&manifest, json!({}), json!({})),
            true
        )
        .is_err()
    );
    manifest["tasks"][1]["parent"] = json!("T");
    manifest["tasks"][1]["deps"] = json!(["C"]);
    assert!(
        plan::reconcile(
            &mut Snapshot::default(),
            request(&manifest, json!({}), json!({})),
            true
        )
        .is_err()
    );
    manifest["tasks"][1]["depends_on"] = json!(["C"]);
    assert!(serde_json::from_value::<plan::ImportRequest>(json!({"plan":manifest})).is_err());
}

fn request(plan: &Value, revisions: Value, adopt: Value) -> plan::ImportRequest {
    serde_json::from_value(json!({"plan":plan,"expected_revisions":revisions,"adopt":adopt}))
        .unwrap()
}

fn manifest(workspace: &std::path::Path) -> Value {
    json!({"schema_version":1,"plan_id":"release-plan","workspaces":{"local":workspace},
    "epics":[{"code":"E","title":"Deliver release","description":"Release tasks","workspace":"local"}],
    "tasks":[
        {"code":"P00","title":"Approve plan","description":"Capture approval","acceptance":["Approval recorded"],"points":1,"priority":1,"task_type":"operations","workspace":"local","epic":"E","human_approval":true},
        {"code":"T01","title":"Implement","description":"Implement requirements","acceptance":["Acceptance met"],"points":3,"priority":1,"task_type":"feature","workspace":"local","epic":"E","deps":["P00"]}
    ]})
}

#[test]
fn previews_and_imports_idempotently_then_updates_with_current_revision() {
    let dir = tempfile::tempdir().unwrap();
    let mut manifest = manifest(dir.path());
    let db = Store::open(":memory:").unwrap();
    let empty = json!({});
    let preview = to_value(
        plan::reconcile(
            &mut db.read().unwrap(),
            request(&manifest, empty.clone(), empty.clone()),
            true,
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        preview["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["operation"] == "create" && entry["task_id"].is_null())
    );
    assert!(db.read().unwrap().tasks.is_empty());

    db.mutate(|s| plan::reconcile(s, request(&manifest, empty.clone(), empty.clone()), false))
        .unwrap();
    let after_create = db.read().unwrap();
    let gate = &after_create.tasks[1];
    let task = &after_create.tasks[2];
    assert_eq!(task.dependencies, vec![gate.id.clone()]);
    assert_eq!(
        task.parent_id.as_deref(),
        Some(after_create.tasks[0].id.as_str())
    );
    assert!(gate.human_approval);
    db.mutate(|s| {
        engine::action(
            s,
            &task.id,
            json!({"action":"comment","reason":"Keep this audit record","revision":task.revision}),
        )
    })
    .unwrap();
    db.mutate(|s| engine::update_task(s, &task.id, json!({"priority":2,"revision":task.revision})))
        .unwrap();
    let before_noop = to_value(db.read().unwrap()).unwrap();
    db.mutate(|s| plan::reconcile(s, request(&manifest, empty.clone(), empty.clone()), false))
        .unwrap();
    assert_eq!(to_value(db.read().unwrap()).unwrap(), before_noop);

    manifest["tasks"][1]["description"] = json!("Updated requirements");
    let stale = db.mutate(|s| {
        plan::reconcile(
            s,
            request(&manifest, json!({"T01":task.revision}), empty.clone()),
            false,
        )
    });
    assert_eq!(stale.err().unwrap().0, 409);
    assert_eq!(to_value(db.read().unwrap()).unwrap(), before_noop);
    let changed = db
        .mutate(|s| {
            plan::reconcile(
                s,
                request(&manifest, json!({"T01":task.revision + 1}), empty.clone()),
                false,
            )
        })
        .unwrap();
    assert_eq!(
        to_value(changed).unwrap()["entries"][2]["revision"],
        task.revision + 2
    );
    let updated = db.read().unwrap();
    assert_eq!(updated.tasks[2].id, task.id);
    assert_eq!(updated.tasks[2].description, "Updated requirements");
    assert_eq!(updated.tasks[2].priority, 2);
    assert!(
        updated
            .events
            .iter()
            .any(|event| event.message == "Keep this audit record")
    );
}

#[test]
fn rejects_invalid_manifests_without_mutating_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let original = manifest(dir.path());
    let mut missing = original.clone();
    missing["tasks"][1]["deps"] = json!(["BAD"]);
    let mut cycle = original.clone();
    cycle["tasks"][0]["deps"] = json!(["T01"]);
    let mut hierarchy_cycle = original.clone();
    hierarchy_cycle["tasks"][0]["deps"] = json!(["E"]);
    let mut duplicate = original.clone();
    duplicate["tasks"][1]["code"] = json!("E");
    let mut invalid_workspace = original;
    invalid_workspace["workspaces"]["local"] = json!(dir.path().join("missing"));
    for invalid in [
        missing,
        cycle,
        hierarchy_cycle,
        duplicate,
        invalid_workspace,
    ] {
        for dry_run in [true, false] {
            let mut snapshot = Snapshot::default();
            let error = plan::reconcile(
                &mut snapshot,
                request(&invalid, json!({}), json!({})),
                dry_run,
            )
            .err()
            .unwrap();
            assert_eq!(error.0, 400, "{error}");
            assert!(snapshot.tasks.is_empty());
            assert!(snapshot.events.is_empty());
        }
    }
}

#[test]
fn old_database_task_deserializes_and_adopts_without_replacing_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relay.sqlite");
    let db = Store::open(&path).unwrap();
    let original = db
        .mutate(|s| engine::create_task(s, json!({"title":"Manually created epic","kind":"epic"})))
        .unwrap();
    drop(db);
    let sql = rusqlite::Connection::open(&path).unwrap();
    sql.execute(
        "UPDATE tasks SET data = json_remove(data, '$.human_approval', '$.approval', '$.imported')",
        [],
    )
    .unwrap();
    drop(sql);
    let db = Store::open(&path).unwrap();
    let manifest = manifest(dir.path());
    let adopt = json!({"E":original.id});
    let stale =
        db.mutate(|s| plan::reconcile(s, request(&manifest, json!({}), adopt.clone()), false));
    assert_eq!(stale.err().unwrap().0, 409);
    db.mutate(|s| {
        plan::reconcile(
            s,
            request(&manifest, json!({"E":original.revision}), adopt),
            false,
        )
    })
    .unwrap();
    let adopted = db.read().unwrap();
    assert_eq!(adopted.tasks.len(), 3);
    assert_eq!(adopted.tasks[0].id, original.id);
    assert_eq!(adopted.tasks[0].title, original.title);
    assert!(
        adopted
            .events
            .iter()
            .any(|event| event.kind == "created" && event.task_id.as_ref() == Some(&original.id))
    );
    assert_eq!(adopted.tasks[0].imported.as_ref().unwrap().code, "E");
}

#[test]
fn rejects_cycles_through_unmanaged_tasks_during_reparenting() {
    let dir = tempfile::tempdir().unwrap();
    let mut manifest = manifest(dir.path());
    manifest["epics"].as_array_mut().unwrap().push(json!({
        "code": "B", "title": "Another epic", "description": "Independent work", "workspace": "local"
    }));
    let db = Store::open(":memory:").unwrap();
    db.mutate(|s| plan::reconcile(s, request(&manifest, json!({}), json!({})), false))
        .unwrap();
    let before = db.read().unwrap();
    let imported = |code: &str| {
        before
            .tasks
            .iter()
            .find(|task| task.imported.as_ref().unwrap().code == code)
            .unwrap()
    };
    let new_parent = imported("B");
    let task = imported("T01");
    db.mutate(|s| {
        engine::create_task(
            s,
            json!({"title":"Manual subtask", "kind":"subtask",
        "parent_id":task.id,"dependencies":[new_parent.id]}),
        )
    })
    .unwrap();
    let before_attempt = to_value(db.read().unwrap()).unwrap();
    manifest["tasks"][1]["epic"] = json!("B");
    for dry_run in [true, false] {
        let error = db
            .mutate(|s| {
                plan::reconcile(
                    s,
                    request(&manifest, json!({"T01":task.revision}), json!({})),
                    dry_run,
                )
            })
            .err()
            .unwrap();
        assert_eq!(error.0, 400, "{error}");
        assert!(error.1.contains("cycle"), "{error}");
        assert_eq!(to_value(db.read().unwrap()).unwrap(), before_attempt);
    }
}
