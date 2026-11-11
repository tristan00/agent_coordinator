use crate::{model::*, store::*};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

pub fn event(
    s: &mut Snapshot,
    task: Option<&str>,
    kind: &str,
    actor: &str,
    message: impl Into<String>,
    data: Value,
) {
    s.events.push(Event {
        id: id(),
        task_id: task.map(str::to_string),
        kind: kind.into(),
        actor: actor.into(),
        message: message.into(),
        at: now(),
        data,
    });
}
fn task_index(s: &Snapshot, id: &str) -> AppResult<usize> {
    s.tasks
        .iter()
        .position(|t| t.id == id)
        .ok_or_else(|| AppError(404, "Task not found".into()))
}
fn text(v: &Value, field: &str, default: &str) -> String {
    v[field].as_str().unwrap_or(default).trim().to_string()
}
fn number(v: &Value, field: &str, default: u64) -> u64 {
    v[field].as_u64().unwrap_or(default)
}
fn strings(v: &Value, field: &str) -> AppResult<Vec<String>> {
    if v[field].is_null() {
        return Ok(vec![]);
    }
    serde_json::from_value(v[field].clone()).map_err(Into::into)
}
fn check_revision(task: &Task, v: &Value) -> AppResult<()> {
    if v["revision"].as_u64() != Some(task.revision) {
        return Err(conflict("This task changed. Refresh and try again."));
    }
    Ok(())
}
pub(crate) fn touch(t: &mut Task) {
    t.revision = t.revision.saturating_add(1);
    t.updated_at = now();
}
pub(crate) fn validate_task(s: &Snapshot, t: &Task) -> AppResult<()> {
    if t.title.is_empty() {
        return Err(bad("Title is required"));
    }
    if !["epic", "task", "subtask"].contains(&t.kind.as_str()) {
        return Err(bad("Invalid hierarchy type"));
    }
    if t.human_approval && (t.kind == "epic" || !t.assignments.is_empty()) {
        return Err(bad(
            "Human approval gates cannot be epics or have agent assignments",
        ));
    }
    if ![
        "feature",
        "bug",
        "research",
        "refactor",
        "documentation",
        "operations",
    ]
    .contains(&t.task_type.as_str())
    {
        return Err(bad("Invalid task type"));
    }
    if !(1..=4).contains(&t.priority) || ![1, 2, 3, 5, 8, 13].contains(&t.complexity) {
        return Err(bad("Invalid priority or complexity"));
    }
    if let Some(parent) = &t.parent_id {
        let p = &s.tasks[task_index(s, parent)?];
        let expected = if t.kind == "subtask" {
            "task"
        } else if t.kind == "task" {
            "epic"
        } else {
            "none"
        };
        let existing_parent = s
            .tasks
            .iter()
            .any(|previous| previous.id == t.id && previous.parent_id.as_ref() == Some(parent));
        if p.kind != expected || (!existing_parent && (p.archived || p.stage == "done")) {
            return Err(bad(
                "Parent must be an active epic for a task, or an active task for a subtask",
            ));
        }
    } else if t.kind == "subtask" {
        return Err(bad("Subtasks need a parent task"));
    }
    validate_dependencies(s, t)?;
    for (stage, agent) in &t.assignments {
        validate_assignment(s, stage, agent)?;
    }
    if !t.workspace.is_empty() && !std::path::Path::new(&t.workspace).is_dir() {
        return Err(bad("Workspace directory does not exist on this machine"));
    }
    Ok(())
}

pub(crate) fn validate_dependencies(s: &Snapshot, t: &Task) -> AppResult<()> {
    for d in &t.dependencies {
        task_index(s, d)?;
        let mut stack = vec![d.clone()];
        let mut visited = HashSet::new();
        while let Some(next) = stack.pop() {
            if next == t.id {
                return Err(bad("Dependencies cannot form a cycle"));
            }
            if visited.insert(next.clone()) {
                if let Some(other) = s.tasks.iter().find(|x| x.id == next) {
                    stack.extend(other.dependencies.clone());
                }
                stack.extend(
                    s.tasks
                        .iter()
                        .filter(|child| child.parent_id.as_ref() == Some(&next))
                        .map(|child| child.id.clone()),
                );
                if t.parent_id.as_ref() == Some(&next) {
                    stack.push(t.id.clone());
                }
            }
        }
        if t.parent_id.as_ref() == Some(d)
            || s.tasks
                .iter()
                .any(|x| x.id == *d && x.parent_id.as_ref() == Some(&t.id))
        {
            return Err(bad(
                "Use hierarchy for parent/child work; dependencies must be separate",
            ));
        }
    }
    Ok(())
}
fn validate_assignment(s: &Snapshot, stage: &str, agent: &str) -> AppResult<()> {
    if !EXECUTABLE_STAGES.contains(&stage) {
        return Err(bad("Only executable stages can have agents"));
    }
    let a = s
        .agents
        .iter()
        .find(|a| a.id == agent)
        .ok_or_else(|| bad("Assigned agent does not exist"))?;
    if !a.stages.iter().any(|x| x == stage) {
        return Err(bad("Agent does not support this stage"));
    }
    Ok(())
}
pub(crate) fn task_from_input(key: String, v: &Value) -> AppResult<Task> {
    let points = number(&v, "complexity", 3);
    if points > 13
        || number(&v, "priority", 2) > 4
        || number(&v, "max_attempts", 0) > u32::MAX as u64
        || number(&v, "max_sendbacks", 0) > u32::MAX as u64
    {
        return Err(bad("Task numbers are out of range"));
    }
    let confidence = text(&v, "estimate_confidence", "medium");
    if !["low", "medium", "high"].contains(&confidence.as_str()) {
        return Err(bad("Invalid estimate confidence"));
    }
    Ok(Task {
        id: id(),
        key,
        title: text(&v, "title", ""),
        description: text(&v, "description", ""),
        acceptance: text(&v, "acceptance", ""),
        constraints: text(&v, "constraints", ""),
        kind: text(&v, "kind", "task"),
        task_type: text(&v, "task_type", "feature"),
        parent_id: v["parent_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        dependencies: strings(&v, "dependencies")?,
        tags: strings(&v, "tags")?,
        priority: number(&v, "priority", 2) as u8,
        complexity: points as u8,
        estimates: vec![Estimate {
            points: points as u8,
            confidence,
            reason: text(&v, "estimate_reason", "Initial estimate"),
            at: now(),
        }],
        stage: "backlog".into(),
        status: "ready".into(),
        revision: 1,
        visit: 0,
        sendbacks: 0,
        max_attempts: number(&v, "max_attempts", 0) as u32,
        max_sendbacks: number(&v, "max_sendbacks", 0) as u32,
        timeout_seconds: number(&v, "timeout_seconds", 0),
        token_budget: v["token_budget"].as_u64(),
        assignments: if v["assignments"].is_null() {
            BTreeMap::new()
        } else {
            serde_json::from_value(v["assignments"].clone())?
        },
        workspace: text(&v, "workspace", ""),
        current_attempt: None,
        blocked_reason: String::new(),
        created_at: now(),
        updated_at: now(),
        archived: false,
        human_approval: v["human_approval"].as_bool().unwrap_or(false),
        approval: None,
        imported: None,
        recovery_of: None,
        planner_id: v["planner_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
    })
}
pub fn create_task(s: &mut Snapshot, v: Value) -> AppResult<Task> {
    let next_key = s
        .tasks
        .iter()
        .filter_map(|t| {
            t.key
                .strip_prefix("RL-")
                .and_then(|n| n.parse::<u64>().ok())
        })
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| bad("Persisted task key cannot be incremented; repair the invalid key before creating another task"))?;
    let t = task_from_input(format!("RL-{next_key:03}"), &v)?;
    validate_task(s, &t)?;
    s.tasks.push(t.clone());
    event(
        s,
        Some(&t.id),
        "created",
        "human",
        format!("Created {} · {}", t.key, t.title),
        json!({"complexity":t.complexity}),
    );
    Ok(t)
}
pub fn update_task(s: &mut Snapshot, id: &str, v: Value) -> AppResult<Task> {
    let i = task_index(s, id)?;
    check_revision(&s.tasks[i], &v)?;
    if s.tasks[i].status == "running" {
        return Err(conflict(
            "Pause this task before editing its execution inputs",
        ));
    }
    if s.tasks[i].stage == "done" {
        return Err(conflict(
            "Send completed work back before changing its requirements",
        ));
    }
    let mut data = serde_json::to_value(&s.tasks[i])?;
    let allowed = [
        "title",
        "description",
        "acceptance",
        "constraints",
        "task_type",
        "parent_id",
        "dependencies",
        "tags",
        "priority",
        "complexity",
        "max_attempts",
        "max_sendbacks",
        "timeout_seconds",
        "token_budget",
        "assignments",
        "planner_id",
        "workspace",
    ];
    for field in allowed {
        if let Some(value) = v.get(field) {
            data[field] = value.clone();
        }
    }
    let mut t: Task = serde_json::from_value(data)?;
    validate_task(s, &t)?;
    if t.complexity != s.tasks[i].complexity {
        t.estimates.push(Estimate {
            points: t.complexity,
            confidence: text(&v, "estimate_confidence", "medium"),
            reason: text(&v, "estimate_reason", "Estimate revised"),
            at: now(),
        });
    }
    let inputs_changed = t.description != s.tasks[i].description
        || t.acceptance != s.tasks[i].acceptance
        || t.constraints != s.tasks[i].constraints
        || t.workspace != s.tasks[i].workspace
        || t.dependencies != s.tasks[i].dependencies
        || t.parent_id != s.tasks[i].parent_id;
    if inputs_changed && t.stage != "backlog" {
        for a in &mut s.artifacts {
            if a.task_id == t.id {
                a.stale = true;
            }
        }
        t.stage = "planning".into();
        t.status = "ready".into();
        t.visit = t.visit.saturating_add(1);
        t.current_attempt = None;
        t.blocked_reason.clear();
    }
    touch(&mut t);
    s.tasks[i] = t.clone();
    if inputs_changed {
        invalidate_dependents(s, id);
    }
    event(
        s,
        Some(id),
        "edited",
        "human",
        if inputs_changed {
            "Updated inputs; downstream work needs revalidation"
        } else {
            "Updated task"
        },
        json!({"revision":t.revision}),
    );
    Ok(t)
}
pub fn save_agent(s: &mut Snapshot, id_value: Option<&str>, v: Value) -> AppResult<Agent> {
    let old = id_value
        .and_then(|id| s.agents.iter().find(|a| a.id == id))
        .cloned();
    if id_value.is_some() && old.is_none() {
        return Err(AppError(404, "Agent not found".into()));
    }
    let agent = Agent {
        id: old.as_ref().map(|a| a.id.clone()).unwrap_or_else(id),
        name: text(&v, "name", ""),
        adapter: text(&v, "adapter", "cli"),
        model: text(&v, "model", ""),
        stages: normalize_stages(strings(&v, "stages")?),
        instructions: text(&v, "instructions", ""),
        command: text(&v, "command", ""),
        args: strings(&v, "args")?,
        unset_env: strings(&v, "unset_env")?,
        endpoint: text(&v, "endpoint", "http://127.0.0.1:11434"),
        enabled: v["enabled"].as_bool().unwrap_or(true),
        capacity: number(&v, "capacity", 1) as usize,
        timeout_seconds: number(&v, "timeout_seconds", 0),
        created_at: old
            .as_ref()
            .map(|a| a.created_at.clone())
            .unwrap_or_else(now),
        worker: old.as_ref().and_then(|a| a.worker.clone()),
        presence: old.and_then(|a| a.presence),
    };
    if agent.name.is_empty() {
        return Err(bad("Agent needs a name"));
    }
    if !["cli", "ollama", "demo"].contains(&agent.adapter.as_str()) {
        return Err(bad("Unknown adapter"));
    }
    if agent.adapter == "cli" && agent.command.is_empty() {
        return Err(bad("Command agents require an executable"));
    }
    if agent
        .unset_env
        .iter()
        .any(|name| name.is_empty() || name.contains(['=', '\0']))
    {
        return Err(bad("Invalid environment variable names"));
    }
    if agent.adapter == "ollama" {
        let url = reqwest::Url::parse(&agent.endpoint).map_err(|_| bad("Invalid Ollama URL"))?;
        if !["127.0.0.1", "localhost", "[::1]"].contains(&url.host_str().unwrap_or(""))
            || url.scheme() != "http"
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(bad("Ollama must use a local HTTP endpoint"));
        }
        if agent.model.is_empty() {
            return Err(bad("Ollama agents require a model name"));
        }
    }
    if agent.stages.is_empty()
        || agent
            .stages
            .iter()
            .any(|st| !EXECUTABLE_STAGES.contains(&st.as_str()))
    {
        return Err(bad("Select valid stages for this agent"));
    }
    if agent.capacity == 0 {
        return Err(bad("Capacity must be positive"));
    }
    if let Some(i) = s.agents.iter().position(|a| a.id == agent.id) {
        s.agents[i] = agent.clone();
    } else {
        s.agents.push(agent.clone());
    }
    event(
        s,
        None,
        "agent_saved",
        "human",
        format!("Configured agent {}", agent.name),
        json!({"agent_id":agent.id}),
    );
    Ok(agent)
}
pub fn save_settings(s: &mut Snapshot, v: Value) -> AppResult<Settings> {
    let patch = v
        .as_object()
        .ok_or_else(|| bad("Settings must be an object"))?;
    let mut merged = serde_json::to_value(&s.settings)?;
    for (key, value) in patch {
        if key == "expected_revision" {
            let revision = value
                .as_u64()
                .ok_or_else(|| bad("expected_revision must be an integer"))?;
            if revision != s.revision {
                return Err(conflict(
                    "Workspace settings changed. Reload before saving.",
                ));
            }
        } else if merged.get(key).is_some() {
            merged[key] = value.clone();
        } else {
            return Err(bad(format!("Unknown workspace setting: {key}")));
        }
    }
    let mut settings: Settings = serde_json::from_value(merged)?;
    settings.project_name = workspace_name(&settings.project_name)?;
    normalize_routes(&mut settings.stage_agents);
    if settings.concurrency == 0 {
        return Err(bad("Concurrency must be positive"));
    }
    for (stage, agent) in &settings.stage_agents {
        validate_assignment(s, stage, agent)?;
    }
    s.settings = settings.clone();
    event(
        s,
        None,
        "settings",
        "human",
        "Updated coordinator settings",
        json!({"paused":settings.paused}),
    );
    Ok(settings)
}
pub(crate) fn workspace_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(bad("Workspace name must contain 1–100 characters"));
    }
    Ok(name.to_owned())
}
pub fn dependency_reason(s: &Snapshot, t: &Task) -> Option<String> {
    let unmet: Vec<_> = t
        .dependencies
        .iter()
        .filter_map(|id| s.tasks.iter().find(|d| d.id == *id))
        .filter(|d| d.stage != "done" || d.status != "completed")
        .map(|d| d.key.clone())
        .collect();
    if !unmet.is_empty() {
        return Some(format!("Waiting for {}", unmet.join(", ")));
    }
    None
}
fn completion_gate(s: &Snapshot, t: &Task) -> AppResult<()> {
    if let Some(reason) = dependency_reason(s, t) {
        return Err(conflict(reason));
    }
    let children: Vec<_> = s
        .tasks
        .iter()
        .filter(|x| x.parent_id.as_ref() == Some(&t.id) && !x.archived && x.status != "cancelled")
        .collect();
    if children.iter().any(|c| c.status != "completed") {
        return Err(conflict("Complete the child tasks first"));
    }
    if t.kind != "epic" && t.acceptance.trim().is_empty() {
        return Err(bad("Set acceptance criteria before completing work"));
    }
    Ok(())
}
fn stop_attempt(s: &mut Snapshot, task_index: usize, reason: &str) {
    if let Some(aid) = s.tasks[task_index].current_attempt.take() {
        if let Some(a) = s
            .attempts
            .iter_mut()
            .find(|a| a.id == aid && a.status == "running")
        {
            a.status = "cancelling".into();
            a.error = reason.into();
        }
    }
}
pub(crate) fn invalidate_dependents(s: &mut Snapshot, source: &str) {
    let mut pending = vec![source.to_string()];
    let mut visited = HashSet::from([source.to_string()]);
    while let Some(changed) = pending.pop() {
        let parent = s
            .tasks
            .iter()
            .find(|t| t.id == changed)
            .and_then(|t| t.parent_id.clone());
        let targets: Vec<usize> = s
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                !t.archived && (t.dependencies.contains(&changed) || parent.as_ref() == Some(&t.id))
            })
            .map(|(i, _)| i)
            .collect();
        for i in targets {
            let tid = s.tasks[i].id.clone();
            if !visited.insert(tid.clone()) {
                continue;
            }
            pending.push(tid.clone());
            if s.tasks[i].stage == "backlog" || s.tasks[i].status == "cancelled" {
                continue;
            }
            stop_attempt(s, i, "Upstream work changed");
            for artifact in &mut s.artifacts {
                if artifact.task_id == tid {
                    artifact.stale = true;
                }
            }
            let t = &mut s.tasks[i];
            t.stage = if t.kind == "epic" || t.human_approval {
                "backlog"
            } else {
                "planning"
            }
            .into();
            t.status = "ready".into();
            t.approval = None;
            t.visit = t.visit.saturating_add(1);
            t.blocked_reason.clear();
            touch(t);
            event(
                s,
                Some(&tid),
                "invalidated",
                "coordinator",
                "Upstream work changed; prior approvals need revalidation",
                json!({"source_task":changed}),
            );
        }
    }
}
pub fn elapsed(start: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(start)
        .map(|t| {
            (chrono::Utc::now() - t.with_timezone(&chrono::Utc))
                .num_milliseconds()
                .max(0) as u64
        })
        .unwrap_or(0)
}
fn move_stage(
    s: &mut Snapshot,
    i: usize,
    destination: &str,
    actor: &str,
    reason: &str,
) -> AppResult<()> {
    let destination = canonical_stage(destination);
    let current = stage_index(&s.tasks[i].stage).ok_or_else(|| bad("Unknown current stage"))?;
    let target = stage_index(destination).ok_or_else(|| bad("Unknown destination"))?;
    if target == current {
        return Err(bad("Task is already in that stage"));
    }
    let backwards = target < current;
    if backwards {
        if reason.trim().is_empty() {
            return Err(bad("Sendbacks require a reason"));
        }
        if target == 0 {
            return Err(bad("Send work back to an executable stage"));
        }
        if s.tasks[i].max_sendbacks > 0 && s.tasks[i].sendbacks >= s.tasks[i].max_sendbacks {
            return Err(conflict(
                "Sendback limit reached. Increase the limit to continue.",
            ));
        }
        s.tasks[i].sendbacks = s.tasks[i].sendbacks.saturating_add(1);
        let task_id = s.tasks[i].id.clone();
        for a in &mut s.artifacts {
            if a.task_id == task_id && stage_index(&a.stage).unwrap_or(0) >= target {
                a.stale = true;
            }
        }
    } else if target != current + 1 {
        return Err(bad("Stages must be completed in order"));
    }
    if target == STAGES.len() - 1 {
        completion_gate(s, &s.tasks[i])?;
    }
    stop_attempt(s, i, "Task moved to another stage");
    let t = &mut s.tasks[i];
    let from = t.stage.clone();
    t.stage = destination.into();
    t.status = if target == STAGES.len() - 1 {
        "completed"
    } else {
        "ready"
    }
    .into();
    t.visit = t.visit.saturating_add(1);
    t.blocked_reason.clear();
    touch(t);
    let task_id = t.id.clone();
    event(
        s,
        Some(&task_id),
        if backwards { "sendback" } else { "transition" },
        actor,
        format!(
            "{} → {}{}",
            from,
            destination,
            if reason.is_empty() {
                String::new()
            } else {
                format!(": {reason}")
            }
        ),
        json!({"from":from,"to":destination,"reason":reason}),
    );
    if backwards {
        invalidate_dependents(s, &task_id);
    }
    Ok(())
}
pub fn action(s: &mut Snapshot, task_id: &str, v: Value) -> AppResult<Task> {
    action_as(s, task_id, v, false)
}

pub(crate) fn action_as(
    s: &mut Snapshot,
    task_id: &str,
    v: Value,
    approval_authorized: bool,
) -> AppResult<Task> {
    let i = task_index(s, task_id)?;
    check_revision(&s.tasks[i], &v)?;
    let act = text(&v, "action", "");
    let reason = text(&v, "reason", "");
    if matches!(act.as_str(), "approve" | "reopen_approval") && !approval_authorized {
        return Err(AppError(
            403,
            "An operator approval code is required".into(),
        ));
    }
    if s.tasks[i].human_approval
        && ![
            "approve",
            "reopen_approval",
            "comment",
            "archive",
            "restore",
        ]
        .contains(&act.as_str())
    {
        return Err(conflict(
            "This task requires an explicit human approval action",
        ));
    }
    if s.tasks[i].archived && !["restore", "comment"].contains(&act.as_str()) {
        return Err(conflict("Restore this task before changing it"));
    }
    if s.tasks[i].stage == "done"
        && ["pause", "cancel", "resume", "retry", "start"].contains(&act.as_str())
    {
        return Err(conflict("Send completed work back to reopen it"));
    }
    match act.as_str() {
        "approve" => {
            if !s.tasks[i].human_approval || s.tasks[i].stage != "backlog" {
                return Err(conflict(
                    "Only a pending human approval gate can be approved",
                ));
            }
            if reason.is_empty() {
                return Err(bad("Record the explicit human approval evidence"));
            }
            completion_gate(s, &s.tasks[i])?;
            let approved_at = now();
            s.tasks[i].approval = Some(Approval {
                reason: reason.clone(),
                at: approved_at.clone(),
            });
            s.tasks[i].stage = "done".into();
            s.tasks[i].status = "completed".into();
            touch(&mut s.tasks[i]);
            event(
                s,
                Some(task_id),
                "approved",
                "human",
                reason,
                json!({"approved_at": approved_at, "revision": s.tasks[i].revision}),
            );
        }
        "reopen_approval" => {
            if !s.tasks[i].human_approval || s.tasks[i].approval.is_none() {
                return Err(conflict("Only an approved human gate can be reopened"));
            }
            if reason.is_empty() {
                return Err(bad("Record why approval must be obtained again"));
            }
            s.tasks[i].approval = None;
            s.tasks[i].stage = "backlog".into();
            s.tasks[i].status = "ready".into();
            touch(&mut s.tasks[i]);
            invalidate_dependents(s, task_id);
            event(
                s,
                Some(task_id),
                "approval_reopened",
                "human",
                reason,
                json!({}),
            );
        }
        "start" => {
            if s.tasks[i].kind == "epic" {
                return Err(bad("Epics group work. Run their tasks and subtasks."));
            }
            if s.tasks[i].stage != "backlog" {
                return Err(bad("Task has already started"));
            }
            if s.tasks[i].acceptance.is_empty() {
                return Err(bad("Add acceptance criteria before starting"));
            }
            move_stage(s, i, "planning", "human", &reason)?;
        }
        "send_back" => {
            let destination = text(&v, "stage", "");
            if stage_index(&destination) >= stage_index(&s.tasks[i].stage) {
                return Err(bad("Sendback destination must be earlier"));
            }
            move_stage(s, i, &destination, "human", &reason)?;
        }
        "complete_stage" => {
            if reason.is_empty() {
                return Err(bad("Describe the completion evidence"));
            }
            if s.tasks[i].status == "running"
                || s.tasks[i].status == "cancelled"
                || s.tasks[i].archived
            {
                return Err(conflict("Stop active work or restore the task first"));
            }
            if let Some(r) = dependency_reason(s, &s.tasks[i]) {
                return Err(conflict(r));
            }
            if s.tasks[i].kind == "epic" {
                completion_gate(s, &s.tasks[i])?;
                s.tasks[i].stage = "done".into();
                s.tasks[i].status = "completed".into();
                touch(&mut s.tasks[i]);
                event(s, Some(task_id), "completed", "human", reason, json!({}));
            } else {
                let stage = s.tasks[i].stage.clone();
                let idx = stage_index(&stage).ok_or_else(|| {
                    bad("Task has an unknown persisted stage; recover its stage before completion")
                })?;
                if !(1..STAGES.len() - 1).contains(&idx) {
                    return Err(bad("Start the task before completing a stage"));
                }
                s.artifacts.push(Artifact {
                    id: id(),
                    task_id: task_id.into(),
                    attempt_id: None,
                    stage,
                    visit: s.tasks[i].visit,
                    name: "Human completion evidence".into(),
                    content: reason.clone(),
                    media_type: "text/markdown".into(),
                    stale: false,
                    created_at: now(),
                });
                move_stage(s, i, STAGES[idx + 1], "human", &reason)?;
            }
        }
        "pause" | "cancel" | "archive" => {
            let was_running = s.tasks[i].status == "running";
            stop_attempt(s, i, &format!("Task {act} requested"));
            if act == "archive" {
                s.tasks[i].archived = true;
                if was_running {
                    s.tasks[i].status = "paused".into();
                }
            } else {
                s.tasks[i].status = if act == "pause" {
                    "paused"
                } else {
                    "cancelled"
                }
                .into();
            }
            touch(&mut s.tasks[i]);
            event(
                s,
                Some(task_id),
                &act,
                "human",
                format!("Task {act}: {reason}"),
                json!({}),
            );
        }
        "restore" => {
            s.tasks[i].archived = false;
            touch(&mut s.tasks[i]);
            event(
                s,
                Some(task_id),
                "restored",
                "human",
                "Restored archived task",
                json!({}),
            );
        }
        "resume" | "retry" => {
            if s.tasks[i].status == "running" || s.tasks[i].stage == "done" {
                return Err(conflict("Task cannot be resumed in its current state"));
            }
            let used = s
                .attempts
                .iter()
                .filter(|a| a.task_id == task_id && a.visit == s.tasks[i].visit)
                .count();
            if s.tasks[i].max_attempts > 0 && used >= s.tasks[i].max_attempts as usize {
                return Err(conflict(
                    "Attempt limit reached. Increase the limit to continue.",
                ));
            }
            s.tasks[i].status = "ready".into();
            s.tasks[i].archived = false;
            s.tasks[i].blocked_reason.clear();
            touch(&mut s.tasks[i]);
            event(
                s,
                Some(task_id),
                &act,
                "human",
                format!("Task queued. {reason}"),
                json!({}),
            );
        }
        "comment" => {
            if reason.is_empty() {
                return Err(bad("Comment is empty"));
            }
            event(s, Some(task_id), "comment", "human", reason, json!({}));
        }
        _ => return Err(bad("Unknown action")),
    }
    Ok(s.tasks[i].clone())
}

pub fn eligible_agent(s: &Snapshot, t: &Task) -> Result<Agent, String> {
    eligible_for(s, t, None)
}

/// Execution readiness is distinct from the scheduler's enabled setting.
/// Presence-only reports do not establish a dispatch path.
pub fn dispatch_status(s: &Snapshot) -> Value {
    let automatic_workers = s
        .agents
        .iter()
        .filter(|a| crate::workers::connected(a))
        .count();
    let assigned = s
        .agents
        .iter()
        .filter(|a| a.worker.as_ref().is_some_and(|w| w.assignment.is_some()))
        .count();
    let active = s
        .attempts
        .iter()
        .filter(|a| ["running", "cancelling"].contains(&a.status.as_str()))
        .count();
    let pending: Vec<_> = s
        .tasks
        .iter()
        .filter(|t| {
            !t.archived
                && t.kind != "epic"
                && !["completed", "cancelled"].contains(&t.status.as_str())
        })
        .collect();
    let mut eligible = 0;
    let mut launchable = 0;
    for t in &pending {
        if t.status != "ready"
            || t.human_approval
            || t.acceptance.trim().is_empty()
            || dependency_reason(s, t).is_some()
        {
            continue;
        }
        let mut next = (*t).clone();
        if next.stage == "backlog" {
            if !s.settings.auto_advance {
                continue;
            }
            next.stage = "planning".into();
        }
        match eligible_agent(s, &next) {
            Ok(_) => {
                eligible += 1;
                launchable += 1;
            }
            Err(reason) if reason.starts_with("No eligible agent") => {
                eligible += 1;
                if s.agents.iter().any(|a| {
                    crate::workers::connected(a) && eligible_for(s, &next, Some(&a.id)).is_ok()
                }) {
                    launchable += 1;
                }
            }
            _ => {}
        }
    }
    let mode = if s.settings.paused {
        "paused"
    } else if active > 0 {
        "running"
    } else if assigned > 0 {
        "assigned"
    } else if launchable > 0 {
        "dispatching"
    } else if eligible > 0 {
        "waiting_for_workers"
    } else if pending.is_empty() {
        "idle"
    } else {
        "blocked"
    };
    json!({"mode":mode,"pending":pending.len(),"eligible":eligible,"launchable":launchable,"active_attempts":active,"assigned":assigned,"automatic_workers":automatic_workers})
}

pub(crate) fn eligible_for(
    s: &Snapshot,
    t: &Task,
    external_agent: Option<&str>,
) -> Result<Agent, String> {
    if t.acceptance.trim().is_empty() {
        return Err("Add acceptance criteria".into());
    }
    if t.human_approval {
        return Err("This task requires explicit human approval".into());
    }
    if let Some(reason) = dependency_reason(s, t) {
        return Err(reason);
    }
    if t.kind == "epic" {
        return Err("Epics are completed through their child tasks".into());
    }
    if s.tasks.iter().any(|child| {
        child.parent_id.as_ref() == Some(&t.id)
            && !child.archived
            && child.status != "cancelled"
            && child.status != "completed"
    }) {
        return Err("Waiting for child tasks to complete".into());
    }
    if crate::workers::reserved(s, t) {
        return Err("Assigned to a worker; awaiting pickup".into());
    }
    if s.attempts.iter().any(|a| {
        a.task_id == t.id && ["running", "cancelling", "lost"].contains(&a.status.as_str())
    }) {
        return Err("Previous task execution must confirm it has stopped".into());
    }
    let assigned = t.assignments.get(&t.stage).or_else(|| {
        if t.recovery_of.is_some() {
            None
        } else {
            s.settings.stage_agents.get(&t.stage)
        }
    });
    let assigned = if t.recovery_of.is_some() {
        assigned.filter(|id| {
            s.agents
                .iter()
                .any(|agent| &agent.id == *id && agent.enabled && agent.stages.contains(&t.stage))
        })
    } else {
        assigned
    };
    let implementer = s
        .attempts
        .iter()
        .rev()
        .find(|a| a.task_id == t.id && a.stage == "implementation" && a.status == "completed")
        .map(|a| a.agent_id.as_str());
    let used_tokens: u64 = s
        .attempts
        .iter()
        .filter(|a| a.task_id == t.id)
        .map(|a| {
            a.usage
                .input_tokens
                .unwrap_or(0)
                .saturating_add(a.usage.output_tokens.unwrap_or(0))
        })
        .fold(0u64, u64::saturating_add);
    if t.token_budget
        .is_some_and(|limit| limit > 0 && used_tokens >= limit)
    {
        return Err("Reported token budget exhausted".into());
    }
    if t.max_attempts > 0
        && s.attempts
            .iter()
            .filter(|a| a.task_id == t.id && a.visit == t.visit)
            .count()
            >= t.max_attempts as usize
    {
        return Err("Attempt limit reached".into());
    }
    if !t.workspace.is_empty() {
        let canonical = std::fs::canonicalize(&t.workspace)
            .map_err(|_| "Workspace no longer exists".to_string())?;
        if s.occupied_workspaces
            .iter()
            .any(|p| p.starts_with(&canonical) || canonical.starts_with(p))
        {
            return Err("Another project is using this workspace".into());
        }
        if s.attempts.iter().any(|a| {
            ["running", "cancelling", "lost"].contains(&a.status.as_str())
                && a.input["task"]["workspace"].as_str().is_some_and(|w| {
                    !w.is_empty()
                        && std::fs::canonicalize(w)
                            .ok()
                            .is_some_and(|p| p.starts_with(&canonical) || canonical.starts_with(&p))
                })
        }) {
            return Err("Another agent is using this workspace".into());
        }
    }
    s.agents
        .iter()
        .find(|a| {
            a.enabled
                && if let Some(id) = external_agent {
                    a.id == id && a.adapter == "external"
                } else {
                    a.adapter != "external"
                }
                && (!(a.adapter == "demo"
                    || (a.adapter == "cli"
                        && a.args.first().is_some_and(|arg| arg == "example-agent")))
                    || t.tags.iter().any(|tag| tag == "demo"))
                && assigned.is_none_or(|id| id == &a.id)
                && a.stages.contains(&t.stage)
                && !(s.settings.independent_review
                    && t.stage == "review"
                    && implementer == Some(a.id.as_str()))
                && s.attempts
                    .iter()
                    .filter(|run| {
                        run.agent_id == a.id
                            && ["running", "cancelling", "lost"].contains(&run.status.as_str())
                    })
                    .count()
                    < a.capacity
        })
        .cloned()
        .ok_or_else(|| {
            "No eligible agent is available. Check stage routing, capacity, and independent review."
                .into()
        })
}
pub fn claim(s: &mut Snapshot, requested: Option<&str>) -> AppResult<Option<(Attempt, Agent)>> {
    claim_for(s, requested, None)
}
pub fn claim_for(
    s: &mut Snapshot,
    requested: Option<&str>,
    external_agent: Option<&str>,
) -> AppResult<Option<(Attempt, Agent)>> {
    if requested.is_none() && s.settings.paused {
        return Ok(None);
    }
    if external_agent.is_none()
        && s.attempts
            .iter()
            .filter(|a| {
                a.external.is_none() && ["running", "cancelling"].contains(&a.status.as_str())
            })
            .count()
            >= s.settings.concurrency
    {
        return if requested.is_some() {
            Err(conflict("All execution slots are busy"))
        } else {
            Ok(None)
        };
    }
    let mut candidates: Vec<usize> = s
        .tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            !t.archived
                && t.status == "ready"
                && t.stage != "backlog"
                && t.stage != "done"
                && requested.is_none_or(|id| id == t.id)
        })
        .map(|(i, _)| i)
        .collect();
    candidates.sort_by_key(|i| (s.tasks[*i].priority, s.tasks[*i].created_at.clone()));
    for i in candidates {
        let t = s.tasks[i].clone();
        let agent = match eligible_for(s, &t, external_agent) {
            Ok(a) => a,
            Err(reason) => {
                if requested.is_some() {
                    return Err(conflict(reason));
                }
                continue;
            }
        };
        let artifacts: Vec<_> = s
            .artifacts
            .iter()
            .filter(|a| a.task_id == t.id && !a.stale)
            .cloned()
            .collect();
        let feedback: Vec<_> = s
            .events
            .iter()
            .rev()
            .filter(|e| e.task_id.as_ref() == Some(&t.id))
            .take(20)
            .cloned()
            .collect();
        let dependencies: Vec<_> = s
            .tasks
            .iter()
            .filter(|d| t.dependencies.contains(&d.id))
            .cloned()
            .collect();
        let dependency_artifacts: Vec<_> = s
            .artifacts
            .iter()
            .filter(|a| t.dependencies.contains(&a.task_id) && !a.stale)
            .cloned()
            .collect();
        let children: Vec<_> = s
            .tasks
            .iter()
            .filter(|child| child.parent_id.as_ref() == Some(&t.id) && !child.archived)
            .collect();
        let child_artifacts: Vec<_> = s
            .artifacts
            .iter()
            .filter(|a| !a.stale && children.iter().any(|child| child.id == a.task_id))
            .collect();
        let parent = t
            .parent_id
            .as_ref()
            .and_then(|id| s.tasks.iter().find(|p| p.id == *id));
        let recovery_context = t.recovery_of.as_ref().map(|incident| json!({
            "owner":"coordinator", "incident":incident,
            "instructions":recovery_instructions(&t.stage),
            "original_task":s.tasks.iter().find(|task| task.id == incident.task_id),
            "blocked_attempt":s.attempts.iter().find(|attempt| attempt.id == incident.attempt_id),
            "previous_repair_attempts":s.attempts.iter().filter(|attempt| attempt.task_id == t.id).map(|attempt| json!({
                "id":attempt.id,"stage":attempt.stage,"status":attempt.status,
                "summary":attempt.summary,"error":attempt.error,"output":attempt.output,
            })).collect::<Vec<_>>(),
        }));
        let input = json!({"protocol_version":1,"recovery_context":recovery_context,"task":t,"stage":t.stage,"instructions":agent.instructions,"artifacts":artifacts,"feedback":feedback,"dependencies":dependencies,"dependency_artifacts":dependency_artifacts,"parent":parent,"children":children,"child_artifacts":child_artifacts,
          "result_contract":{"estimate":{"points":3,"confidence":"low | medium | high","reason":"Optional: planning agents may revise complexity with reasoning"},"outcome":"complete | send_back | blocked | needs_input | failed","summary":"Required summary","send_back_to":"Earlier stage, for send_back","reason":"Required for sendbacks or blocking","artifacts":[{"name":"plan.md","content":"Deliverable or evidence","media_type":"text/markdown"}],"usage":{"input_tokens":null,"output_tokens":null,"cost_usd":null,"source":"unavailable"}}});
        let attempt = Attempt {
            id: id(),
            task_id: t.id.clone(),
            stage: t.stage.clone(),
            visit: t.visit,
            number: u32::try_from(
                s.attempts
                    .iter()
                    .filter(|a| a.task_id == t.id && a.visit == t.visit)
                    .count(),
            )
            .unwrap_or(u32::MAX)
            .saturating_add(1),
            agent_id: agent.id.clone(),
            agent_name: agent.name.clone(),
            adapter: agent.adapter.clone(),
            model: agent.model.clone(),
            status: "running".into(),
            started_at: now(),
            finished_at: None,
            duration_ms: None,
            summary: String::new(),
            error: String::new(),
            usage: Usage::default(),
            input,
            output: None,
            log: String::new(),
            external: None,
        };
        s.tasks[i].current_attempt = Some(attempt.id.clone());
        s.tasks[i].status = "running".into();
        s.tasks[i].blocked_reason.clear();
        touch(&mut s.tasks[i]);
        s.attempts.push(attempt.clone());
        event(
            s,
            Some(&t.id),
            "attempt_started",
            &agent.name,
            format!("Started {} · attempt {}", t.stage, attempt.number),
            json!({"attempt_id":attempt.id,"model":agent.model}),
        );
        return Ok(Some((attempt, agent)));
    }
    if requested.is_some() {
        return Err(conflict("Task must be ready in an executable stage"));
    }
    Ok(None)
}
pub fn validate_result(result: &AgentResult) -> AppResult<()> {
    if let Some(e) = &result.estimate {
        if ![1, 2, 3, 5, 8, 13].contains(&e.points)
            || !["low", "medium", "high"].contains(&e.confidence.as_str())
            || e.reason.trim().is_empty()
        {
            return Err(bad(
                "Estimate requires valid points, confidence, and reasoning",
            ));
        }
    }
    if !["complete", "send_back", "blocked", "needs_input", "failed"]
        .contains(&result.outcome.as_str())
    {
        return Err(bad("Invalid agent outcome"));
    }
    if result.summary.trim().is_empty() {
        return Err(bad("Agent result requires a summary"));
    }
    if result.outcome == "complete" && result.artifacts.is_empty() {
        return Err(bad(
            "Stage completion requires an artifact containing the deliverable or validation evidence",
        ));
    }
    if result
        .artifacts
        .iter()
        .any(|a| a.name.trim().is_empty() || a.content.trim().is_empty())
    {
        return Err(bad("Artifacts require a name and content"));
    }
    if result.outcome != "complete" && result.reason.trim().is_empty() {
        return Err(bad("Non-completion outcomes require a reason"));
    }
    if result
        .usage
        .cost_usd
        .is_some_and(|v| v < 0.0 || !v.is_finite())
    {
        return Err(bad("Invalid cost"));
    }
    if !["reported", "estimated", "unavailable"].contains(&result.usage.source.as_str()) {
        return Err(bad("Invalid usage source"));
    }
    Ok(())
}
pub fn finish(
    s: &mut Snapshot,
    attempt_id: &str,
    result: AgentResult,
    log: String,
) -> AppResult<()> {
    validate_result(&result)?;
    let ai = s
        .attempts
        .iter()
        .position(|a| a.id == attempt_id)
        .ok_or_else(|| AppError(404, "Attempt not found".into()))?;
    if s.attempts[ai].external.is_some()
        && ["cancelling", "lost"].contains(&s.attempts[ai].status.as_str())
    {
        return Err(conflict(
            "Execution no longer accepts results; confirm the worker stopped before releasing ownership",
        ));
    }
    if s.attempts[ai].status == "cancelling" {
        return fail(s, attempt_id, "Execution cancelled", &log);
    }
    if s.attempts[ai].status != "running" {
        return Ok(());
    } // Duplicate or late results have no effect.
    let ti = task_index(s, &s.attempts[ai].task_id)?;
    if s.tasks[ti].human_approval {
        return Err(conflict(
            "Agent results cannot complete human approval gates",
        ));
    }
    if result.model.as_deref() == Some("deterministic-demo")
        && !s.tasks[ti].tags.iter().any(|tag| tag == "demo")
    {
        return Err(conflict("Demo results are only valid on tasks tagged demo"));
    }
    if s.tasks[ti].current_attempt.as_deref() != Some(attempt_id)
        || s.tasks[ti].visit != s.attempts[ai].visit
    {
        return Err(conflict("Attempt no longer owns this stage"));
    }
    let task_id = s.tasks[ti].id.clone();
    let actor = s.attempts[ai].agent_name.clone();
    if result.outcome == "send_back" {
        let target = result
            .send_back_to
            .as_deref()
            .ok_or_else(|| bad("Sendback requires a destination"))?;
        if stage_index(target).is_none() || stage_index(target) >= stage_index(&s.tasks[ti].stage) {
            return Err(bad("Sendback must target an earlier stage"));
        }
        if s.tasks[ti].max_sendbacks > 0 && s.tasks[ti].sendbacks >= s.tasks[ti].max_sendbacks {
            return Err(conflict("Sendback limit exhausted"));
        }
    }
    if result.outcome == "complete" && s.tasks[ti].stage == "review" {
        completion_gate(s, &s.tasks[ti])?;
    }
    if result.outcome == "complete" && s.tasks[ti].stage == "planning" {
        if let Some(e) = &result.estimate {
            s.tasks[ti].complexity = e.points;
            s.tasks[ti].estimates.push(Estimate {
                points: e.points,
                confidence: e.confidence.clone(),
                reason: e.reason.clone(),
                at: now(),
            });
            event(
                s,
                Some(&task_id),
                "estimated",
                &actor,
                format!(
                    "Estimated {} points ({} confidence): {}",
                    e.points, e.confidence, e.reason
                ),
                json!({"points":e.points,"attempt_id":attempt_id}),
            );
        }
    }
    let a = &mut s.attempts[ai];
    a.status = if result.outcome == "complete" {
        "completed"
    } else {
        &result.outcome
    }
    .into();
    a.finished_at = Some(now());
    a.duration_ms = Some(elapsed(&a.started_at));
    a.summary = result.summary.clone();
    a.usage = result.usage.clone();
    a.log = log;
    a.output = Some(serde_json::to_value(&result)?);
    if let Some(model) = &result.model {
        a.model = model.clone();
    }
    crate::external::reconcile_presence(s);
    s.tasks[ti].current_attempt = None;
    for ar in &result.artifacts {
        s.artifacts.push(Artifact {
            id: id(),
            task_id: task_id.clone(),
            attempt_id: Some(attempt_id.into()),
            stage: s.tasks[ti].stage.clone(),
            visit: s.tasks[ti].visit,
            name: ar.name.clone(),
            content: ar.content.clone(),
            media_type: ar.media_type.clone(),
            stale: false,
            created_at: now(),
        });
    }
    event(
        s,
        Some(&task_id),
        "attempt_finished",
        &actor,
        &result.summary,
        json!({"attempt_id":attempt_id,"outcome":result.outcome,"usage":result.usage}),
    );
    match result.outcome.as_str() {
        "complete" => {
            let next = stage_index(&s.tasks[ti].stage).ok_or_else(|| bad("Task has an unknown persisted stage; recover its stage before accepting completion"))? + 1;
            let destination = STAGES.get(next).ok_or_else(|| bad("Task has no next executable stage; recover its stage before accepting completion"))?;
            move_stage(s, ti, destination, &actor, &result.summary)?;
        }
        "send_back" => {
            move_stage(
                s,
                ti,
                result.send_back_to.as_deref().unwrap(),
                &actor,
                &result.reason,
            )?;
        }
        status => {
            s.tasks[ti].status = status.into();
            s.tasks[ti].blocked_reason = result.reason.clone();
            touch(&mut s.tasks[ti]);
        }
    }
    if ["blocked", "needs_input", "failed"].contains(&result.outcome.as_str()) {
        crate::callbacks::notify(s, ti, attempt_id, &result.reason);
    }
    Ok(())
}
pub fn fail(s: &mut Snapshot, attempt_id: &str, error: &str, log: &str) -> AppResult<()> {
    let ai = s
        .attempts
        .iter()
        .position(|a| a.id == attempt_id)
        .ok_or_else(|| AppError(404, "Attempt missing".into()))?;
    if s.attempts[ai].status == "cancelling" {
        let a = &mut s.attempts[ai];
        a.status = "cancelled".into();
        a.finished_at = Some(now());
        a.duration_ms = Some(elapsed(&a.started_at));
        crate::external::reconcile_presence(s);
        return Ok(());
    }
    if s.attempts[ai].status != "running" {
        return Ok(());
    }
    let task_id = s.attempts[ai].task_id.clone();
    let ti = task_index(s, &task_id)?;
    let a = &mut s.attempts[ai];
    a.status = "failed".into();
    a.error = error.into();
    a.log = log.into();
    a.finished_at = Some(now());
    a.duration_ms = Some(elapsed(&a.started_at));
    crate::external::reconcile_presence(s);
    if s.tasks[ti].current_attempt.as_deref() == Some(attempt_id) {
        s.tasks[ti].current_attempt = None;
        s.tasks[ti].status = "failed".into();
        s.tasks[ti].blocked_reason = error.into();
        touch(&mut s.tasks[ti]);
    }
    event(
        s,
        Some(&task_id),
        "attempt_failed",
        "runner",
        error,
        json!({"attempt_id":attempt_id}),
    );
    crate::callbacks::notify(s, ti, attempt_id, error);
    Ok(())
}
pub fn recover(s: &mut Snapshot) -> AppResult<usize> {
    let ids: Vec<_> = s
        .attempts
        .iter()
        .filter(|a| a.external.is_none() && ["running", "cancelling"].contains(&a.status.as_str()))
        .map(|a| a.id.clone())
        .collect();
    for id in &ids {
        fail(
            s,
            id,
            "Coordinator restarted during execution. Reconcile partial work before continuing.",
            "",
        )?;
    }
    Ok(ids.len())
}

pub fn migrate_workflow(s: &mut Snapshot) -> AppResult<()> {
    normalize_routes(&mut s.settings.stage_agents);
    for a in &mut s.agents {
        a.stages = normalize_stages(std::mem::take(&mut a.stages));
    }
    for i in 0..s.tasks.len() {
        normalize_routes(&mut s.tasks[i].assignments);
        if s.tasks[i].stage == "verification" {
            s.tasks[i].stage = "review".into();
            touch(&mut s.tasks[i]);
            let tid = s.tasks[i].id.clone();
            event(
                s,
                Some(&tid),
                "workflow_migrated",
                "coordinator",
                "Verification consolidated into Review; existing run ownership and history retained",
                json!({"workflow_version":2}),
            );
        }
    }
    Ok(())
}

/// Guidance belongs to the role that can act on it. Planning is not required
/// to execute a repair or prove that an operation it cannot run has succeeded.
pub fn recovery_instructions(stage: &str) -> &'static str {
    match stage {
        "planning" => {
            "Investigate the available failure evidence and produce an actionable repair plan. Planning may complete with that plan even if execution or verification requires the implementation agent. Identify the operation, likely cause, and a changed approach. Do not demand a successful repair before handing the plan to implementation."
        }
        "implementation" => {
            "Investigate the root cause, perform the repair using available authorized capabilities, and run relevant checks when possible. Use the original failure and prior repair attempts to choose a changed approach. Report what changed and what remains uncertain. Do not merely repeat a failed operation or claim that the original task is complete."
        }
        "review" => {
            "Evaluate the repair and its available evidence against the original failure. Report concrete remaining problems and send them to implementation. Do not require planning to perform implementation work or introduce unrelated acceptance requirements."
        }
        _ => "Investigate and resolve the incident through the next actionable step.",
    }
}

fn recovery_stage_available(s: &Snapshot, task: &Task, stage: &str) -> bool {
    s.agents.iter().any(|a| {
        a.enabled
            && a.stages.iter().any(|v| v == stage)
            && (!(a.adapter == "demo"
                || (a.adapter == "cli" && a.args.first().is_some_and(|v| v == "example-agent")))
                || task.tags.iter().any(|tag| tag == "demo"))
    })
}

fn route_recovery(s: &mut Snapshot, i: usize, stage: &str, previous_agent: Option<&str>) {
    let task = s.tasks[i].clone();
    let implementer = s
        .attempts
        .iter()
        .rev()
        .find(|a| a.task_id == task.id && a.stage == "implementation" && a.status == "completed")
        .map(|a| a.agent_id.as_str());
    let mut candidates: Vec<_> = s
        .agents
        .iter()
        .filter(|a| {
            a.enabled
                && a.stages.iter().any(|v| v == stage)
                && (!(a.adapter == "demo"
                    || (a.adapter == "cli"
                        && a.args.first().is_some_and(|v| v == "example-agent")))
                    || task.tags.iter().any(|tag| tag == "demo"))
                && !(s.settings.independent_review
                    && stage == "review"
                    && implementer == Some(a.id.as_str()))
        })
        .collect();
    // Prefer another capable profile after failure. If only one exists, keep
    // working with it and pass all earlier incident attempts in the handoff.
    candidates.sort_by_key(|a| (previous_agent == Some(a.id.as_str()), a.adapter == "ollama"));
    if let Some(agent) = candidates.first().filter(|a| a.adapter != "external") {
        s.tasks[i]
            .assignments
            .insert(stage.into(), agent.id.clone());
    } else {
        s.tasks[i].assignments.remove(stage);
    }
}

fn schedule_supervisor_repair(
    s: &mut Snapshot,
    i: usize,
    attempt_id: &str,
    reason: &str,
    trigger: &str,
) -> AppResult<()> {
    let task = s.tasks[i].clone();
    if let Some(ri) = s
        .tasks
        .iter()
        .position(|t| t.recovery_of.as_ref().is_some_and(|r| r.task_id == task.id))
    {
        let incident = s.tasks[ri].recovery_of.as_ref().unwrap().clone();
        if incident.attempt_id == attempt_id || !incident.resumed {
            return Ok(());
        }
        // A recurrence reuses its incident and history. It does not exhaust an
        // invisible one-shot recovery allowance or spawn a recursive task tree.
        if s.tasks[ri].archived || matches!(s.tasks[ri].status.as_str(), "paused" | "cancelled") {
            return Ok(());
        }
        s.tasks[ri].recovery_of = Some(RecoveryIncident {
            task_id: task.id.clone(),
            attempt_id: attempt_id.into(),
            stage: task.stage.clone(),
            resumed: false,
        });
        let stage = if recovery_stage_available(s, &s.tasks[ri], "planning")
            || !recovery_stage_available(s, &s.tasks[ri], "implementation")
        {
            "planning"
        } else {
            "implementation"
        };
        s.tasks[ri].stage = stage.into();
        s.tasks[ri].status = "ready".into();
        s.tasks[ri].visit = s.tasks[ri].visit.saturating_add(1);
        s.tasks[ri].blocked_reason.clear();
        for a in &mut s.artifacts {
            if a.task_id == s.tasks[ri].id {
                a.stale = true;
            }
        }
        route_recovery(s, ri, stage, None);
        touch(&mut s.tasks[ri]);
        let repair_id = s.tasks[ri].id.clone();
        s.tasks[i].status = "blocked".into();
        s.tasks[i].blocked_reason = format!(
            "Recovery {} is investigating the recurring incident: {reason}",
            s.tasks[ri].key
        );
        touch(&mut s.tasks[i]);
        event(
            s,
            Some(&task.id),
            "supervisor_recovery_reopened",
            "coordinator",
            "Recurring failure reopened its recovery task",
            json!({"recovery_task_id":repair_id,"attempt_id":attempt_id,"reason":reason}),
        );
        return Ok(());
    }
    let workspace = if task.workspace.is_empty() || std::path::Path::new(&task.workspace).is_dir() {
        task.workspace.clone()
    } else {
        String::new()
    };
    let mut repair = create_task(
        s,
        json!({
            "title":format!("Recovery for {}",task.key),
            "description":format!("Investigate and repair the incident from {} ({}) at stage {}. Attempt: {}. Original execution directory: {}.\n\nFailure: {}\n\nPlanning: {}\n\nImplementation: {}\n\nReview: {}",task.key,task.id,task.stage,attempt_id,task.workspace,reason,recovery_instructions("planning"),recovery_instructions("implementation"),recovery_instructions("review")),
            "acceptance":"Address the original failure through an actionable plan, implementation, and review. Record what changed and any remaining uncertainty. The original task resumes its own workflow afterward.",
            "constraints":task.constraints, "planner_id":task.planner_id,
            "task_type":"operations","priority":1,"complexity":3,"workspace":workspace,
            "max_attempts":0,"max_sendbacks":0,"timeout_seconds":0,
            "tags":if task.tags.iter().any(|tag| tag == "demo") {vec!["coordinator-recovery","demo"]} else {vec!["coordinator-recovery"]}
        }),
    )?;
    repair.recovery_of = Some(RecoveryIncident {
        task_id: task.id.clone(),
        attempt_id: attempt_id.into(),
        stage: task.stage.clone(),
        resumed: false,
    });
    let ri = task_index(s, &repair.id)?;
    s.tasks[ri] = repair.clone();
    let stage = if recovery_stage_available(s, &repair, "planning")
        || !recovery_stage_available(s, &repair, "implementation")
    {
        "planning"
    } else {
        "implementation"
    };
    s.tasks[ri].stage = stage.into();
    s.tasks[ri].visit = 1;
    route_recovery(s, ri, stage, None);
    touch(&mut s.tasks[ri]);
    s.tasks[i].dependencies.push(repair.id.clone());
    s.tasks[i].status = "blocked".into();
    s.tasks[i].blocked_reason = format!("Recovery {} is investigating: {reason}", repair.key);
    touch(&mut s.tasks[i]);
    event(
        s,
        Some(&task.id),
        "supervisor_recovery_created",
        "coordinator",
        format!("Incident investigation assigned to {}", repair.key),
        json!({"owner":"coordinator","recovery_task_id":repair.id,"attempt_id":attempt_id,"reason":reason,"trigger":trigger}),
    );
    Ok(())
}

fn reconcile_supervisor_repairs(s: &mut Snapshot) -> AppResult<()> {
    let completed: Vec<_> = s
        .tasks
        .iter()
        .filter(|t| !t.archived && t.stage == "done" && t.status == "completed")
        .filter_map(|t| {
            t.recovery_of
                .as_ref()
                .filter(|r| !r.resumed)
                .map(|r| (t.id.clone(), r.clone()))
        })
        .collect();
    for (repair_id, incident) in completed {
        let Some(i) = s.tasks.iter().position(|t| t.id == incident.task_id) else {
            continue;
        };
        let task = s.tasks[i].clone();
        if task.archived
            || task.human_approval
            || task.status != "blocked"
            || task.current_attempt.is_some()
        {
            continue;
        }
        // A deliberately configured spending budget remains operator policy.
        // Defaults have no budget; the recovery itself has no inherited cap.
        let used = s
            .attempts
            .iter()
            .filter(|a| a.task_id == task.id)
            .fold(0u64, |n, a| {
                n.saturating_add(a.usage.input_tokens.unwrap_or(0))
                    .saturating_add(a.usage.output_tokens.unwrap_or(0))
            });
        if task
            .token_budget
            .is_some_and(|limit| limit > 0 && used >= limit)
        {
            crate::callbacks::notify(
                s,
                i,
                &format!("budget:{}:{}", task.visit, incident.attempt_id),
                "An explicitly configured token budget prevents the original task from resuming; planner action is needed to revise that budget.",
            );
            continue;
        }
        let ri = task_index(s, &repair_id)?;
        s.tasks[ri].recovery_of.as_mut().unwrap().resumed = true;
        touch(&mut s.tasks[ri]);
        s.tasks[i].stage = incident.stage;
        s.tasks[i].visit = s.tasks[i].visit.saturating_add(1);
        s.tasks[i].status = "ready".into();
        s.tasks[i].blocked_reason.clear();
        touch(&mut s.tasks[i]);
        event(
            s,
            Some(&task.id),
            "supervisor_recovery_completed",
            "coordinator",
            "Recovery finished; original task is queued to continue",
            json!({"recovery_task_id":repair_id,"blocked_attempt_id":incident.attempt_id,"visit":s.tasks[i].visit}),
        );
    }
    Ok(())
}

fn recover_worker_blocker(s: &mut Snapshot, i: usize) -> AppResult<()> {
    let task = s.tasks[i].clone();
    if !matches!(task.status.as_str(), "blocked" | "needs_input" | "failed")
        || task.current_attempt.is_some()
    {
        return Ok(());
    }
    let Some(last) = s
        .attempts
        .iter()
        .rev()
        .find(|a| a.task_id == task.id && a.visit == task.visit)
        .filter(|a| {
            matches!(a.status.as_str(), "blocked" | "needs_input" | "failed")
                || (task.status == "failed" && a.status == "cancelled")
        })
        .cloned()
    else {
        return Ok(());
    };
    let reason = last
        .output
        .as_ref()
        .and_then(|v| v["reason"].as_str())
        .filter(|v| !v.is_empty())
        .unwrap_or(if last.error.is_empty() {
            &task.blocked_reason
        } else {
            &last.error
        })
        .to_owned();
    crate::callbacks::notify(s, i, &last.id, &reason);
    if stage_index(&task.stage).is_none() {
        s.tasks[i].stage = if EXECUTABLE_STAGES.contains(&last.stage.as_str()) {
            last.stage.clone()
        } else {
            "planning".into()
        };
        touch(&mut s.tasks[i]);
        event(
            s,
            Some(&task.id),
            "stage_recovered",
            "coordinator",
            "Recovered invalid stage from the durable attempt",
            json!({"previous_stage":task.stage,"stage":s.tasks[i].stage}),
        );
    }
    let task = s.tasks[i].clone();
    if task.recovery_of.is_none() {
        return schedule_supervisor_repair(s, i, &last.id, &reason, "worker failure");
    }
    // A failed investigation stays actionable. Pace retries without imposing a
    // maximum count or exponential hours-long disappearance from the queue.
    if last
        .finished_at
        .as_ref()
        .is_some_and(|at| elapsed(at) < s.settings.retry_delay_seconds.saturating_mul(1000))
    {
        return Ok(());
    }
    let preferred = match task.stage.as_str() {
        "planning" => "implementation",
        "implementation" => "planning",
        "review" => "implementation",
        _ => "planning",
    };
    let target = if recovery_stage_available(s, &task, preferred) {
        preferred
    } else {
        task.stage.as_str()
    };
    s.tasks[i].stage = target.into();
    s.tasks[i].status = "ready".into();
    s.tasks[i].visit = s.tasks[i].visit.saturating_add(1);
    s.tasks[i].blocked_reason.clear();
    // Old repair tasks created by previous versions must not retain the very
    // ceilings which stranded them. Explicit original-task budgets stay intact.
    s.tasks[i].max_attempts = 0;
    s.tasks[i].max_sendbacks = 0;
    s.tasks[i].timeout_seconds = 0;
    s.tasks[i].token_budget = None;
    route_recovery(s, i, target, Some(&last.agent_id));
    touch(&mut s.tasks[i]);
    event(
        s,
        Some(&task.id),
        "recovery_continued",
        "coordinator",
        format!(
            "Recovery continues in {target} after {}: {reason}",
            task.stage
        ),
        json!({"attempt_id":last.id,"from_stage":task.stage,"to_stage":target,"reason":reason,"instructions":recovery_instructions(target)}),
    );
    Ok(())
}

// Waiting for capacity or a locked execution directory is normal queueing.
// A missing route, in contrast, needs the originating planner's attention.
fn notify_unavailable_routes(s: &mut Snapshot) {
    for i in 0..s.tasks.len() {
        let t = s.tasks[i].clone();
        if t.archived
            || t.human_approval
            || t.status != "ready"
            || !EXECUTABLE_STAGES.contains(&t.stage.as_str())
            || dependency_reason(s, &t).is_some()
        {
            continue;
        }
        let route = t.assignments.get(&t.stage).or_else(|| {
            if t.recovery_of.is_none() {
                s.settings.stage_agents.get(&t.stage)
            } else {
                None
            }
        });
        let implementer = s
            .attempts
            .iter()
            .rev()
            .find(|a| a.task_id == t.id && a.stage == "implementation" && a.status == "completed")
            .map(|a| a.agent_id.as_str());
        let available = s.agents.iter().any(|a| {
            a.enabled
                && a.stages.contains(&t.stage)
                && route.is_none_or(|id| id == &a.id)
                && !(s.settings.independent_review
                    && t.stage == "review"
                    && implementer == Some(a.id.as_str()))
                && (!(a.adapter == "demo"
                    || (a.adapter == "cli"
                        && a.args.first().is_some_and(|v| v == "example-agent")))
                    || t.tags.iter().any(|tag| tag == "demo"))
        });
        if !available {
            crate::callbacks::notify(
                s,
                i,
                &format!("routing:{}:{}", t.stage, t.visit),
                "No enabled agent supports the current route. Connect or configure a capable agent so this queued task can continue.",
            );
        }
    }
}

/// Each task reconciles independently: a malformed task cannot roll back
/// unrelated admissions and strand an entire board.
pub fn reconcile(s: &mut Snapshot) -> AppResult<()> {
    crate::external::expire_leases(s)?;
    crate::external::reconcile_presence(s);
    crate::workers::expire_assignments(s);
    for _ in 0..s.tasks.len() {
        let mut changed = false;
        for i in 0..s.tasks.len() {
            let t = &s.tasks[i];
            if t.archived
                || t.kind != "epic"
                || !matches!(t.status.as_str(), "ready" | "running")
                || t.human_approval
            {
                continue;
            }
            let children: Vec<_> = s
                .tasks
                .iter()
                .filter(|c| c.parent_id.as_ref() == Some(&t.id) && !c.archived)
                .collect();
            if children.is_empty()
                || children.iter().any(|c| c.status != "completed")
                || dependency_reason(s, t).is_some()
            {
                continue;
            }
            let tid = t.id.clone();
            s.tasks[i].stage = "done".into();
            s.tasks[i].status = "completed".into();
            touch(&mut s.tasks[i]);
            event(
                s,
                Some(&tid),
                "epic_completed",
                "coordinator",
                "All child tasks completed",
                json!({}),
            );
            changed = true;
        }
        if !changed {
            break;
        }
    }
    if s.settings.paused || !s.settings.auto_advance {
        return Ok(());
    }
    reconcile_supervisor_repairs(s)?;
    for i in 0..s.tasks.len() {
        let t = s.tasks[i].clone();
        if t.archived || t.kind == "epic" || t.human_approval {
            continue;
        }
        let result = if matches!(t.status.as_str(), "blocked" | "needs_input" | "failed") {
            recover_worker_blocker(s, i)
        } else if t.stage == "backlog"
            && t.status == "ready"
            && !t.acceptance.trim().is_empty()
            && dependency_reason(s, &t).is_none()
        {
            move_stage(s, i, "planning", "coordinator", "Dependencies satisfied")
        } else {
            Ok(())
        };
        if let Err(error) = result {
            let message = error.to_string();
            if !s.events.iter().any(|e| {
                e.task_id.as_ref() == Some(&t.id)
                    && e.kind == "task_reconcile_error"
                    && e.message == message
            }) {
                event(
                    s,
                    Some(&t.id),
                    "task_reconcile_error",
                    "coordinator",
                    &message,
                    json!({}),
                );
                crate::callbacks::notify(s, i, "reconcile", &message);
            }
        }
    }
    notify_unavailable_routes(s);
    Ok(())
}

pub fn seed_demo(s: &mut Snapshot) -> AppResult<()> {
    if s.tasks
        .iter()
        .any(|t| t.tags.iter().any(|v| v == "demo") && !t.archived)
    {
        return Err(conflict("Demo tasks are already present"));
    }
    let mut profiles = vec![];
    for (name, stages) in [
        ("Demo · Planner", vec!["planning"]),
        ("Demo · Builder", vec!["implementation"]),
        ("Demo · Reviewer", vec!["review"]),
    ] {
        let a = save_agent(
            s,
            None,
            json!({"name":name,"adapter":"demo","model":"deterministic-demo","stages":stages}),
        )?;
        profiles.push(a);
    }
    let epic = create_task(
        s,
        json!({"title":"A calmer way to coordinate agents","kind":"epic","description":"An example epic. Demo agents simulate handoffs and do not change your files.","tags":["demo"],"complexity":13}),
    )?;
    for (index, (title, kind, points, stage)) in [
        (
            "Define the task handoff contract",
            "research",
            3,
            "planning",
        ),
        (
            "Build the local activity timeline",
            "feature",
            5,
            "implementation",
        ),
        ("Check the retry and sendback rules", "bug", 3, "review"),
        ("Verify acceptance criteria", "operations", 2, "review"),
        ("Document agent setup", "documentation", 2, "backlog"),
        (
            "Explore better complexity estimates",
            "research",
            8,
            "backlog",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut assignments = BTreeMap::new();
        for a in &profiles {
            for stage in &a.stages {
                assignments.insert(stage.clone(), a.id.clone());
            }
        }
        let t = create_task(
            s,
            json!({"title":title,"task_type":kind,"complexity":points,"priority":if index==2 {1}else{2},"parent_id":epic.id,"description":"Sample task for exploring Relay. This is explicitly simulated work; replace it with your own tasks and agents.","acceptance":"The demo stage produces a clearly labeled artifact and preserves the handoff history.","tags":["demo"],"assignments":assignments}),
        )?;
        let ti = task_index(s, &t.id)?;
        while s.tasks[ti].stage != stage {
            let next = stage_index(&s.tasks[ti].stage).unwrap() + 1;
            move_stage(
                s,
                ti,
                STAGES[next],
                "demo setup",
                "Illustrative starting position; no real work has run",
            )?;
        }
    }
    Ok(())
}
