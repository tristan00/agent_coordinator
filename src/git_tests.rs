use super::*;
use crate::test_support::Temp;
use std::fs;

fn repository(temp: &Temp) -> (PathBuf, PathBuf) {
    let workspace = temp.dir("workspace");
    let repository = temp.dir("workspace/game");
    run(&repository, &["init", "-q", "-b", "main"]).unwrap();
    fs::write(repository.join("a.txt"), "one\n").unwrap();
    run(&repository, &["add", "-A"]).unwrap();
    run(&repository, &["commit", "-q", "-m", "Baseline"]).unwrap();
    (workspace, repository)
}
fn worktree(git: &Value) -> PathBuf {
    PathBuf::from(git["worktree"].as_str().unwrap())
}

#[test]
fn a_task_works_on_its_own_branch_and_merges_when_accepted() {
    let temp = Temp::new();
    let (workspace, repository) = repository(&temp);
    fs::create_dir_all(repository.join("src")).unwrap();
    let git = prepare(&workspace, &repository.join("src"), "R1-1", &[], "implementation").unwrap().unwrap();
    let place = worktree(&git);
    assert_eq!((git["branch"].as_str(), git["main"].as_str(), git["conflict"].as_bool()), (Some("task/R1-1"), Some("main"), Some(false)));
    assert_eq!(relative(&place, &workspace), Some(PathBuf::from("worktrees/R1-1")));
    assert_eq!(git["cwd"].as_str().map(PathBuf::from), Some(PathBuf::from(shown(&place.join("src")))));
    assert_eq!(shown(&map(&repository.join("src/lib.rs"), &git)), shown(&place.join("src/lib.rs")));
    assert_eq!(map(&temp.path().join("elsewhere"), &git), temp.path().join("elsewhere"));
    fs::write(place.join("b.txt"), "two\n").unwrap();
    let head = finish(&git, "R1-1").unwrap();
    assert_ne!(head, git["base"].as_str().unwrap());
    let changed = changes(&git, &head).unwrap();
    assert_eq!(changed["files"], json!([{"path":"b.txt","added":1,"deleted":0}]));
    assert_eq!((changed["totals"]["files"].as_u64(), changed["source"].as_str()), (Some(1), Some("git")));
    assert!(!repository.join("b.txt").exists());
    fs::write(repository.join("notes.txt"), "planner notes\n").unwrap();
    let merged = merge(&git, "R1-1 Add b").unwrap();
    assert!(repository.join("b.txt").exists() && !place.exists() && !branch_exists(&repository, "task/R1-1"));
    assert_eq!(run(&repository, &["rev-parse", "HEAD"]).unwrap(), merged);
    let subjects = run(&repository, &["log", "--format=%s"]).unwrap();
    assert!(subjects.starts_with("R1-1 Add b\nUncommitted changes before merging task/R1-1\n"), "{subjects}");
    assert!(run(&repository, &["status", "--porcelain"]).unwrap().is_empty());
}

#[test]
fn children_start_from_their_parent_branch_and_only_implementation_creates_worktrees() {
    let temp = Temp::new();
    let (workspace, repository) = repository(&temp);
    assert!(prepare(&workspace, &repository, "R1-1", &[], "review").unwrap().is_none());
    let parent = prepare(&workspace, &repository, "R1-1", &[], "implementation").unwrap().unwrap();
    fs::write(worktree(&parent).join("partial.txt"), "half\n").unwrap();
    finish(&parent, "R1-1").unwrap();
    let child = prepare(&workspace, &repository, "R1-2", &["R1-1".into()], "implementation").unwrap().unwrap();
    assert!(worktree(&child).join("partial.txt").exists());
    let fresh = prepare(&workspace, &repository, "R1-3", &["R1-9".into()], "implementation").unwrap().unwrap();
    assert!(!worktree(&fresh).join("partial.txt").exists());
    assert_eq!(prepare(&workspace, &repository, "R1-2", &[], "review").unwrap().unwrap()["worktree"], child["worktree"]);
    assert!(prepare(&workspace, &temp.dir("workspace/plain"), "R1-4", &[], "implementation").unwrap().is_none());
    let outer = temp.dir("outer");
    run(&outer, &["init", "-q"]).unwrap();
    assert!(prepare(&temp.dir("outer/ws"), &temp.dir("outer/ws/game"), "R2-1", &[], "implementation").unwrap().is_none());
}

#[test]
fn a_conflicting_branch_is_not_merged_and_the_next_attempt_sees_the_conflict() {
    let temp = Temp::new();
    let (workspace, repository) = repository(&temp);
    let git = prepare(&workspace, &repository, "R1-1", &[], "implementation").unwrap().unwrap();
    fs::write(worktree(&git).join("a.txt"), "task\n").unwrap();
    finish(&git, "R1-1").unwrap();
    fs::write(repository.join("a.txt"), "main\n").unwrap();
    run(&repository, &["commit", "-q", "-am", "Main moved"]).unwrap();
    assert!(merge(&git, "R1-1 Change a").is_err());
    assert_eq!(fs::read_to_string(repository.join("a.txt")).unwrap().trim(), "main");
    assert!(run(&repository, &["status", "--porcelain"]).unwrap().is_empty());
    let retry = prepare(&workspace, &repository, "R1-1", &[], "implementation").unwrap().unwrap();
    assert_eq!(retry["conflict"], true);
    assert!(fs::read_to_string(worktree(&retry).join("a.txt")).unwrap().contains("<<<<<<<"));
}

#[test]
fn review_sees_main_merged_in_and_a_conflict_leaves_the_branch_untouched() {
    let temp = Temp::new();
    let (workspace, repository) = repository(&temp);
    let git = prepare(&workspace, &repository, "R1-1", &[], "implementation").unwrap().unwrap();
    fs::write(worktree(&git).join("b.txt"), "task\n").unwrap();
    finish(&git, "R1-1").unwrap();
    fs::write(repository.join("c.txt"), "other task\n").unwrap();
    run(&repository, &["add", "-A"]).unwrap();
    run(&repository, &["commit", "-q", "-m", "Other task merged"]).unwrap();
    let main_head = run(&repository, &["rev-parse", "HEAD"]).unwrap();
    let review = prepare(&workspace, &repository, "R1-1", &[], "review").unwrap().unwrap();
    assert_eq!((review["conflict"].as_bool(), review["base"].as_str()), (Some(false), Some(main_head.as_str())));
    assert!(worktree(&review).join("c.txt").exists() && worktree(&review).join("b.txt").exists());
    let head = finish(&review, "R1-1").unwrap();
    assert_eq!(changes(&review, &head).unwrap()["files"], json!([{"path":"b.txt","added":1,"deleted":0}]));
    fs::write(worktree(&review).join("a.txt"), "task\n").unwrap();
    finish(&review, "R1-1").unwrap();
    fs::write(repository.join("a.txt"), "main\n").unwrap();
    run(&repository, &["commit", "-q", "-am", "Main moved"]).unwrap();
    let before = run(&worktree(&review), &["rev-parse", "HEAD"]).unwrap();
    let conflicted = prepare(&workspace, &repository, "R1-1", &[], "review").unwrap().unwrap();
    assert_eq!(conflicted["conflict"], true);
    assert_eq!(run(&worktree(&review), &["rev-parse", "HEAD"]).unwrap(), before);
    assert!(run(&worktree(&review), &["status", "--porcelain"]).unwrap().is_empty());
}
