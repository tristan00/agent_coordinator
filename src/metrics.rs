use crate::history::{Event, METRIC_DEFINITION_VERSION};
use crate::{engine::Task, now};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
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
    [("Waiting for writable scope ", "resource"), ("Waiting for ", "dependency"), ("Retry scheduled at ", "retry")].into_iter().find_map(|(prefix, bucket)| reason.starts_with(prefix).then_some(bucket)).unwrap_or("queue")
}
#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    at: u64,
    files: BTreeMap<PathBuf, Fingerprint>,
    skipped: BTreeSet<PathBuf>,
    #[serde(default)]
    excluded: BTreeSet<PathBuf>,
    #[serde(default)]
    elapsed_ms: u64,
    #[serde(default)]
    roots: Vec<PathBuf>,
}
pub fn clean(directory: &Path) {
    for name in ["baseline.json", "final-snapshot.json"] {
        let _ = fs::remove_file(directory.join(name));
    }
}
#[derive(Serialize, Deserialize)]
struct Fingerprint {
    bytes: u64,
    lines: Option<Vec<u64>>,
    trimmed: Option<Vec<u64>>,
}
pub fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3))
}
pub fn capture(roots: &[PathBuf], data: &Path) -> Snapshot {
    let clock = std::time::Instant::now();
    let mut snapshot = Snapshot { at: now(), files: BTreeMap::new(), skipped: BTreeSet::new(), excluded: BTreeSet::new(), elapsed_ms: 0, roots: roots.to_vec() };
    let mut pending = roots.to_vec();
    let mut visited = BTreeSet::new();
    while let Some(path) = pending.pop() {
        if !visited.insert(path.clone()) {
            continue;
        }
        if crate::engine::covers(data, &path)
            || path.file_name().is_some_and(|name| EXCLUDED.iter().any(|excluded| name.eq_ignore_ascii_case(excluded)) || name.to_string_lossy().to_ascii_lowercase().starts_with("runtime-data-"))
        {
            snapshot.excluded.insert(path);
            continue;
        }
        let observed = (|| -> std::io::Result<()> {
            let meta = match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                other => other?,
            };
            if meta.file_attributes() & 0x400 != 0 {
                snapshot.skipped.insert(path.clone());
            } else if meta.is_dir() {
                for entry in fs::read_dir(&path)? {
                    match entry {
                        Ok(entry) => pending.push(entry.path()),
                        Err(_) => {
                            snapshot.skipped.insert(path.clone());
                        }
                    }
                }
            } else if meta.is_file() {
                let bytes = fs::read(&path)?;
                let text = std::str::from_utf8(&bytes).ok().filter(|s| !s.contains('\0'));
                let hash_lines = |trim| text.map(|text| text.lines().map(|line| fingerprint(if trim { line.trim() } else { line }.as_bytes())).collect());
                snapshot.files.insert(path.clone(), Fingerprint { bytes: fingerprint(&bytes), lines: hash_lines(false), trimmed: hash_lines(true) });
            }
            Ok(())
        })();
        if observed.is_err() {
            snapshot.skipped.insert(path);
        }
    }
    snapshot.elapsed_ms = clock.elapsed().as_millis() as u64;
    snapshot
}
fn common_lines(left: &[u64], right: &[u64]) -> usize {
    let prefix = left.iter().zip(right).take_while(|(a, b)| a == b).count();
    let (left, right) = (&left[prefix..], &right[prefix..]);
    let suffix = left.iter().rev().zip(right.iter().rev()).take_while(|(a, b)| a == b).count();
    let (left, right) = (&left[..left.len() - suffix], &right[..right.len() - suffix]);
    let (left, right) = if left.len() < right.len() { (right, left) } else { (left, right) };
    let mut row = vec![0; right.len() + 1];
    for a in left {
        let mut previous = 0;
        for (column, b) in right.iter().enumerate() {
            let above = row[column + 1];
            row[column + 1] = if a == b { previous + 1 } else { above.max(row[column]) };
            previous = above;
        }
    }
    prefix + suffix + row[right.len()]
}
fn category(path: &Path) -> &'static str {
    let parts: Vec<_> = path.iter().map(|part| part.to_string_lossy().to_ascii_lowercase()).collect();
    let has = |names: &[&str]| parts.iter().any(|part| names.contains(&part.as_str()));
    let extension = |names: &[&str]| path.extension().is_some_and(|ext| names.iter().any(|name| ext.eq_ignore_ascii_case(name)));
    let test_name = parts.last().is_some_and(|name| name.ends_with("_tests.rs") || name.ends_with("_test.rs") || name.contains(".test.") || name.contains(".spec."));
    [
        ("generated", has(&["target", "dist", "build", "node_modules", "generated", "coverage", "vendor"])),
        ("test", has(&["tests", "test", "__tests__"]) || test_name),
        ("ui", has(&["web", "ui", "frontend"]) || extension(&["html", "css", "tsx", "jsx"])),
        ("source", has(&["src", "lib", "app"]) || extension(&["rs", "py", "js", "ts", "go", "java", "cs", "c", "cpp", "h"])),
    ]
    .into_iter()
    .find_map(|(category, matches)| matches.then_some(category))
    .unwrap_or("other")
}
pub fn compare(before: Snapshot, after: Snapshot, base: &Path, peers: &[String]) -> Value {
    let clock = std::time::Instant::now();
    let mut files = Vec::new();
    let skipped: BTreeSet<_> = before.skipped.union(&after.skipped).collect();
    let paths: BTreeSet<_> = before.files.keys().chain(after.files.keys()).collect();
    for path in paths {
        if skipped.iter().any(|root| crate::engine::covers(root, path)) {
            continue;
        }
        let old = before.files.get(path);
        let new = after.files.get(path);
        if old.zip(new).is_some_and(|(a, b)| a.bytes == b.bytes) {
            continue;
        }
        let counts = lines(old).zip(lines(new)).map(|(old, new)| {
            let common = common_lines(old, new);
            (new.len() - common, old.len() - common)
        });
        let whitespace_only = old
            .zip(new)
            .and_then(|(old, new)| old.trimmed.as_ref().zip(new.trimmed.as_ref()))
            .map(|(old, new)| old.iter().filter(|line| **line != fingerprint(b"")).eq(new.iter().filter(|line| **line != fingerprint(b""))));
        files.push(json!({"path":path,"category":category(path.strip_prefix(base).unwrap_or(path)),
            "beforeFingerprint":old.map(|f| format!("fnv1a64:{:016x}",f.bytes)),
            "afterFingerprint":new.map(|f| format!("fnv1a64:{:016x}",f.bytes)),
            "added":counts.map(|c| c.0),"deleted":counts.map(|c| c.1),"whitespaceOnly":whitespace_only}));
    }
    json!({"definitionVersion":METRIC_DEFINITION_VERSION,"before":before.at,"after":after.at,"files":files,
        "comparisonOverheadMs":clock.elapsed().as_millis(),"totals":footprint(files.iter()),"scopeSize":{"before":size(&before, base),"after":size(&after, base)},"skipped":skipped,"overlappingAttempts":peers,
        "attribution":if peers.is_empty() {"scoped_observation"} else {"ambiguous"},
        "limitation":"Endpoint observations cannot identify external edits or changes reverted within an attempt"})
}
fn lines(file: Option<&Fingerprint>) -> Option<&[u64]> {
    file.map_or(Some(&[]), |file| file.lines.as_deref())
}
fn footprint<'a>(files: impl Iterator<Item = &'a Value> + Clone) -> Value {
    let paths: BTreeSet<_> = files.clone().filter_map(|file| file["path"].as_str()).collect();
    let total = files.clone().count();
    let counts = files.filter_map(|file| file["added"].as_u64().zip(file["deleted"].as_u64()));
    let known = counts.clone().count();
    let (added, deleted) = counts.fold((0_u128, 0_u128), |(a, d), (add, del)| (a + u128::from(add), d + u128::from(del)));
    json!({"uniqueFiles":paths.len(),"added":added,"deleted":deleted,"net":added as i128 - deleted as i128,
        "churn":added + deleted,"unknownLineCounts":total-known})
}

const EXCLUDED: &[&str] = &[".git", "target", "node_modules", "dist", "build", "coverage", "vendor", "generated", ".venv", "__pycache__", "data"];
pub fn size(snapshot: &Snapshot, workspace: &Path) -> Value {
    let categories: BTreeMap<_, _> = ["source", "ui", "test", "generated", "other"]
        .into_iter()
        .map(|name| {
            let files: Vec<_> = snapshot.files.iter().filter(|(path, _)| category(path.strip_prefix(workspace).unwrap_or(path)) == name).collect();
            let known: Vec<_> = files.iter().filter_map(|(_, file)| file.lines.as_ref().zip(file.trimmed.as_ref())).collect();
            (
                name,
                json!({"files":files.len(),"textFiles":known.len(),"binaryOrNonUtf8Files":files.len()-known.len(),
            "physicalLines":known.iter().map(|(lines,_)| lines.len()).sum::<usize>(),
            "nonblankLines":known.iter().map(|(_,lines)| lines.iter().filter(|&&line| line != fingerprint(b"")).count()).sum::<usize>()}),
            )
        })
        .collect();
    let fingerprints: BTreeMap<_, _> = snapshot.files.iter().map(|(path, file)| (path, format!("fnv1a64:{:016x}", file.bytes))).collect();
    json!({"definitionVersion":METRIC_DEFINITION_VERSION,"rules":{"version":1,"workspace":workspace,"include":"all regular files beneath workspace, including unchanged files", "excludeDirectoryNames":EXCLUDED,"excludeNamePrefixes":["runtime-data-"],"excludeRuntimeData":true,"reparsePoints":"unmeasured; never followed","encoding":"UTF-8 without NUL; Rust str.lines physical lines; trim-empty lines excluded from nonblank","categoryPrecedence":["generated","test","ui","source","other"]},
        "scope":snapshot.roots,"startedAt":snapshot.at,"finishedAt":snapshot.at.saturating_add(snapshot.elapsed_ms),"overheadMs":snapshot.elapsed_ms,
        "byCategory":categories,"fingerprints":fingerprints,"skipped":snapshot.skipped,"excluded":snapshot.excluded,
        "status":if snapshot.skipped.is_empty() && snapshot.excluded.is_empty() {"measured"} else {"partial"},
        "missingness":"Excluded trees have unknown file and line counts, not zero. Binary files count as files; their line counts are unknown. Snapshot is not atomic."})
}
pub fn project_capture(directory: &Path, request: &Value, final_capture: bool) -> crate::Result<()> {
    let base = Path::new(request["task"]["workspaceId"].as_str().or_else(|| request["task"]["cwd"].as_str()).ok_or("Missing workspace")?);
    crate::require(base.is_dir(), "Workspace unavailable for project measurement")?;
    let data = directory.parent().and_then(Path::parent).ok_or("Missing data")?;
    let name = if final_capture { "project-after.json" } else { "project-before.json" };
    if !directory.join(name).exists() {
        crate::write(&directory.join(name), &size(&capture(&[base.to_path_buf()], data), base))?;
    }
    Ok(())
}

pub fn size_delta(before: &Value, after: &Value) -> Value {
    if before["rules"] != after["rules"] || before.is_null() || after.is_null() {
        return Value::Null;
    }
    let categories: BTreeMap<_, _> = ["source", "ui", "test", "generated", "other"]
        .into_iter()
        .map(|category| {
            let values: BTreeMap<_, _> = ["files", "physicalLines", "nonblankLines"]
                .into_iter()
                .map(|field| {
                    let value = before["byCategory"][category][field].as_i64().zip(after["byCategory"][category][field].as_i64()).map(|(a, b)| b - a);
                    (field, value)
                })
                .collect();
            (category, values)
        })
        .collect();
    json!({"includedCategories":categories,"status":if before["status"] == "measured" && after["status"] == "measured" {"measured"} else {"partial"},"attribution":"endpoint_project_delta_not_task_change"})
}

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod tests;
