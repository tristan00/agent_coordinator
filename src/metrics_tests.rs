use super::*;
use crate::engine::{Attempt, Task};
use crate::history::{Actor, Launch};
use crate::test_support::Temp;

fn task(stage: &str) -> Task {
    serde_json::from_value(json!({"id":"t","title":"T","outcome":"O","cwd":"C:\\","acceptance":["c"],"dependencies":[],
        "stage":stage,"stopped":false,"attempts":[],"active":null,"feedback":[],"failure":"","retryAt":0})).unwrap()
}
fn push(task: &mut Task, kind: &str, at: u64, evidence: Value) {
    let mut event = Event::new(task, kind, Actor::Scheduler, "test", evidence);
    event.at = at;
    task.history.push(event);
}
fn totals(task: &Task) -> Value {
    lifecycle(task)["intervals"]["milliseconds"].clone()
}

#[test]
fn fingerprint_is_fnv1a_64() {
    assert_eq!(fingerprint(b""), 0xcbf29ce484222325);
    assert_eq!(fingerprint(b"a"), 0xaf63dc4c8601ec8c);
    assert_ne!(fingerprint(b"ab"), fingerprint(b"ba"));
}

#[test]
fn common_lines_finds_longest_common_subsequence() {
    assert_eq!(common_lines(&[1, 2, 3], &[1, 2, 3]), 3);
    assert_eq!(common_lines(&[1, 2, 3], &[1, 4, 3]), 2);
    assert_eq!(common_lines(&[1, 2], &[1, 9, 2]), 2);
    assert_eq!(common_lines(&[], &[1, 2]), 0);
    assert_eq!(common_lines(&[1, 2, 3, 4], &[3, 4, 1, 2]), 2);
    assert_eq!(common_lines(&[5, 1, 5, 2, 5], &[1, 2]), 2);
    assert_eq!(common_lines(&[7, 7], &[7]), 1);
}

#[test]
fn categories_follow_documented_precedence() {
    let cases = [
        ("src/main.rs", "source"),
        ("src/engine_tests.rs", "test"),
        ("web/app.js", "ui"),
        ("target/debug/x.rs", "generated"),
        ("README.md", "other"),
        ("frontend/src/a.test.ts", "test"),
        ("tests/node_modules/x.js", "generated"),
        ("lib.rs", "source"),
        ("page.HTML", "ui"),
        ("Src/Main.RS", "source"),
        ("docs/notes.txt", "other"),
    ];
    for (path, expected) in cases {
        assert_eq!(category(Path::new(path)), expected, "{path}");
    }
}

#[test]
fn wait_reasons_map_to_interval_buckets() {
    assert_eq!(wait_bucket("Waiting for writable scope C:\\x"), "resource");
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

fn workspace() -> (Temp, PathBuf) {
    let temp = Temp::new();
    let ws = temp.dir("ws");
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src").join("a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
    fs::write(ws.join("notes.txt"), "hello").unwrap();
    fs::write(ws.join("bin.dat"), [0u8, 1, 2]).unwrap();
    fs::write(ws.join("w.rs"), "a\nb\n").unwrap();
    (temp, ws)
}

#[test]
fn capture_records_text_and_binary_files_and_excludes_runtime_trees() {
    let (temp, ws) = workspace();
    for excluded in ["node_modules", "Data", "runtime-data-7", "board"] {
        fs::create_dir_all(ws.join(excluded)).unwrap();
        fs::write(ws.join(excluded).join("f.txt"), "x").unwrap();
    }
    let snapshot = capture(&[ws.clone(), temp.path().join("absent")], &ws.join("board"));
    let files: Vec<_> = snapshot.files.keys().map(|p| p.strip_prefix(&ws).unwrap().to_path_buf()).collect();
    assert_eq!(files, [PathBuf::from("bin.dat"), PathBuf::from("notes.txt"), PathBuf::from("src").join("a.rs"), PathBuf::from("w.rs")]);
    assert!(snapshot.files[&ws.join("bin.dat")].lines.is_none());
    assert_eq!(snapshot.files[&ws.join("notes.txt")].lines.as_deref(), Some(&[fingerprint(b"hello")][..]));
    assert_eq!(snapshot.excluded.len(), 4);
    assert!(snapshot.skipped.is_empty());
}

#[test]
fn compare_reports_line_changes_whitespace_edits_and_totals() {
    let (_temp, ws) = workspace();
    let before = capture(std::slice::from_ref(&ws), Path::new("Z:\\unused"));
    fs::write(ws.join("src").join("a.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
    fs::remove_file(ws.join("notes.txt")).unwrap();
    fs::write(ws.join("src").join("new.rs"), "x\ny\n").unwrap();
    fs::write(ws.join("w.rs"), "  a\n\nb  \n").unwrap();
    fs::write(ws.join("bin.dat"), [0u8, 1, 3]).unwrap();
    let after = capture(std::slice::from_ref(&ws), Path::new("Z:\\unused"));
    let value = compare(before, after, &ws, &[]);
    let files: BTreeMap<String, Value> = value["files"].as_array().unwrap().iter()
        .map(|f| (Path::new(f["path"].as_str().unwrap()).strip_prefix(&ws).unwrap().to_string_lossy().into_owned(), f.clone())).collect();
    let summary = |name: &str| (files[name]["added"].clone(), files[name]["deleted"].clone(), files[name]["category"].clone());
    assert_eq!(summary("src\\a.rs"), (json!(1), json!(0), json!("source")));
    assert_eq!(summary("notes.txt"), (json!(0), json!(1), json!("other")));
    assert_eq!(summary("src\\new.rs"), (json!(2), json!(0), json!("source")));
    assert_eq!(summary("w.rs"), (json!(3), json!(2), json!("source")));
    assert_eq!(summary("bin.dat"), (Value::Null, Value::Null, json!("other")));
    assert_eq!(files["w.rs"]["whitespaceOnly"], true);
    assert_eq!(files["src\\a.rs"]["whitespaceOnly"], false);
    assert!(files["notes.txt"]["whitespaceOnly"].is_null() && files["notes.txt"]["afterFingerprint"].is_null());
    assert!(files["src\\new.rs"]["beforeFingerprint"].is_null());
    assert_eq!(value["totals"], json!({"uniqueFiles":5,"added":6,"deleted":3,"net":3,"churn":9,"unknownLineCounts":1}));
    assert_eq!(value["attribution"], "scoped_observation");
    assert_eq!(value["scopeSize"]["after"]["byCategory"]["source"]["files"], 3);
}

#[test]
fn compare_marks_peers_ambiguous_and_ignores_skipped_roots() {
    let (_temp, ws) = workspace();
    let peers = ["other-attempt".to_string()];
    let same = compare(capture(std::slice::from_ref(&ws), Path::new("Z:\\")), capture(std::slice::from_ref(&ws), Path::new("Z:\\")), &ws, &peers);
    assert_eq!(same["files"], json!([]));
    assert_eq!((same["attribution"].as_str(), same["overlappingAttempts"].clone()), (Some("ambiguous"), json!(peers)));
    let before = capture(std::slice::from_ref(&ws), Path::new("Z:\\"));
    fs::write(ws.join("src").join("a.rs"), "changed").unwrap();
    let mut after = capture(std::slice::from_ref(&ws), Path::new("Z:\\"));
    after.skipped.insert(ws.join("src"));
    let value = compare(before, after, &ws, &[]);
    assert_eq!(value["files"], json!([]));
    assert_eq!(value["skipped"].as_array().unwrap().len(), 1);
}

#[test]
fn size_counts_lines_by_category() {
    let temp = Temp::new();
    let ws = temp.dir("ws");
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::create_dir_all(ws.join("web")).unwrap();
    fs::write(ws.join("src").join("a.rs"), "x\n\n  \ny\n").unwrap();
    fs::write(ws.join("web").join("index.html"), "<p>\n").unwrap();
    fs::write(ws.join("blob.bin"), [0u8, 0, 1]).unwrap();
    let value = size(&capture(std::slice::from_ref(&ws), &temp.path().join("data")), &ws);
    assert_eq!(value["byCategory"]["source"], json!({"files":1,"textFiles":1,"binaryOrNonUtf8Files":0,"physicalLines":4,"nonblankLines":2}));
    assert_eq!(value["byCategory"]["ui"]["physicalLines"], 1);
    assert_eq!(value["byCategory"]["other"], json!({"files":1,"textFiles":0,"binaryOrNonUtf8Files":1,"physicalLines":0,"nonblankLines":0}));
    assert_eq!(value["status"], "measured");
    assert_eq!(value["fingerprints"].as_object().unwrap().len(), 3);
    fs::create_dir_all(ws.join("node_modules")).unwrap();
    let partial = size(&capture(std::slice::from_ref(&ws), &temp.path().join("data")), &ws);
    assert_eq!(partial["status"], "partial");
}

#[test]
fn size_delta_compares_matching_rule_sets_only() {
    let temp = Temp::new();
    let ws = temp.dir("ws");
    fs::write(ws.join("a.rs"), "x\n").unwrap();
    let before = size(&capture(std::slice::from_ref(&ws), Path::new("Z:\\")), &ws);
    fs::write(ws.join("b.rs"), "y\nz\n").unwrap();
    let after = size(&capture(std::slice::from_ref(&ws), Path::new("Z:\\")), &ws);
    let delta = size_delta(&before, &after);
    assert_eq!(delta["includedCategories"]["source"], json!({"files":1,"physicalLines":2,"nonblankLines":2}));
    assert_eq!(delta["includedCategories"]["ui"]["files"], 0);
    assert_eq!(delta["status"], "measured");
    assert!(size_delta(&Value::Null, &after).is_null());
    assert!(size_delta(&before, &Value::Null).is_null());
    let mut changed = after.clone();
    changed["rules"]["version"] = json!(2);
    assert!(size_delta(&before, &changed).is_null());
    let mut partial = after;
    partial["status"] = json!("partial");
    assert_eq!(size_delta(&before, &partial)["status"], "partial");
}

#[test]
fn project_capture_writes_each_endpoint_once() {
    let temp = Temp::new();
    let ws = temp.dir("ws");
    fs::write(ws.join("a.rs"), "x\n").unwrap();
    let directory = temp.dir("data/attempts/a1");
    let request = json!({"task":{"workspaceId":ws,"cwd":"ignored"}});
    project_capture(&directory, &request, false).unwrap();
    let before: Value = crate::read(&directory.join("project-before.json")).unwrap();
    assert_eq!(before["byCategory"]["source"]["files"], 1);
    fs::write(directory.join("project-before.json"), "{\"kept\":true}").unwrap();
    project_capture(&directory, &request, false).unwrap();
    assert_eq!(crate::read::<Value>(&directory.join("project-before.json")).unwrap(), json!({"kept":true}));
    project_capture(&directory, &json!({"task":{"cwd":ws}}), true).unwrap();
    assert!(directory.join("project-after.json").exists());
    assert!(project_capture(&directory, &json!({"task":{}}), true).is_err());
    assert!(project_capture(&directory, &json!({"task":{"workspaceId":ws.join("absent")}}), true).is_err());
}

#[test]
fn clean_removes_snapshots_and_tolerates_missing_files() {
    let temp = Temp::new();
    let directory = temp.dir("attempt");
    for name in ["baseline.json", "final-snapshot.json", "changes.json"] {
        fs::write(directory.join(name), "{}").unwrap();
    }
    clean(&directory);
    clean(&directory);
    assert!(!directory.join("baseline.json").exists() && !directory.join("final-snapshot.json").exists());
    assert!(directory.join("changes.json").exists());
}

#[test]
fn snapshots_round_trip_through_json_with_defaults() {
    let (_temp, ws) = workspace();
    let snapshot = capture(std::slice::from_ref(&ws), Path::new("Z:\\"));
    let files = snapshot.files.len();
    let mut value = serde_json::to_value(&snapshot).unwrap();
    for field in ["excluded", "elapsed_ms", "roots"] {
        value.as_object_mut().unwrap().remove(field);
    }
    let restored: Snapshot = serde_json::from_value(value).unwrap();
    assert_eq!((restored.files.len(), restored.elapsed_ms, restored.roots.len()), (files, 0, 0));
}
