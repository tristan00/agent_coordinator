use crate::{Result, engine::{Engine, workspace}, history::WorkspaceSettings, require};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::{Component, Path, PathBuf}};

pub const DOCUMENTS: [(&str, &str); 2] = [("project_planner", "PROJECT_PLANNER.md"), ("worker_protocol", "WORKER_PROTOCOL.md")];
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub candidate: String,
    pub sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub project_planner: Document,
    pub worker_protocol: Document,
}
pub fn root() -> PathBuf {
    Path::new(option_env!("COORDINATOR_SOURCE_ROOT").unwrap_or(env!("CARGO_MANIFEST_DIR"))).join("prompt_candidate")
}
fn name(value: &str) -> Result<()> {
    require(!value.is_empty() && !value.contains(['/', '\\', ':']) && Path::new(value).components().count() == 1 && matches!(Path::new(value).components().next(), Some(Component::Normal(_))), "Prompt candidate must be a filename within its role directory")
}
pub fn text(path: &Path) -> Result<String> {
    require(fs::symlink_metadata(path)?.is_file(), "Prompt must be a regular file")?;
    let text = fs::read_to_string(path)?;
    let words = text.split_whitespace().count();
    require((1..=200).contains(&words), format!("Prompt {} has {words} words; expected 1..200", path.display()))?;
    Ok(text)
}
fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn candidate(root: &Path, role: &str, selected: &str) -> Result<(Document, String)> {
    name(selected)?;
    let directory = root.join(role).canonicalize()?;
    let path = directory.join(selected);
    require(path.canonicalize()?.parent() == Some(directory.as_path()), "Candidate resolves outside its role directory")?;
    let text = text(&path)?;
    Ok((Document { candidate: selected.into(), sha256: digest(&text) }, text))
}
pub fn catalog(root: &Path) -> Result<Value> {
    let mut result = json!({"directory":root,"max_words":200,"invalid":[],"agents":crate::agents::listing(&Default::default(), crate::agents::launcher())});
    for (role, _) in DOCUMENTS {
        let mut entries = Vec::new();
        fs::create_dir_all(root.join(role))?;
        if root.join(role).exists() {
            for entry in fs::read_dir(root.join(role))? {
                let entry = entry?;
                if !entry.file_type()?.is_file() { continue; }
                let selected = entry.file_name().to_string_lossy().into_owned();
                match candidate(root, role, &selected) {
                    Ok((document, text)) => entries.push(json!({"candidate":document.candidate,"sha256":document.sha256,"words":text.split_whitespace().count()})),
                    Err(error) => result["invalid"].as_array_mut().unwrap().push(json!({"role":role,"candidate":selected,"error":error.to_string()})),
                }
            }
        }
        entries.sort_by(|a, b| a["candidate"].as_str().cmp(&b["candidate"].as_str()));
        result[role] = json!(entries);
    }
    Ok(result)
}
pub fn verify(directory: &Path, settings: &WorkspaceSettings) -> Result<()> {
    let selected = settings.prompts.as_ref().ok_or("Workspace prompt selections are required")?;
    for (filename, document) in [("PROJECT_PLANNER.md", &selected.project_planner), ("WORKER_PROTOCOL.md", &selected.worker_protocol)] {
        name(&document.candidate)?;
        require(digest(&text(&directory.join(filename))?) == document.sha256, format!("Workspace prompt changed: {}", directory.join(filename).display()))?;
    }
    Ok(())
}
pub fn worker(directory: &Path, settings: &WorkspaceSettings) -> Result<Value> {
    verify(directory, settings)?;
    let document = &settings.prompts.as_ref().unwrap().worker_protocol;
    let path = directory.join("WORKER_PROTOCOL.md");
    Ok(json!({"path":path,"candidate":document.candidate,"sha256":document.sha256,"text":text(&path)?}))
}
pub fn create(engine: &mut Engine, input: Value, root: &Path) -> Result<Value> {
    let path = PathBuf::from(input["path"].as_str().ok_or("Workspace path required")?);
    require(path.is_absolute() && !path.exists() && !path.components().any(|part| part == Component::ParentDir), "Workspace must be a new absolute directory")?;
    let parent = path.parent().ok_or("Workspace parent required")?.canonicalize()?;
    let project = Path::new(option_env!("COORDINATOR_SOURCE_ROOT").unwrap_or(env!("CARGO_MANIFEST_DIR"))).canonicalize()?;
    require(!parent.starts_with(project), "Workspace data must be outside the coordinator source directory")?;
    let target = parent.join(path.file_name().ok_or("Workspace name required")?);
    require(!engine.state.config.workspaces.contains_key(&target), "Workspace already configured")?;
    let mut settings: WorkspaceSettings = serde_json::from_value(input["settings"].clone())?;
    settings.validate(engine.state.family)?;
    let (planner, planner_text) = candidate(root, "project_planner", input["project_planner"].as_str().ok_or("project_planner candidate required")?)?;
    let (worker, worker_text) = candidate(root, "worker_protocol", input["worker_protocol"].as_str().ok_or("worker_protocol candidate required")?)?;
    settings.prompts = Some(Selection { project_planner: planner, worker_protocol: worker });
    fs::create_dir(&target)?;
    let result = (|| {
        fs::write(target.join("PROJECT_PLANNER.md"), planner_text)?;
        fs::write(target.join("WORKER_PROTOCOL.md"), worker_text)?;
        verify(&target, &settings)?;
        let path = workspace(&target)?;
        let mut state = engine.state.clone();
        require(!state.config.workspaces.contains_key(&path), "Workspace already configured")?;
        state.config.workspaces.insert(path.clone(), settings.clone());
        engine.commit(state)?;
        Ok(json!({"workspace":path,"settings":settings}))
    })();
    if result.is_err() {
        for (_, filename) in DOCUMENTS { let _ = fs::remove_file(target.join(filename)); }
        let _ = fs::remove_dir(&target);
    }
    result
}

#[cfg(test)]
#[path = "workspace_prompts_tests.rs"]
mod tests;
