use super::*;
use crate::history::AttemptEvidence;
use crate::test_support::Temp;

fn attempt(configuration: Option<Value>) -> Attempt {
    Attempt { id: "a".into(), revision: 0, stage: "implementation".into(), started: 0, ended: None, result: None, error: None,
        evidence: AttemptEvidence { configuration, ..Default::default() } }
}

#[test]
fn digest_is_prefixed_sha256() {
    let temp = Temp::new();
    let file = temp.path().join("abc.txt");
    fs::write(&file, "abc").unwrap();
    assert_eq!(digest(&file).unwrap(), "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert!(digest(&temp.path().join("missing")).is_err());
}

#[test]
fn safe_paths_stay_inside_their_root() {
    let temp = Temp::new();
    let root = temp.dir("root");
    let outside = temp.dir("outside");
    fs::write(outside.join("x.txt"), "x").unwrap();
    fs::create_dir_all(root.join("deliverables")).unwrap();
    fs::write(root.join("deliverables").join("report.md"), "r").unwrap();
    assert_eq!(safe_path(&root, "deliverables/report.md"), Some(root.join("deliverables/report.md")));
    assert_eq!(safe_path(&root, "missing.md"), None);
    assert!(safe_file(&root, "deliverables/report.md").is_some());
    assert!(safe_file(&root, "deliverables").is_none());
    assert!(safe_path(&root, "deliverables").is_some());
}

#[test]
fn hard_linked_files_are_not_trusted() {
    let temp = Temp::new();
    let root = temp.dir("root");
    fs::create_dir_all(root.join("deliverables")).unwrap();
    let original = temp.path().join("original.md");
    fs::write(&original, "r").unwrap();
    fs::hard_link(&original, root.join("deliverables").join("report.md")).unwrap();
    assert!(crate::engine::linked_file(&original));
    assert!(safe_file(&root, "deliverables/report.md").is_none());
    assert!(artifact(&root, Artifact::Report).is_none());
    assert!(!crate::engine::linked_file(&temp.path().join("missing")));
}

#[test]
fn artifact_names_and_records() {
    assert_eq!(artifact_name(Artifact::None), None);
    assert_eq!(artifact_name(Artifact::Candidate), Some("deliverables/candidate.exe"));
    assert_eq!(artifact_name(Artifact::Report), Some("deliverables/report.md"));
    let temp = Temp::new();
    let root = temp.dir("root");
    assert!(artifact(&root, Artifact::None).is_none());
    assert!(artifact(&root, Artifact::Report).is_none());
    fs::create_dir_all(root.join("deliverables")).unwrap();
    fs::write(root.join("deliverables").join("report.md"), "abc").unwrap();
    let record = artifact(&root, Artifact::Report).unwrap();
    assert_eq!(record["digest"], "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(PathBuf::from(record["path"].as_str().unwrap()), root.join("deliverables/report.md"));
}

#[test]
fn recover_artifact_moves_stray_scratch_output_without_overwriting() {
    let temp = Temp::new();
    let root = temp.dir("root");
    let stray = root.join("scratch").join("deliverables");
    fs::create_dir_all(&stray).unwrap();
    fs::write(stray.join("report.md"), "from scratch").unwrap();
    recover_artifact(&root, "deliverables/report.md");
    assert_eq!(fs::read_to_string(root.join("deliverables").join("report.md")).unwrap(), "from scratch");
    assert!(!stray.join("report.md").exists());
    fs::write(stray.join("report.md"), "newer").unwrap();
    recover_artifact(&root, "deliverables/report.md");
    assert_eq!(fs::read_to_string(root.join("deliverables").join("report.md")).unwrap(), "from scratch");
    let other = temp.dir("other");
    fs::create_dir_all(other.join("scratch").join("deliverables")).unwrap();
    let source = temp.path().join("linked.md");
    fs::write(&source, "linked").unwrap();
    fs::hard_link(&source, other.join("scratch").join("deliverables").join("report.md")).unwrap();
    recover_artifact(&other, "deliverables/report.md");
    assert!(!other.join("deliverables").join("report.md").exists());
    recover_artifact(&temp.dir("empty"), "deliverables/report.md");
}

#[test]
fn capture_trace_keeps_only_completed_tool_results() {
    let temp = Temp::new();
    let lines = [
        r#"{"type":"item.completed","item":{"type":"command_execution","exit_code":0,"aggregated_output":"ok"}}"#,
        r#"{"type":"item.completed","item":{"type":"command_execution","exit_code":null,"aggregated_output":"ok"}}"#,
        r#"{"type":"item.completed","item":{"type":"mcp_tool_call","result":{"content":[]}}}"#,
        r#"{"type":"item.completed","item":{"type":"mcp_tool_call","error":{"message":"x"}}}"#,
        r#"{"type":"item.completed","item":{"type":"mcp_tool_call","result":"text"}}"#,
        r#"{"type":"item.started","item":{"type":"command_execution","exit_code":0,"aggregated_output":"ok"}}"#,
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"hi"}}"#,
        "not json",
    ];
    fs::write(temp.path().join("events.jsonl"), lines.join("\n")).unwrap();
    capture_trace(temp.path()).unwrap();
    let trace = fs::read_to_string(temp.path().join("review-trace.jsonl")).unwrap();
    assert_eq!(trace.lines().collect::<Vec<_>>(), [lines[0], lines[2], lines[3]]);
    assert!(capture_trace(&temp.dir("empty")).is_err());
}

#[test]
fn clean_scratch_removes_only_declared_scratch() {
    let temp = Temp::new();
    for (configuration, removed) in [(Some(json!({"scratch":true})), true), (Some(json!({"scratch":false})), false), (None, false)] {
        let directory = temp.dir(&format!("attempt-{}", crate::engine::id()));
        fs::create_dir_all(directory.join("scratch").join("nested")).unwrap();
        fs::write(directory.join("scratch").join("nested").join("f"), "x").unwrap();
        clean_scratch(&directory, &attempt(configuration));
        assert_eq!(!directory.join("scratch").exists(), removed);
        assert!(!directory.join("cleanup.json").exists());
    }
    let missing = temp.dir("no-scratch");
    clean_scratch(&missing, &attempt(Some(json!({"scratch":true}))));
    assert!(!missing.join("cleanup.json").exists());
}
