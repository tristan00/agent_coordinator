use crate::agents::{Agent, Harness};
use crate::engine::{Engine, Task, id, workspace};
use crate::process::{Job, command};
use crate::{Result, lock, now, read, require, write};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::Duration;

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub workspace: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    pub status: String,
    pub requests: Vec<Request>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    #[serde(default, alias = "decisions", skip_serializing_if = "BTreeSet::is_empty")]
    pub notified: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_error: Option<String>,
    pub created: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evaluations: Vec<Evaluation>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Submitted {
    pub task: String,
    pub revision: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Evaluation {
    pub run: PathBuf,
    pub started: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<u64>,
    pub submitted: Vec<Submitted>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verdicts: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub kind: String,
    pub text: String,
    pub run: PathBuf,
    pub started: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_ended: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<Value>,
}

fn session<'a>(engine: &'a Engine, id: &str) -> Result<&'a Session> {
    engine.state.sessions.get(id).ok_or_else(|| format!("Unknown session {id}").into())
}

pub fn can_still_run(task: &Task) -> bool {
    !task.container() && (task.active.is_some() || !(task.stage == "done" || task.stage == "rejected" || task.stopped || task.blocked))
}

pub fn needs_decision(task: &Task) -> bool {
    !task.container() && task.blocked && task.active.is_none() && !task.stopped && task.stage != "done"
}

fn decision_key(task: &Task) -> String {
    format!("{}:{}", task.id, task.attempts.len())
}

fn finished_key(task: &Task, actor: &str) -> Option<String> {
    let created = task.history.iter().any(|e| e.kind == "created" && e.actor.as_deref() == Some(actor));
    let times = task.history.iter().filter(|e| e.kind == "phase_changed" && e.evidence["to"] == "done").count();
    (created && task.stage == "done" && times > 0).then(|| format!("done:{}:{times}", task.id))
}

fn label(task: &Task) -> String {
    format!("{} {}", task.short_id.as_deref().unwrap_or(&task.id), task.title)
}

fn hours(seconds: f64) -> String {
    format!("{:.1} CPU-hours", seconds / 3600.0)
}

fn board(scoped: &[&Task], since: u64, live: &dyn Fn(&Task) -> Option<f64>) -> String {
    let now = now();
    let work: Vec<&&Task> = scoped.iter().filter(|t| !t.container()).collect();
    let counts: Vec<String> = [("Proposed", "proposed"), ("To do", "to do"), ("In progress", "in progress"), ("Review", "in review"), ("Blocked", "blocked"), ("Stopped", "stopped"), ("Rejected", "rejected"), ("Done", "done")]
        .into_iter()
        .map(|(status, words)| format!("{} {words}", work.iter().filter(|t| t.status() == status).count()))
        .collect();
    let mut lines = vec![format!("Board: {}.", counts.join(", "))];
    let running: Vec<String> = work
        .iter()
        .filter_map(|t| {
            let attempt = t.attempts.iter().rev().find(|a| Some(&a.id) == t.active.as_ref())?;
            let cpu = live(t).map(|c| format!(", {}", hours(c))).unwrap_or_default();
            Some(format!("{} ({}, {} min{cpu})", label(t), attempt.stage, now.saturating_sub(attempt.started) / 60_000))
        })
        .collect();
    if !running.is_empty() {
        lines.push(format!("Running now: {}.", running.join("; ")));
    }
    let rejected: Vec<String> = work
        .iter()
        .flat_map(|t| t.attempts.iter().filter(move |a| a.stage == "review" && a.status() == Some("rejected") && a.ended.is_some_and(|e| e > since)).map(move |a| (t, a)))
        .map(|(t, a)| format!("{}: {}", label(t), a.result.as_ref().and_then(|r| r["summary"].as_str()).unwrap_or("no summary")))
        .collect();
    if !rejected.is_empty() {
        lines.push(format!("Review rejections since your last turn: {}.", rejected.join("; ")));
    }
    let costs: Vec<Option<f64>> = work.iter().flat_map(|t| t.attempts.iter().filter(|a| a.ended.is_some())).map(|a| a.evidence.cost.as_ref().and_then(|c| c["usd"].as_f64())).collect();
    let unreported = costs.iter().filter(|c| c.is_none()).count();
    let spent: f64 = costs.iter().flatten().sum();
    lines.push(format!("Worker cost so far: ${spent:.2}{}.", if unreported > 0 { format!(" ({unreported} attempts did not report a cost)") } else { String::new() }));
    let cpu: f64 = work.iter().flat_map(|t| t.attempts.iter().filter(|a| a.ended.is_some())).filter_map(|a| a.evidence.cpu_seconds).sum::<f64>() + work.iter().filter_map(|t| live(t)).sum::<f64>();
    lines.push(format!("Worker compute so far: {}.", hours(cpu)));
    lines.join("\n")
}

fn evaluation_findings(task: &Task) -> String {
    task.plan_reviews.last().map(|r| r.findings.join(" ")).unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn wake_prompt(finished: &[&Task], blocked: &[&Task], heavy: &[(&Task, f64)], reviewed: &[&Task], attempts: u64, board: String, idle: bool, minutes: u64) -> String {
    let line = |t: &Task, detail: &str| format!("- {} (GET $COORDINATOR_URL/api/tasks/{}): {detail}", label(t), t.id);
    let result = |t: &Task| t.attempts.iter().rev().find(|a| a.stage == "review" && a.status() == Some("accepted")).and_then(|a| a.result.as_ref()?["summary"].as_str().map(str::to_owned)).unwrap_or_else(|| "accepted".into());
    let mut parts = Vec::new();
    if !reviewed.is_empty() {
        let list: Vec<String> = reviewed.iter().map(|t| line(t, &format!("{} ({} of {attempts} submissions left)", evaluation_findings(t), attempts.saturating_sub(t.kickbacks() as u64)))).collect();
        parts.push(format!("The plan evaluator kicked these proposed tasks back:
{}

Revise each so it resolves every finding, or delete it. A task sent back unchanged uses another submission.", list.join("
")));
    }
    if !finished.is_empty() {
        let list: Vec<String> = finished.iter().map(|t| line(t, &result(t))).collect();
        parts.push(format!("These tasks finished since your last turn:\n{}\n\nCompare what they produced with your target and change, add or remove tasks where the plan no longer leads to it.", list.join("\n")));
    } else if blocked.is_empty() && heavy.is_empty() && reviewed.is_empty() {
        parts.push("Nothing has finished since your last turn. Check that the work in progress still leads to your target.".to_owned());
    }
    if !blocked.is_empty() {
        let list: Vec<String> = blocked.iter().map(|t| line(t, &t.failure)).collect();
        parts.push(format!("These tasks are blocked and need your decision:\n{}\n\nFor each one, correct it, ask for a replan, resume it, replace it with new tasks or delete it.", list.join("\n")));
    }
    if !heavy.is_empty() {
        let list: Vec<String> = heavy.iter().map(|(t, c)| line(t, &format!("{} so far", hours(*c)))).collect();
        parts.push(format!("These tasks are using a lot of compute:\n{}\n\nDecide whether that work is worth it; you can stop, change or replan it.", list.join("\n")));
    }
    parts.push(board);
    parts.push(if idle {
        "No task can still run, so the request finishes when you end this turn unless you add or resume tasks.".to_owned()
    } else {
        format!("End your turn when you are done. The coordinator resumes you when a task needs your decision, every {minutes} minutes while work runs, and when nothing more can run.")
    });
    parts.join("\n\n")
}

pub fn create(engine: &mut Engine, input: Value) -> Result<Value> {
    let source = workspace(Path::new(input["workspace"].as_str().ok_or("workspace is required")?))?;
    let settings = engine.state.config.workspaces.get(&source).cloned().ok_or_else(|| format!("{} is not a configured workspace", source.display()))?;
    let text = input["request"].as_str().filter(|text| !text.trim().is_empty()).ok_or("request text is required")?.to_owned();
    settings.agent("planner", engine.state.family)?;
    let path = PathBuf::from(input["directory"].as_str().ok_or("directory is required")?);
    require(path.is_absolute(), "directory must be absolute")?;
    require(path.ancestors().find(|a| a.exists()).is_some_and(|a| a.canonicalize().is_ok_and(|a| a.starts_with(&source))), "directory must be inside the workspace")?;
    fs::create_dir_all(&path)?;
    let directory = workspace(&path)?;
    require(directory != source, "directory must be a folder inside the workspace, not the workspace itself")?;
    require(!engine.state.sessions.values().any(|s| s.workspace == directory && s.status == "running"), "A session is already running in this directory")?;
    repository(&directory)?;
    let id = id();
    let session = Session { id: id.clone(), workspace: directory.clone(), root: Some(source.clone()), status: "running".into(), requests: Vec::new(), conversation: None, notified: BTreeSet::new(), wake_error: None, created: now(), evaluations: Vec::new() };
    engine.change(|state| {
        state.sessions.insert(id.clone(), session);
        Ok(Value::Null)
    })?;
    start(engine, &id, text, "initial")?;
    view(engine, &id)
}

fn repository(directory: &Path) -> Result<()> {
    if directory.join(".git").exists() {
        return Ok(());
    }
    let steps: [&[&str]; 3] = [&["init", "-q"], &["add", "-A"], &["-c", "user.name=coordinator", "-c", "user.email=coordinator@localhost", "commit", "-q", "--allow-empty", "-m", "Baseline"]];
    for args in steps {
        let output = command("git").args(args).current_dir(directory).output()?;
        require(output.status.success(), format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()))?;
    }
    Ok(())
}

pub fn message(engine: &mut Engine, id: &str, input: Value) -> Result<Value> {
    let text = input["request"].as_str().filter(|text| !text.trim().is_empty()).ok_or("request text is required")?.to_owned();
    require(session(engine, id)?.status == "finished", "The session is still working on its current request")?;
    start(engine, id, text, "follow-up")?;
    view(engine, id)
}

fn clean(path: &Path) -> String {
    path.to_string_lossy().trim_start_matches(r"\\?\").to_owned()
}

fn prompt(session: &Session, text: &str, first: bool, instructions: &str, minutes: u64, attempts: Option<u64>) -> String {
    if !first {
        return format!("New request in the same folder:\n\n{text}");
    }
    let w = clean(&session.workspace);
    let root = clean(session.root.as_deref().unwrap_or(&session.workspace));
    let evaluation = attempts.map(|n| format!(" Every task you create or change starts as proposed. A plan evaluator checks proposed tasks against the request before workers can take them and resumes you with its findings; a task it kicks back {n} times is rejected and never runs. Proposed tasks are evaluated only once every task they depend on has been accepted.")).unwrap_or_default();
    format!(
        "You plan the request in {w}, inside the workspace {root}. Create tasks through the coordinator at $COORDINATOR_URL; AGENTS.md lists its endpoints. When your tasks are in place, end your turn.{evaluation} The coordinator brings you results every {minutes} minutes, decisions as they come up, and a final check when nothing more can run.\n\nPlanner instructions:\n{instructions}\n\nRequest:\n{text}"
    )
}

fn start(engine: &mut Engine, id: &str, text: String, kind: &str) -> Result<()> {
    let session = session(engine, id)?.clone();
    let settings = engine.state.settings(Some(&session.workspace));
    let agent = settings.agent("planner", engine.state.family)?;
    let instructions = crate::workspace_prompts::planner(&crate::workspace_prompts::root(), &settings)?;
    let run = engine.directory.join("sessions").join(id).join((session.requests.len() + 1).to_string());
    fs::create_dir_all(&run)?;
    let port: u16 = read(&engine.directory.join("port.json")).unwrap_or(4242);
    let attempts = settings.evaluator.is_some().then(|| settings.plan_attempts());
    let text_prompt = if kind == "wake" { text.clone() } else { prompt(&session, &text, session.requests.is_empty(), &instructions, settings.wake_minutes(), attempts) };
    write(&run.join("request.json"), &json!({"session":id,"workspace":session.workspace,"root":session.root.as_ref().unwrap_or(&session.workspace),"prompt":text_prompt,"agent":agent,
        "conversation":session.conversation,"command":engine.state.config.planner,"url":format!("http://127.0.0.1:{port}/s/{id}")}))?;
    write(&run.join("launch-intent.json"), &json!({"at":now()}))?;
    let request = Request { kind: kind.into(), text, run: run.clone(), started: now(), planner_ended: None, exit_code: None, usage: None, ended: None, error: None, rejected: Vec::new() };
    engine.change(|state| {
        let s = state.sessions.get_mut(id).ok_or("Unknown session")?;
        s.status = "running".into();
        s.requests.push(request);
        Ok(Value::Null)
    })?;
    command(std::env::current_exe()?.to_str().ok_or("Invalid executable path")?).arg("--planner").arg(&run).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    Ok(())
}

pub fn stop(engine: &mut Engine, id: &str) -> Result<Value> {
    let session = session(engine, id)?.clone();
    if let Some(request) = session.requests.last().filter(|r| r.planner_ended.is_none()) {
        fs::write(request.run.join("stop"), b"")?;
    }
    if let Some(evaluation) = session.evaluations.last().filter(|e| e.ended.is_none()) {
        fs::write(evaluation.run.join("stop"), b"")?;
    }
    let tasks: Vec<String> = engine.state.tasks.iter().filter(|t| engine.state.in_scope(t, Some(&session.workspace)) && can_still_run(t) && !t.stopped).map(|t| t.id.clone()).collect();
    for task in tasks {
        engine.control(true, Some(&task))?;
    }
    engine.change(|state| {
        let s = state.sessions.get_mut(id).ok_or("Unknown session")?;
        s.status = "stopped".into();
        if let Some(request) = s.requests.last_mut() {
            request.ended.get_or_insert(now());
        }
        Ok(Value::Null)
    })?;
    view(engine, id)
}

pub fn view(engine: &Engine, id: &str) -> Result<Value> {
    let s = session(engine, id)?;
    let tasks: Vec<&str> = engine.state.tasks.iter().filter(|t| engine.state.in_scope(t, Some(&s.workspace))).map(|t| t.id.as_str()).collect();
    let mut value = serde_json::to_value(s)?;
    value["tasks"] = json!(tasks);
    value["agent"] = json!(engine.state.settings(Some(&s.workspace)).agent("planner", engine.state.family).ok());
    let actor = format!("planner:{id}");
    let actions: Vec<Value> = engine.state.tasks.iter().chain(&engine.state.archived).flat_map(|t| t.history.iter().filter(|e| e.actor.as_deref() == Some(actor.as_str())).filter_map(move |e| {
        let kind = match e.kind.as_str() {
            "created" => "created",
            "updated" | "revised" => "edited",
            "replan_requested" => "replan",
            "stopped" | "resumed" => e.kind.as_str(),
            "archived" | "abandoned" => "deleted",
            _ => return None,
        };
        Some(json!({"at":e.at,"kind":kind,"task":t.id,"shortId":t.short_id,"title":t.title}))
    })).collect();
    value["actions"] = json!(actions);
    Ok(value)
}

pub fn list(engine: &Engine) -> Result<Value> {
    Ok(json!(engine.state.sessions.keys().map(|id| view(engine, id)).collect::<Result<Vec<_>>>()?))
}

fn exited(run: &Path, since: u64) -> Option<Value> {
    if let Ok(exit) = read::<Value>(&run.join("exit.json")) {
        return Some(exit);
    }
    let started = run.join("run.json").exists();
    let stale = now().saturating_sub(since) > 60_000;
    (lock(&run.join("lock")).is_ok() && (started || stale)).then(|| json!({"code":-1,"error":"Planner process ended without recording an exit"}))
}

fn planner_finished(request: &Request) -> Option<Value> {
    exited(&request.run, request.started)
}

const VERDICTS: &str = r#"{"type":"object","properties":{"verdicts":{"type":"array","items":{"type":"object","properties":{"task":{"type":"string"},"accept":{"type":"boolean"},"findings":{"type":"array","items":{"type":"string"}}},"required":["task","accept","findings"],"additionalProperties":false}}},"required":["verdicts"],"additionalProperties":false}"#;

fn proposal(engine: &Engine, task: &Task, attempts: u64) -> Value {
    let name = |id: &str| engine.state.get(id).map(label).unwrap_or_else(|| id.to_owned());
    json!({"task":task.id,"shortId":task.short_id,"title":task.title,"outcome":task.outcome,"acceptance":task.acceptance,"attributes":task.attributes,
        "container":task.parent.as_deref().map(name),"dependencies":task.dependencies.iter().map(|d| json!({"on":name(&d.target),"reason":d.reason})).collect::<Vec<_>>(),
        "after":task.after.iter().map(|id| name(id)).collect::<Vec<_>>(),"submission":task.kickbacks() + 1,"submissionsAllowed":attempts,
        "earlierFindings":task.plan_reviews.iter().filter(|r| !r.accepted).map(|r| &r.findings).collect::<Vec<_>>()})
}

fn evaluation_prompt(engine: &Engine, session: &Session, pending: &[&Task], instructions: &str, attempts: u64) -> Result<String> {
    let requests: Vec<&str> = session.requests.iter().filter(|r| r.kind != "wake").map(|r| r.text.as_str()).collect();
    let others: Vec<String> = engine
        .state
        .tasks
        .iter()
        .filter(|t| engine.state.in_scope(t, Some(&session.workspace)) && !t.container() && !pending.iter().any(|p| p.id == t.id))
        .map(|t| {
            let summary = t.attempts.iter().rev().find_map(|a| a.result.as_ref()?["summary"].as_str()).map(|s| format!(" Latest result: {s}")).unwrap_or_default();
            format!("- {} [{}]: {} Criteria: {}{summary}", label(t), t.status(), t.outcome, t.acceptance.join("; "))
        })
        .collect();
    let proposals: Vec<Value> = pending.iter().map(|t| proposal(engine, t, attempts)).collect();
    Ok(format!(
        "You evaluate the proposed tasks for the request in {}, inside the workspace {}. Read whatever you need, but change nothing: no edits, commits or coordinator writes. Workers can only take a task once you accept it.\n\nEvaluator instructions:\n{instructions}\n\nRequest:\n{}\n\nTasks already on the board:\n{}\n\nProposed tasks:\n{}\n\nGive one verdict per proposed task, using its task id: accept it, or kick it back with findings the planner can act on. A task kicked back {attempts} times is rejected and never runs. Only tasks whose prerequisites are already accepted are listed as proposed here; proposed tasks that wait on them appear on the board.",
        clean(&session.workspace),
        clean(session.root.as_deref().unwrap_or(&session.workspace)),
        requests.join("\n\nFollow-up request:\n"),
        if others.is_empty() { "None.".to_owned() } else { others.join("\n") },
        serde_json::to_string_pretty(&proposals)?
    ))
}

fn start_evaluation(engine: &mut Engine, id: &str, pending: &[Submitted]) -> Result<()> {
    let session = session(engine, id)?.clone();
    let settings = engine.state.settings(Some(&session.workspace));
    let agent = settings.evaluator_agent(engine.state.family)?.ok_or("No evaluator configured")?;
    let instructions = crate::workspace_prompts::evaluator(&crate::workspace_prompts::root(), &settings)?;
    let tasks: Vec<&Task> = pending.iter().filter_map(|p| engine.state.get(&p.task)).collect();
    let text = evaluation_prompt(engine, &session, &tasks, &instructions, settings.plan_attempts())?;
    let run = engine.directory.join("sessions").join(id).join(format!("evaluation-{}", session.evaluations.len() + 1));
    fs::create_dir_all(&run)?;
    let port: u16 = read(&engine.directory.join("port.json")).unwrap_or(4242);
    write(&run.join("request.json"), &json!({"session":format!("{id}-evaluation-{}", session.evaluations.len() + 1),"workspace":session.workspace,"root":session.root.as_ref().unwrap_or(&session.workspace),"prompt":text,"agent":agent,
        "conversation":null,"command":[],"url":format!("http://127.0.0.1:{port}"),"schema":serde_json::from_str::<Value>(VERDICTS)?,"schemaFile":run.join("schema.json"),"outputFile":run.join("output.json")}))?;
    write(&run.join("schema.json"), &serde_json::from_str::<Value>(VERDICTS)?)?;
    let evaluation = Evaluation { run: run.clone(), started: now(), ended: None, submitted: pending.to_vec(), verdicts: Vec::new(), usage: None, error: None };
    engine.change(|state| {
        state.sessions.get_mut(id).ok_or("Unknown session")?.evaluations.push(evaluation);
        Ok(Value::Null)
    })?;
    command(std::env::current_exe()?.to_str().ok_or("Invalid executable path")?).arg("--planner").arg(&run).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    Ok(())
}

fn finish_evaluation(engine: &mut Engine, id: &str, exit: &Value) -> Result<()> {
    let session = session(engine, id)?.clone();
    let evaluation = session.evaluations.last().ok_or("Missing evaluation")?.clone();
    let settings = engine.state.settings(Some(&session.workspace));
    let limits = settings.limits.clone();
    let output = read::<Value>(&evaluation.run.join("output.json")).map_err(|error| error.to_string());
    let mut verdicts = Vec::new();
    let mut error = exit["error"].as_str().map(str::to_owned);
    match &output {
        Ok(output) => {
            for verdict in output["verdicts"].as_array().into_iter().flatten() {
                let named = verdict["task"].as_str().unwrap_or_default();
                let Some(submitted) = evaluation.submitted.iter().find(|s| s.task == named || engine.state.get(&s.task).is_some_and(|t| t.short_id.as_deref() == Some(named))) else { continue };
                let accepted = verdict["accept"].as_bool().unwrap_or(false);
                let mut findings: Vec<String> = verdict["findings"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|f| !f.trim().is_empty()).take(limits.findings).map(|f| crate::history::clip(f, limits.finding_words)).collect();
                if !accepted && findings.is_empty() {
                    findings.push("The evaluator kicked this task back without a reason".into());
                }
                verdicts.push((submitted.task.clone(), submitted.revision, accepted, findings));
            }
        }
        Err(message) => error = error.or_else(|| Some(format!("Evaluator produced no verdicts: {message}"))),
    }
    engine.review_plan(&verdicts, settings.plan_attempts())?;
    if error.is_none() && verdicts.len() < evaluation.submitted.len() {
        error = Some(format!("Evaluator returned {} of {} verdicts", verdicts.len(), evaluation.submitted.len()));
    }
    let recorded: Vec<Value> = verdicts.iter().map(|(task, revision, accepted, findings)| json!({"task":task,"revision":revision,"accepted":accepted,"findings":findings})).collect();
    engine.change(|state| {
        let s = state.sessions.get_mut(id).ok_or("Unknown session")?;
        let e = s.evaluations.last_mut().ok_or("Missing evaluation")?;
        e.ended = Some(now());
        e.verdicts = recorded;
        e.usage = exit.get("usage").cloned();
        e.error = error;
        Ok(Value::Null)
    })
    .map(drop)
}

fn evaluate(engine: &mut Engine, session: &Session) -> Result<bool> {
    let settings = engine.state.settings(Some(&session.workspace));
    if settings.evaluator.is_none() {
        return Ok(false);
    }
    if let Some(evaluation) = session.evaluations.last().filter(|e| e.ended.is_none()) {
        if let Some(exit) = exited(&evaluation.run, evaluation.started) {
            finish_evaluation(engine, &session.id, &exit)?;
        }
        return Ok(true);
    }
    let since = session.requests.last().and_then(|r| r.planner_ended).unwrap_or(0);
    let pending: Vec<Submitted> = engine
        .state
        .tasks
        .iter()
        .filter(|t| engine.state.in_scope(t, Some(&session.workspace)) && t.stage == "proposed" && !t.stopped && !t.blocked && t.active.is_none() && !engine.state.awaiting_plan(t))
        .filter(|t| t.plan_reviews.last().is_none_or(|r| r.revision != t.revision || r.at < since))
        .map(|t| Submitted { task: t.id.clone(), revision: t.revision })
        .collect();
    if pending.is_empty() {
        return Ok(false);
    }
    let retried = session.evaluations.last().is_some_and(|e| e.started > since && e.submitted == pending);
    if retried {
        return Ok(false);
    }
    if let Err(error) = start_evaluation(engine, &session.id, &pending) {
        engine.change(|state| {
            state.sessions.get_mut(&session.id).ok_or("Unknown session")?.wake_error = Some(format!("Plan evaluation could not start: {error}"));
            Ok(Value::Null)
        })?;
        return Ok(false);
    }
    Ok(true)
}

pub fn tick(engine: &mut Engine) -> Result<()> {
    let running: Vec<String> = engine.state.sessions.values().filter(|s| s.status == "running").map(|s| s.id.clone()).collect();
    for id in running {
        let session = session(engine, &id)?.clone();
        let Some(request) = session.requests.last() else { continue };
        if request.planner_ended.is_none() {
            let Some(exit) = planner_finished(request) else { continue };
            engine.change(|state| {
                let s = state.sessions.get_mut(&id).ok_or("Unknown session")?;
                if let Some(conversation) = exit["conversation"].as_str() {
                    s.conversation = Some(conversation.to_owned());
                }
                let r = s.requests.last_mut().ok_or("Missing request")?;
                r.planner_ended = Some(now());
                r.exit_code = exit["code"].as_i64();
                r.usage = exit.get("usage").cloned();
                r.error = exit["error"].as_str().map(str::to_owned);
                Ok(Value::Null)
            })?;
            continue;
        }
        if evaluate(engine, &session)? {
            continue;
        }
        let quiet = !engine.state.settings(Some(&session.workspace)).kickoff();
        let attempts = engine.state.settings(Some(&session.workspace)).plan_attempts();
        let minutes = engine.state.settings(Some(&session.workspace)).wake_minutes();
        let actor = format!("planner:{id}");
        let scoped: Vec<&Task> = engine.state.tasks.iter().filter(|t| engine.state.in_scope(t, Some(&session.workspace))).collect();
        let reviewed: Vec<(String, &Task)> = scoped
            .iter()
            .filter(|t| t.stage == "proposed" && t.plan_reviews.last().is_some_and(|r| !r.accepted))
            .map(|t| (format!("plan:{}:{}", t.id, t.plan_reviews.len()), *t))
            .filter(|(k, _)| !session.notified.contains(k))
            .collect();
        if quiet && reviewed.is_empty() {
            continue;
        }
        let finished: Vec<(String, &Task)> = scoped.iter().filter_map(|t| finished_key(t, &actor).filter(|k| !session.notified.contains(k)).map(|k| (k, *t))).collect();
        let blocked: Vec<(String, &Task)> = scoped.iter().filter(|t| needs_decision(t)).map(|t| (decision_key(t), *t)).filter(|(k, _)| !session.notified.contains(k)).collect();
        let idle = !scoped.iter().any(|t| can_still_run(t) && !engine.state.held_by_rejection(t));
        let halted = scoped.iter().any(|t| !t.container() && engine.state.halted(t));
        let since = request.planner_ended.unwrap_or(request.started);
        let due = now().saturating_sub(since) >= minutes.saturating_mul(60_000);
        let directory = engine.directory.join("attempts");
        let live = |t: &Task| t.active.as_ref().and_then(|a| crate::process::cpu_seconds(&directory.join(a)));
        let alert = engine.state.settings(Some(&session.workspace)).compute_alert_seconds();
        let heavy: Vec<(String, &Task, f64)> = scoped
            .iter()
            .filter_map(|t| Some((format!("cpu:{}", t.active.as_ref()?), *t, live(t)?)))
            .filter(|(k, _, c)| *c >= alert && !session.notified.contains(k))
            .collect();
        if !reviewed.is_empty() || (!quiet && (!blocked.is_empty() || !heavy.is_empty() || (idle && !finished.is_empty() && !halted) || (!idle && due))) {
            let summary = board(&scoped, since, &live);
            let (finished, blocked, heavy) = if quiet { (Vec::new(), Vec::new(), Vec::new()) } else { (finished, blocked, heavy) };
            let text = wake_prompt(
                &finished.iter().map(|(_, t)| *t).collect::<Vec<_>>(),
                &blocked.iter().map(|(_, t)| *t).collect::<Vec<_>>(),
                &heavy.iter().map(|(_, t, c)| (*t, *c)).collect::<Vec<_>>(),
                &reviewed.iter().map(|(_, t)| *t).collect::<Vec<_>>(),
                attempts,
                summary,
                idle,
                minutes,
            );
            let keys: Vec<String> = finished.into_iter().chain(blocked).chain(reviewed).map(|(k, _)| k).chain(heavy.into_iter().map(|(k, _, _)| k)).collect();
            match start(engine, &id, text, "wake") {
                Ok(()) => engine.change(|state| {
                    let s = state.sessions.get_mut(&id).ok_or("Unknown session")?;
                    s.notified.extend(keys);
                    s.wake_error = None;
                    Ok(Value::Null)
                })?,
                Err(error) if session.wake_error.as_deref() != Some(error.to_string().as_str()) => engine.change(|state| {
                    state.sessions.get_mut(&id).ok_or("Unknown session")?.wake_error = Some(error.to_string());
                    Ok(Value::Null)
                })?,
                Err(_) => Value::Null,
            };
            continue;
        }
        if !idle {
            continue;
        }
        engine.change(|state| {
            let s = state.sessions.get_mut(&id).ok_or("Unknown session")?;
            s.status = "finished".into();
            s.requests.last_mut().ok_or("Missing request")?.ended = Some(now());
            Ok(Value::Null)
        })?;
    }
    Ok(())
}

pub fn stop_all(engine: &Engine) {
    for session in engine.state.sessions.values().filter(|s| s.status == "running") {
        if let Some(request) = session.requests.last() {
            let _ = fs::write(request.run.join("stop"), b"");
        }
        if let Some(evaluation) = session.evaluations.last().filter(|e| e.ended.is_none()) {
            let _ = fs::write(evaluation.run.join("stop"), b"");
        }
    }
}

fn uuid(seed: &str) -> String {
    let hex = format!("{:x}", Sha256::digest(format!("{seed}:{}", now())));
    format!("{}-{}-4{}-a{}-{}", &hex[0..8], &hex[8..12], &hex[13..16], &hex[17..20], &hex[20..32])
}

fn arguments(request: &Value, agent: &Agent) -> Result<(Vec<String>, Option<String>)> {
    let conversation = request["conversation"].as_str().map(str::to_owned);
    let custom: Vec<String> = serde_json::from_value(request["command"].clone()).unwrap_or_default();
    if !custom.is_empty() {
        let effort = agent.effort().to_owned();
        return Ok((custom.into_iter().map(|a| a.replace("{model}", &agent.model).replace("{effort}", &effort)).collect(), conversation));
    }
    let effort = agent.effort().to_owned();
    match agent.harness {
        Harness::ClaudeCode => {
            let program = std::env::var("CLAUDE_BIN").ok().or_else(|| crate::adapters::executable("claude", std::env::var_os("PATH")).map(|p| p.to_string_lossy().into_owned())).ok_or("Claude Code executable not found on PATH")?;
            let mut argv: Vec<String> = [program.as_str(), "-p", "--model", &agent.model, "--output-format", "stream-json", "--verbose", "--dangerously-skip-permissions"].map(str::to_owned).into();
            if !effort.is_empty() {
                argv.extend(["--effort".into(), effort]);
            }
            if request["schema"].is_object() {
                argv.extend(["--json-schema".into(), request["schema"].to_string(), "--no-session-persistence".into(), "--disallowedTools".into(), "Edit".into(), "Write".into(), "NotebookEdit".into()]);
                return Ok((argv, None));
            }
            let id = conversation.clone().unwrap_or_else(|| uuid(request["session"].as_str().unwrap_or_default()));
            argv.extend([if conversation.is_some() { "--resume" } else { "--session-id" }.to_owned(), id.clone()]);
            Ok((argv, Some(id)))
        }
        Harness::Codex => {
            let program = std::env::var("CODEX_BIN").unwrap_or_else(|_| "codex".into());
            let mut argv = vec![program, "exec".into()];
            if conversation.is_some() {
                argv.push("resume".into());
            }
            argv.extend(["--json", "--skip-git-repo-check", "--dangerously-bypass-approvals-and-sandbox", "--model", &agent.model, "-c"].map(str::to_owned));
            argv.push(format!("model_reasoning_effort=\"{effort}\""));
            if request["schema"].is_object() {
                for (flag, field) in [("--output-schema", "schemaFile"), ("--output-last-message", "outputFile")] {
                    argv.extend([flag.to_owned(), request[field].as_str().ok_or(format!("Missing {field}"))?.to_owned()]);
                }
            }
            argv.extend(conversation.clone());
            argv.push("-".into());
            Ok((argv, conversation))
        }
    }
}

fn usage(events: &Path) -> (Option<Value>, Option<String>) {
    let (mut input, mut output, mut cached, mut written, mut thread, mut cost) = (0u64, 0u64, 0u64, 0u64, None, 0f64);
    let mut seen = false;
    if let Ok(file) = File::open(events) {
        for line in BufReader::new(file).lines().map_while(std::result::Result::ok) {
            let Ok(e) = serde_json::from_str::<Value>(&line) else { continue };
            let n = |u: &Value, key: &str| u[key].as_u64().unwrap_or(0);
            if e["type"] == "result" {
                let u = &e["usage"];
                (cached, written) = (cached + n(u, "cache_read_input_tokens"), written + n(u, "cache_creation_input_tokens"));
                input += n(u, "input_tokens") + n(u, "cache_read_input_tokens") + n(u, "cache_creation_input_tokens");
                output += n(u, "output_tokens");
                cost = cost.max(e["total_cost_usd"].as_f64().unwrap_or(0.0));
                seen = true;
            } else if e["type"] == "turn.completed" {
                let u = &e["usage"];
                input += n(u, "input_tokens");
                cached += n(u, "cached_input_tokens");
                output += n(u, "output_tokens");
                seen = true;
            } else if e["type"] == "thread.started" {
                thread = e["thread_id"].as_str().map(str::to_owned);
            }
        }
    }
    (seen.then(|| json!({"input_tokens":input,"cached_input_tokens":cached,"cache_write_input_tokens":written,"output_tokens":output,"totalTokens":input + output,"costUsd":(cost > 0.0).then_some(cost)})), thread)
}

fn stamped(line: &[u8], at: u64) -> Vec<u8> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let mut out = match line.strip_prefix(b"{") {
        Some(rest) if rest.trim_ascii_start().starts_with(b"}") => format!("{{\"coordinatorAt\":{at}").into_bytes().into_iter().chain(rest.iter().copied()).collect(),
        Some(rest) => format!("{{\"coordinatorAt\":{at},").into_bytes().into_iter().chain(rest.iter().copied()).collect(),
        None => line.to_vec(),
    };
    out.push(b'\n');
    out
}

pub fn rejected(engine: &mut Engine, id: &str, method: &str, path: &str, error: &str) -> Result<()> {
    if engine.state.sessions.get(id).is_none_or(|s| s.requests.is_empty()) {
        return Ok(());
    }
    let entry = json!({"at":now(),"method":method,"path":path,"error":error});
    engine.change(|state| {
        let session = state.sessions.get_mut(id).ok_or("Unknown session")?;
        session.requests.last_mut().ok_or("Missing request")?.rejected.push(entry);
        Ok(Value::Null)
    })
    .map(drop)
}

pub fn transcript(engine: &Engine, id: &str, number: &str, after: u64) -> Result<Value> {
    let session = session(engine, id)?;
    let index: usize = number.parse().map_err(|_| "Request number must be a whole number")?;
    let request = index.checked_sub(1).and_then(|i| session.requests.get(i)).ok_or_else(|| format!("Session {id} has no request {number}"))?;
    let bytes = fs::read(request.run.join("events.jsonl")).unwrap_or_default();
    let start = usize::try_from(after).unwrap_or(usize::MAX).min(bytes.len());
    let end = bytes[start..].iter().rposition(|b| *b == b'\n').map_or(start, |i| start + i + 1);
    let lines: Vec<Value> = bytes[start..end].split(|b| *b == b'\n').filter(|l| !l.trim_ascii().is_empty()).map(|l| serde_json::from_slice(l).unwrap_or_else(|_| json!({"type":"text","text":String::from_utf8_lossy(l)}))).collect();
    Ok(json!({"lines":lines,"next":end,"finished":request.planner_ended.is_some()}))
}

pub fn run(directory: &Path) -> Result<()> {
    let _guard = lock(&directory.join("lock"))?;
    if directory.join("exit.json").exists() {
        return Ok(());
    }
    let request: Value = read(&directory.join("request.json"))?;
    let agent: Agent = serde_json::from_value(request["agent"].clone())?;
    let (argv, conversation) = arguments(&request, &agent)?;
    let workspace = PathBuf::from(request["workspace"].as_str().ok_or("Missing workspace")?);
    let root = request["root"].as_str().map_or_else(|| workspace.clone(), PathBuf::from);
    let mut cli = command(&argv[0]);
    cli.args(&argv[1..]).current_dir(&root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(File::create(directory.join("log.txt"))?);
    cli.env("COORDINATOR_URL", request["url"].as_str().unwrap_or_default()).env("COORDINATOR_SESSION", request["session"].as_str().unwrap_or_default()).env("COORDINATOR_WORKSPACE", &workspace);
    let label = format!("planner-{}-{}", request["session"].as_str().unwrap_or_default(), directory.file_name().unwrap_or_default().to_string_lossy());
    cli.envs(crate::adapters::temp_environment(&root, &label)?);
    for (name, _) in std::env::vars_os() {
        if crate::adapters::hosted_claude_environment(&name.to_string_lossy()) {
            cli.env_remove(name);
        }
    }
    if agent.harness == Harness::Codex {
        let profile = std::env::var_os("USERPROFILE").map(PathBuf::from).filter(|path| path.is_absolute());
        cli.envs(profile.filter(|_| std::env::var_os("CODEX_HOME").is_none_or(|home| home.is_empty())).map(|path| ("CODEX_HOME", path.join(".codex"))));
        cli.env_remove("CODEX_WINDOWS_REGISTERED_CORE");
    }
    if let Some(data) = directory.ancestors().nth(3) {
        match crate::kill_guard::install(data).and_then(|dir| crate::kill_guard::environment(&dir)) {
            Ok(vars) => drop(cli.envs(vars)),
            Err(error) => fs::write(directory.join("kill-guard-error.txt"), error.to_string())?,
        }
    }
    let job = Job::new()?;
    let mut child = cli.spawn()?;
    job.contain(&child)?;
    write(&directory.join("run.json"), &json!({"planner":child.id(),"supervisor":std::process::id(),"started":now(),"argv":argv}))?;
    let mut events = File::create(directory.join("events.jsonl"))?;
    let stdout = child.stdout.take().ok_or("Missing planner stdout")?;
    let recorder = thread::spawn(move || {
        for line in BufReader::new(stdout).split(b'\n').map_while(std::result::Result::ok) {
            let _ = events.write_all(&stamped(&line, now())).and_then(|()| events.flush());
        }
    });
    let prompt = request["prompt"].as_str().unwrap_or_default().as_bytes().to_vec();
    let mut pipe = child.stdin.take().ok_or("Missing planner stdin")?;
    thread::spawn(move || {
        let _ = pipe.write_all(&prompt);
    });
    let mut stopped = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if !stopped && directory.join("stop").exists() {
            job.stop()?;
            stopped = true;
        }
        thread::sleep(Duration::from_millis(200));
    };
    job.drain()?;
    let _ = recorder.join();
    let (used, thread) = usage(&directory.join("events.jsonl"));
    let mut error = Value::Null;
    if request["schema"].is_object() && agent.harness == Harness::ClaudeCode {
        match fs::read_to_string(directory.join("events.jsonl")).map_err(Into::into).and_then(|events| crate::adapters::claude_result(&events)) {
            Ok((output, _, _)) => write(&directory.join("output.json"), &output)?,
            Err(failure) => error = json!(failure.to_string()),
        }
    }
    write(&directory.join("exit.json"), &json!({"code":status.code(),"stopped":stopped,"conversation":thread.or(conversation),"usage":used,"ended":now(),"error":error}))
}

#[cfg(test)]
#[path = "sessions_tests.rs"]
mod tests;
