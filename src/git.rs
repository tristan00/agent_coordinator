use crate::{Result, require};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn shown(path: &Path) -> String {
    path.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/")
}
pub fn run(directory: &Path, args: &[&str]) -> Result<String> {
    let output = crate::process::command("git").args(["-c", "user.name=coordinator", "-c", "user.email=coordinator@localhost"]).args(args).current_dir(directory).output()?;
    require(output.status.success(), format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
pub fn relative(path: &Path, root: &Path) -> Option<PathBuf> {
    let (full, root) = (shown(path), shown(root));
    let (full, root) = (full.trim_end_matches('/'), root.trim_end_matches('/'));
    let rest = full.get(root.len()..).filter(|_| full.get(..root.len()).is_some_and(|start| start.eq_ignore_ascii_case(root)))?;
    (rest.is_empty() || rest.starts_with('/')).then(|| PathBuf::from(rest.trim_start_matches('/')))
}
fn repository(cwd: &Path, workspace: &Path) -> Option<PathBuf> {
    let top = PathBuf::from(run(cwd, &["rev-parse", "--show-toplevel"]).ok()?);
    (relative(&top, workspace).is_some_and(|r| !r.as_os_str().is_empty())).then_some(top)
}
fn branch_exists(repository: &Path, branch: &str) -> bool {
    run(repository, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok()
}
pub fn branch(name: &str) -> String {
    format!("task/{name}")
}
fn dirty(directory: &Path) -> Result<bool> {
    Ok(!run(directory, &["status", "--porcelain"])?.is_empty())
}
fn commit_all(directory: &Path, message: &str) -> Result<()> {
    if dirty(directory)? {
        run(directory, &["add", "-A"])?;
        run(directory, &["commit", "-q", "-m", message])?;
    }
    Ok(())
}
pub fn prepare(workspace: &Path, cwd: &Path, name: &str, ancestors: &[String], stage: &str) -> Result<Option<Value>> {
    let Some(repository) = repository(cwd, workspace) else {
        return Ok(None);
    };
    let main = run(&repository, &["symbolic-ref", "--short", "HEAD"])?;
    let branch = branch(name);
    let worktree = PathBuf::from(shown(workspace)).join("worktrees").join(name);
    if !worktree.join(".git").exists() {
        if !matches!(stage, "implementation" | "merge") {
            return Ok(None);
        }
        run(&repository, &["worktree", "prune"])?;
        let place = shown(&worktree);
        if branch_exists(&repository, &branch) {
            run(&repository, &["worktree", "add", &place, &branch])?;
        } else {
            let base = ancestors.iter().map(|a| self::branch(a)).find(|b| branch_exists(&repository, b)).unwrap_or_else(|| main.clone());
            run(&repository, &["worktree", "add", "-b", &branch, &place, &base])?;
        }
    }
    let conflict = match stage {
        "implementation" | "merge" => run(&worktree, &["merge", "--no-edit", &main]).is_err(),
        "review" => {
            let failed = run(&worktree, &["merge", "--no-edit", &main]).is_err();
            if failed {
                let _ = run(&worktree, &["merge", "--abort"]);
            }
            failed
        }
        _ => false,
    };
    let base = if stage == "review" && !conflict { run(&worktree, &["merge-base", &main, "HEAD"])? } else { run(&worktree, &["rev-parse", "HEAD"])? };
    let inside = relative(cwd, &repository).unwrap_or_default();
    Ok(Some(json!({"repository":shown(&repository),"main":main,"branch":branch,"worktree":shown(&worktree),"cwd":shown(&worktree.join(inside)),"base":base,"conflict":conflict})))
}
pub fn map(path: &Path, git: &Value) -> PathBuf {
    let (Some(repository), Some(worktree)) = (git["repository"].as_str(), git["worktree"].as_str()) else {
        return path.to_path_buf();
    };
    relative(path, Path::new(repository)).map_or_else(|| path.to_path_buf(), |inside| PathBuf::from(shown(&Path::new(worktree).join(inside))))
}
pub fn finish(git: &Value, name: &str) -> Result<String> {
    let worktree = Path::new(git["worktree"].as_str().ok_or("Missing worktree")?);
    commit_all(worktree, &format!("{name} uncommitted work"))?;
    run(worktree, &["rev-parse", "HEAD"])
}
pub fn changes(git: &Value, head: &str) -> Result<Value> {
    let worktree = Path::new(git["worktree"].as_str().ok_or("Missing worktree")?);
    let base = git["base"].as_str().ok_or("Missing base")?;
    let listed = run(worktree, &["diff", "--numstat", base, head])?;
    let files: Vec<Value> = listed.lines().filter_map(|line| {
        let mut parts = line.splitn(3, '\t');
        let (added, deleted, path) = (parts.next()?, parts.next()?, parts.next()?);
        Some(json!({"path":path,"added":added.parse::<u64>().ok(),"deleted":deleted.parse::<u64>().ok()}))
    }).collect();
    let sum = |key: &str| files.iter().filter_map(|f| f[key].as_u64()).sum::<u64>();
    Ok(json!({"source":"git","base":base,"head":head,"files":files,"totals":{"files":files.len(),"added":sum("added"),"deleted":sum("deleted")}}))
}
pub fn merge(git: &Value, message: &str) -> Result<String> {
    let (repository, worktree) = (Path::new(git["repository"].as_str().ok_or("Missing repository")?), git["worktree"].as_str().ok_or("Missing worktree")?);
    let branch = git["branch"].as_str().ok_or("Missing branch")?;
    commit_all(repository, &format!("Uncommitted changes before merging {branch}"))?;
    if let Err(error) = run(repository, &["merge", "--no-ff", "-m", message, branch]) {
        let _ = run(repository, &["merge", "--abort"]);
        return Err(error);
    }
    let head = run(repository, &["rev-parse", "HEAD"])?;
    discard(repository, Path::new(worktree), branch);
    Ok(head)
}
fn discard(repository: &Path, worktree: &Path, branch: &str) {
    let (repository, worktree, branch) = (repository.to_path_buf(), worktree.to_path_buf(), branch.to_owned());
    let trash = PathBuf::from(format!("{}.removed-{}", shown(&worktree), crate::now()));
    if std::fs::rename(&worktree, &trash).is_ok() {
        let _ = run(&repository, &["worktree", "prune"]);
        let _ = run(&repository, &["branch", "-D", &branch]);
        if let Some(parent) = worktree.parent().map(Path::to_path_buf) {
            std::thread::spawn(move || sweep(&parent));
        }
    } else {
        std::thread::spawn(move || {
            let _ = run(&repository, &["worktree", "remove", "--force", &shown(&worktree)]);
            let _ = run(&repository, &["branch", "-D", &branch]);
        });
    }
}
pub fn sweep(worktrees: &Path) {
    for entry in std::fs::read_dir(worktrees).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().contains(".removed-") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
