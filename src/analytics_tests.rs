use super::*;
use crate::engine::{Attempt, Engine};
use crate::history::{AttemptEvidence, Usage};
use crate::test_support::Temp;

fn attempt(stage: &str, status: &str) -> Attempt {
    Attempt { id: format!("{stage}-{status}"), revision: 0, stage: stage.into(), started: 0, ended: None, result: Some(json!({"status":status})), error: None, evidence: AttemptEvidence::default() }
}
fn create(engine: &mut Engine, ws: &std::path::Path, title: &str, patch: Value) -> usize {
    let value = crate::test_support::merge(json!({"title":title,"outcome":"Outcome","cwd":ws,"acceptance":["Criterion"],"standalone":true}), patch);
    let id = engine.create(value).unwrap()["id"].as_str().unwrap().to_owned();
    engine.task(&id).unwrap()
}
fn board() -> (Temp, Engine) {
    let temp = Temp::new();
    let ws = temp.dir("ws");
    let mut engine = temp.engine();
    let first = create(&mut engine, &ws, "First pass", json!({}));
    let second = create(&mut engine, &ws, "Second pass", json!({}));
    let docs = create(&mut engine, &ws, "Docs", json!({"attributes":{"lane":"docs"}}));
    let container = create(&mut engine, &ws, "Group", json!({"acceptance":[]}));
    let tasks = &mut engine.state.tasks;
    tasks[first].attempts = vec![attempt("implementation", "completed"), attempt("review", "accepted")];
    tasks[first].stage = "done".into();
    tasks[first].attempts[0].evidence.usage = Some(Usage { total_tokens: Some(100), components: None, source: "test".into() });
    tasks[second].attempts = vec![attempt("implementation", "completed"), attempt("review", "rejected"), attempt("implementation", "completed"), attempt("review", "accepted")];
    tasks[second].stage = "done".into();
    let mut failed = attempt("implementation", "blocked");
    failed.result = None;
    failed.error = Some("Worker exited 1".into());
    tasks[docs].attempts = vec![failed, attempt("implementation", "blocked"), attempt("replan", "replanned")];
    tasks[docs].blocked = true;
    tasks[container].stage = "done".into();
    (temp, engine)
}
fn row<'a>(report: &'a Value, title: &str) -> &'a Value {
    report["tasks"].as_array().unwrap().iter().find(|row| row["title"] == title).unwrap()
}

#[test]
fn report_summarizes_all_tasks_and_outcomes() {
    let (_temp, engine) = board();
    let value = report(&engine.state, json!({})).unwrap();
    assert_eq!(value["metricDefinitionVersion"], json!(crate::history::METRIC_DEFINITION_VERSION));
    assert!(value["groupBy"].is_null());
    let all = &value["groups"]["all"];
    assert_eq!(all["tasks"], 4);
    assert_eq!(all["firstPass"], json!({"n":2,"mean":0.5}));
    assert_eq!(all["attempts"], json!({"n":4,"mean":2.25}));
    assert_eq!(all["rejections"]["mean"], 0.25);
    assert_eq!(all["replans"]["mean"], 0.25);
    let first = &row(&value, "First pass")["outcomes"];
    assert_eq!((first["firstPass"].as_u64(), first["tokens"].as_u64(), first["implementations"].as_u64()), (Some(1), Some(100), Some(1)));
    assert_eq!(row(&value, "Second pass")["outcomes"]["firstPass"], 0);
    let docs = &row(&value, "Docs")["outcomes"];
    assert_eq!((docs["errors"].as_u64(), docs["blocks"].as_u64(), docs["held"].as_bool(), docs["replans"].as_u64()), (Some(1), Some(1), Some(true), Some(1)));
    assert!(docs["firstPass"].is_null());
    assert!(row(&value, "Group")["outcomes"]["firstPass"].is_null());
    assert_eq!(row(&value, "Group")["features"]["container"], true);
    assert_eq!(row(&value, "Docs")["features"]["words"], 3);
}

#[test]
fn report_groups_by_feature_and_filters_rows() {
    let (_temp, engine) = board();
    let grouped = report(&engine.state, json!({"groupBy":"lane"})).unwrap();
    let keys: Vec<_> = grouped["groups"].as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["\"code\"", "\"docs\""]);
    assert_eq!(grouped["groups"]["\"code\""]["tasks"], 3);
    assert_eq!(grouped["groups"]["\"docs\""]["firstPass"], json!({"n":0,"mean":null}));
    let containers = report(&engine.state, json!({"groupBy":"container"})).unwrap();
    assert_eq!(containers["groups"]["true"]["tasks"], 1);
    let filtered = report(&engine.state, json!({"filters":{"lane":"docs"}})).unwrap();
    assert_eq!(filtered["tasks"].as_array().unwrap().len(), 1);
    let none = report(&engine.state, json!({"filters":{"lane":"docs","criteria":2}})).unwrap();
    assert!(none["tasks"].as_array().unwrap().is_empty());
    assert!(none["groups"].as_object().unwrap().is_empty());
}

#[test]
fn report_windows_by_creation_time_and_includes_archived_tasks() {
    let (_temp, mut engine) = board();
    let now = crate::now();
    assert_eq!(report(&engine.state, json!({"from":now + 60_000})).unwrap()["tasks"].as_array().unwrap().len(), 0);
    assert_eq!(report(&engine.state, json!({"to":0})).unwrap()["tasks"].as_array().unwrap().len(), 0);
    assert_eq!(report(&engine.state, json!({"from":0,"to":now + 60_000})).unwrap()["tasks"].as_array().unwrap().len(), 4);
    let mut untimed = engine.state.tasks[0].clone();
    untimed.history.clear();
    untimed.id = "archived".into();
    engine.state.archived.push(untimed);
    assert_eq!(report(&engine.state, json!({"from":now + 60_000})).unwrap()["tasks"].as_array().unwrap().len(), 1);
}

#[test]
fn report_rejects_unknown_groups_and_fields() {
    let (_temp, engine) = board();
    assert!(report(&engine.state, json!({"groupBy":"title"})).unwrap_err().to_string().contains("groupBy must be"));
    assert!(report(&engine.state, json!({"group":"lane"})).is_err());
    assert!(report(&engine.state, json!({"from":"yesterday"})).is_err());
    for group in GROUPS {
        report(&engine.state, json!({"groupBy":group})).unwrap();
    }
}
