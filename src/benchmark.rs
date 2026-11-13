use crate::{Result, engine::{Engine, State, Task, workspace}, history::{Actor, WorkspaceSettings}, now, require};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceRun {
    pub id: String,
    pub group: PathBuf,
    pub deadline_ms: u64,
}

pub fn expired(state: &State, task: &Task) -> bool {
    workspace(Path::new(&task.cwd)).ok().is_some_and(|directory|
        state.benchmark_runs.iter().any(|(path, run)| directory.starts_with(path) && now() >= run.deadline_ms))
}

impl Engine {
    pub fn register_benchmark(&mut self, input: Value) -> Result<Value> {
        let path = workspace(Path::new(input["workspace"].as_str().ok_or("workspace required")?))?;
        let group = workspace(Path::new(input["group"].as_str().ok_or("group required")?))?;
        require(path.starts_with(&group), "Run workspace must be within its group")?;
        let settings: WorkspaceSettings = serde_json::from_value(input["settings"].clone())?;
        settings.validate()?;
        crate::workspace_prompts::verify(&group, &settings)?;
        crate::workspace_prompts::verify(&path, &settings)?;
        let run = WorkspaceRun { id: input["run_id"].as_str().ok_or("run_id required")?.into(), group: group.clone(),
            deadline_ms: input["deadline_ms"].as_u64().ok_or("deadline required")? };
        require(!run.id.is_empty() && run.deadline_ms > now(), "Run must have an ID and future deadline")?;
        if let Some(existing) = self.state.benchmark_runs.get(&path) {
            require(existing == &run && self.state.config.workspaces.get(&group) == Some(&settings), "Workspace run already registered differently")?;
            return Ok(json!({"ok":true}));
        }
        require(!self.state.benchmark_runs.keys().any(|existing| path.starts_with(existing) || existing.starts_with(&path)), "Run paths overlap")?;
        let mut state = self.state.clone();
        state.config.workspaces.insert(group, settings);
        state.benchmark_runs.insert(path, run);
        self.commit(state)?;
        Ok(json!({"ok":true}))
    }

    pub fn stop_benchmark(&mut self, id: &str) -> Result<Value> {
        let mut state = self.state.clone();
        let run = state.benchmark_runs.values_mut().find(|run| run.id == id).ok_or("Benchmark run not registered")?;
        run.deadline_ms = run.deadline_ms.min(now());
        self.commit(state)?;
        self.expire_benchmark_runs()?;
        Ok(json!({"ok":true}))
    }

    pub fn expire_benchmark_runs(&mut self) -> Result<()> {
        let mut state = self.state.clone();
        let mut changed = false;
        for task in &mut state.tasks {
            if let Ok(cwd) = workspace(Path::new(&task.cwd))
                && let Some((_, run)) = state.benchmark_runs.iter().find(|(path, _)| cwd.starts_with(path))
                && task.workspace.as_ref() != Some(&run.group) {
                task.workspace = Some(run.group.clone());
                changed = true;
            }
        }
        if changed { self.commit(state)?; }
        let ids: Vec<_> = self.state.tasks.iter().filter(|task| !task.stopped && task.stage != "done" && expired(&self.state, task))
            .map(|task| task.id.clone()).collect();
        for id in ids { self.control_as(true, Some(&id), Actor::Scheduler, "Benchmark run ended")?; }
        Ok(())
    }
}
