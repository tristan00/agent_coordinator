use crate::{engine, model::*, store::*};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    plan: Manifest,
    #[serde(default)]
    expected_revisions: BTreeMap<String, u64>,
    #[serde(default)]
    adopt: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    plan_id: String,
    #[serde(default)]
    planner_id: Option<String>,
    workspaces: BTreeMap<String, String>,
    #[serde(default)]
    execution: Execution,
    #[serde(default)]
    epics: Vec<Epic>,
    tasks: Vec<Entry>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Execution {
    default_timeout_seconds: Option<u64>,
    max_attempts: Option<u32>,
    max_sendbacks: Option<u32>,
    token_budget: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Epic {
    code: String,
    title: String,
    description: String,
    workspace: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    code: String,
    title: String,
    description: String,
    acceptance: Vec<String>,
    workspace: String,
    points: u8,
    task_type: String,
    priority: u8,
    #[serde(default)]
    constraints: String,
    #[serde(default)]
    deps: Vec<String>,
    #[serde(default = "task_kind")]
    kind: String,
    #[serde(alias = "epic")]
    parent: Option<String>,
    #[serde(default)]
    human_approval: bool,
}
fn task_kind() -> String {
    "task".into()
}

#[derive(Serialize)]
pub struct ImportReport {
    plan_id: String,
    dry_run: bool,
    entries: Vec<ImportChange>,
}

#[derive(Serialize)]
struct ImportChange {
    code: String,
    operation: &'static str,
    task_id: Option<String>,
    revision: Option<u64>,
}

fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
}

fn specs(manifest: &Manifest) -> AppResult<BTreeMap<String, Value>> {
    if manifest.schema_version != 1 || !identifier(&manifest.plan_id) {
        return Err(bad("Use schema version 1 and a valid plan ID"));
    }
    for (alias, path) in &manifest.workspaces {
        if !identifier(alias) || !Path::new(path).is_absolute() || !Path::new(path).is_dir() {
            return Err(bad(format!("Invalid workspace directory for {alias}")));
        }
    }
    let workspace = |alias: &str| {
        manifest
            .workspaces
            .get(alias)
            .ok_or_else(|| bad(format!("Unknown workspace {alias}")))
    };
    let mut result = BTreeMap::new();
    let mut insert = |code: &str, spec| {
        if !identifier(code) || result.insert(code.to_owned(), spec).is_some() {
            return Err(bad(format!("Invalid or duplicate plan code {code}")));
        }
        Ok(())
    };
    for epic in &manifest.epics {
        insert(
            &epic.code,
            json!({
                "title": epic.title, "description": epic.description, "kind": "epic",
                "planner_id": manifest.planner_id,
                "workspace": workspace(&epic.workspace)?, "dependencies": [], "parent_id": null,
                "tags": [format!("plan:{}", manifest.plan_id), format!("epic:{}", epic.code)],
            }),
        )?;
    }
    for entry in &manifest.tasks {
        if !["task", "subtask"].contains(&entry.kind.as_str()) {
            return Err(bad(format!("Invalid task kind on {}", entry.code)));
        }
        if entry.deps.iter().collect::<BTreeSet<_>>().len() != entry.deps.len() {
            return Err(bad(format!("Duplicate dependencies on {}", entry.code)));
        }
        let mut dependencies = entry.deps.clone();
        dependencies.sort_unstable();
        let acceptance = entry
            .acceptance
            .iter()
            .map(|line| line.trim())
            .collect::<Vec<_>>();
        if acceptance.is_empty() || acceptance.iter().any(|line| line.is_empty()) {
            return Err(bad(format!("Set acceptance criteria on {}", entry.code)));
        }
        insert(
            &entry.code,
            json!({
                "title": entry.title, "description": entry.description, "kind": entry.kind,
                "planner_id": manifest.planner_id,
                "acceptance": acceptance.join("\n"), "constraints": entry.constraints,
                "workspace": workspace(&entry.workspace)?, "complexity": entry.points,
                "priority": entry.priority, "task_type": entry.task_type,
                "dependencies": dependencies, "parent_id": entry.parent,
                "human_approval": entry.human_approval,
                "tags": [format!("plan:{}", manifest.plan_id), format!("code:{}", entry.code)],
                "timeout_seconds": manifest.execution.default_timeout_seconds.unwrap_or(0),
                "max_attempts": manifest.execution.max_attempts.unwrap_or(0),
                "max_sendbacks": manifest.execution.max_sendbacks.unwrap_or(0),
                "token_budget": manifest.execution.token_budget,
            }),
        )?;
    }
    Ok(result)
}

fn validate_manifest(specs: &BTreeMap<String, Value>) -> AppResult<()> {
    let mut proposed = Snapshot::default();
    for (code, spec) in specs {
        let mut task = engine::task_from_input(code.clone(), spec)?;
        task.id = code.clone();
        proposed.tasks.push(task);
    }
    for task in &proposed.tasks {
        for reference in task.dependencies.iter().chain(task.parent_id.iter()) {
            if !specs.contains_key(reference) {
                return Err(bad(format!(
                    "{} references unknown plan code {reference}",
                    task.id
                )));
            }
        }
        engine::validate_task(&proposed, task)?;
    }
    Ok(())
}

pub fn reconcile(
    state: &mut Snapshot,
    request: ImportRequest,
    dry_run: bool,
) -> AppResult<ImportReport> {
    let specs = specs(&request.plan)?;
    validate_manifest(&specs)?;
    let plan_id = &request.plan.plan_id;
    for code in request
        .adopt
        .keys()
        .chain(request.expected_revisions.keys())
    {
        if !specs.contains_key(code) {
            return Err(bad(format!("Unknown plan code {code}")));
        }
    }

    let mut ids = BTreeMap::new();
    let mut matched = BTreeSet::new();
    for code in specs.keys() {
        let existing = state.tasks.iter().find(|task| {
            task.imported
                .as_ref()
                .is_some_and(|link| link.plan_id == *plan_id && link.code == *code)
        });
        let bound = request.adopt.get(code);
        if existing.is_some_and(|task| bound.is_some_and(|id| *id != task.id)) {
            return Err(conflict(format!(
                "{code} is already linked to a different task"
            )));
        }
        let index = if let Some(task) = existing {
            state
                .tasks
                .iter()
                .position(|candidate| candidate.id == task.id)
        } else if let Some(id) = bound {
            let index = state
                .tasks
                .iter()
                .position(|task| task.id == *id)
                .ok_or_else(|| bad(format!("Unknown task ID for {code}")))?;
            if state.tasks[index].imported.is_some() {
                return Err(conflict(format!(
                    "Task for {code} already belongs to another plan entry"
                )));
            }
            Some(index)
        } else {
            None
        };
        let id = index.map_or_else(id, |i| state.tasks[i].id.clone());
        if !matched.insert(id.clone()) {
            return Err(bad("A task can only be linked to one plan code"));
        }
        ids.insert(code.clone(), (id, index));
    }

    let mut proposed = state.clone();
    let mut changes = Vec::new();
    let mut changed_inputs = Vec::new();
    for (code, spec) in &specs {
        let (id, index) = &ids[code];
        let mut input = spec.clone();
        input["parent_id"] = spec["parent_id"]
            .as_str()
            .map_or(Value::Null, |parent| json!(ids[parent].0));
        input["dependencies"] = json!(
            spec["dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .map(|dependency| &ids[dependency.as_str().unwrap()].0)
                .collect::<Vec<_>>()
        );
        let mut desired =
            engine::task_from_input(format!("RL-{:03}", proposed.tasks.len() + 1), &input)?;
        desired.id = id.clone();
        let link = ImportedTask {
            plan_id: plan_id.clone(),
            code: code.clone(),
            spec: spec.clone(),
        };
        let (operation, revision) = if let Some(index) = index {
            let current = &state.tasks[*index];
            let previous_spec = current.imported.as_ref().map(|link| &link.spec);
            if previous_spec == Some(spec) {
                ("unchanged", Some(current.revision))
            } else {
                if !dry_run && request.expected_revisions.get(code) != Some(&current.revision) {
                    return Err(conflict(format!(
                        "{code} requires current revision {}",
                        current.revision
                    )));
                }
                if current.kind != desired.kind || current.human_approval != desired.human_approval
                {
                    return Err(conflict(format!(
                        "{code} cannot change hierarchy type or human approval mode"
                    )));
                }
                let mut updated = current.clone();
                let operation = if let Some(previous) = previous_spec {
                    if current.status == "running" {
                        return Err(conflict(format!("Stop {code} before updating its plan")));
                    }
                    if update_fields(&mut updated, desired, previous, spec, &mut proposed)? {
                        changed_inputs.push(id.clone());
                    }
                    "update"
                } else {
                    let dependencies =
                        |task: &Task| task.dependencies.iter().cloned().collect::<BTreeSet<_>>();
                    if current.parent_id != desired.parent_id
                        || dependencies(current) != dependencies(&desired)
                    {
                        return Err(conflict(format!(
                            "Existing links for {code} differ from the plan"
                        )));
                    }
                    if desired.planner_id.is_some() {
                        updated.planner_id = desired.planner_id;
                    }
                    "adopt"
                };
                updated.imported = Some(link);
                engine::touch(&mut updated);
                proposed.tasks[*index] = updated;
                (operation, Some(current.revision))
            }
        } else {
            desired.imported = Some(link);
            proposed.tasks.push(desired);
            ("create", None)
        };
        changes.push(ImportChange {
            code: code.clone(),
            operation,
            task_id: index
                .map(|_| id.clone())
                .or_else(|| (!dry_run).then(|| id.clone())),
            revision,
        });
    }

    if changes.iter().any(|change| change.operation != "unchanged") {
        for task in &proposed.tasks {
            engine::validate_dependencies(&proposed, task)?;
        }
    }
    for change in changes
        .iter()
        .filter(|change| change.operation != "unchanged")
    {
        let id = &ids[&change.code].0;
        let task = proposed.tasks.iter().find(|task| task.id == *id).unwrap();
        let old_parent = ids[&change.code]
            .1
            .and_then(|index| state.tasks[index].parent_id.as_ref());
        if let Some(parent) = &task.parent_id
            && old_parent != Some(parent)
        {
            let parent_task = proposed
                .tasks
                .iter()
                .find(|task| task.id == *parent)
                .unwrap();
            if parent_task.archived || parent_task.stage == "done" {
                return Err(conflict(format!("Parent of {} is closed", change.code)));
            }
        }
        engine::validate_task(&proposed, task)?;
        engine::event(
            &mut proposed,
            Some(id),
            "plan_import",
            "human",
            format!("{} {}", change.operation, change.code),
            json!({"plan_id": plan_id, "code": change.code, "operation": change.operation}),
        );
    }
    for id in changed_inputs {
        engine::invalidate_dependents(&mut proposed, &id);
    }
    if !dry_run {
        for change in &mut changes {
            let id = &ids[&change.code].0;
            change.revision = proposed
                .tasks
                .iter()
                .find(|task| task.id == *id)
                .map(|task| task.revision);
        }
        *state = proposed;
    }
    Ok(ImportReport {
        plan_id: plan_id.clone(),
        dry_run,
        entries: changes,
    })
}

fn update_fields(
    task: &mut Task,
    desired: Task,
    previous: &Value,
    spec: &Value,
    state: &mut Snapshot,
) -> AppResult<bool> {
    let changed = |field: &str| previous[field] != spec[field];
    let mut execution_inputs_changed = false;
    macro_rules! set {
        ($($field:ident),* $(,)?) => {
            $(if changed(stringify!($field)) {
                task.$field = desired.$field;
            })*
        };
    }
    for field in [
        "title",
        "description",
        "acceptance",
        "constraints",
        "workspace",
        "dependencies",
        "parent_id",
        "task_type",
    ] {
        execution_inputs_changed |= changed(field);
    }
    set!(
        title,
        description,
        acceptance,
        constraints,
        workspace,
        dependencies,
        parent_id,
        task_type,
        priority,
        max_attempts,
        max_sendbacks,
        timeout_seconds,
        token_budget,
        planner_id
    );
    if changed("complexity") && task.complexity != desired.complexity {
        task.complexity = desired.complexity;
        task.estimates.push(Estimate {
            points: desired.complexity,
            confidence: "medium".into(),
            reason: "Plan estimate revised".into(),
            at: now(),
        });
    }
    if execution_inputs_changed && task.stage != "backlog" {
        if task.current_attempt.is_some() {
            return Err(conflict("Stop active work before changing requirements"));
        }
        task.stage = if task.human_approval || task.kind == "epic" {
            "backlog"
        } else {
            "planning"
        }
        .into();
        task.approval = None;
        task.status = "ready".into();
        task.visit += 1;
        task.blocked_reason.clear();
        for artifact in &mut state.artifacts {
            if artifact.task_id == task.id {
                artifact.stale = true;
            }
        }
    }
    Ok(execution_inputs_changed)
}
