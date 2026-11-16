use crate::{Result, engine::{Engine, workspace}, history::{Role, WorkspaceSettings}, require};
use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::{Component, Path, PathBuf}};

pub const LIMITS_FILE: &str = "LIMITS.json";
pub const AGENTS_FILE: &str = "AGENTS.md";
/// Prompt kinds chosen by a task's stage rather than its role.
pub const STAGE_KINDS: [(&str, &str); 2] = [("review", "reviewer"), ("replan", "replanner")];
pub fn kinds() -> Vec<&'static str> {
    ["project_planner", "worker_protocol"]
        .into_iter()
        .chain(crate::history::ROLES.into_iter().filter(|role| *role != "worker"))
        .chain(STAGE_KINDS.map(|(_, kind)| kind))
        .collect()
}
/// Every kind a workspace must select besides the planner and worker_protocol prompts.
pub fn role_kinds() -> Vec<&'static str> {
    kinds().into_iter().skip(2).collect()
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub candidate: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub project_planner: Document,
    pub worker_protocol: Document,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, Document>,
}
pub fn root() -> PathBuf {
    Path::new(option_env!("COORDINATOR_SOURCE_ROOT").unwrap_or(env!("CARGO_MANIFEST_DIR"))).join("prompts")
}
fn default_file(kind: &str) -> String {
    match kind {
        "project_planner" => "planner.txt".into(),
        "worker_protocol" => "worker.txt".into(),
        _ => format!("{kind}.txt"),
    }
}
fn default_prompt(root: &Path, kind: &str) -> Result<(Document, String)> {
    let file = default_file(kind);
    let path = root.join(&file);
    let text = text(&path).map_err(|error| format!("Default prompt {}: {error}", path.display()))?;
    Ok((Document { candidate: file, sha256: digest(&text), default: true }, text))
}
fn default_settings(root: &Path) -> Result<Value> {
    let path = root.join("settings.json");
    serde_json::from_str(&fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?).map_err(|error| format!("{}: {error}", path.display()).into())
}
pub fn defaults(root: &Path) -> Result<Value> {
    let mut prompts = serde_json::Map::new();
    for kind in kinds() {
        let (document, text) = default_prompt(root, kind)?;
        prompts.insert(kind.into(), json!({"file":document.candidate,"sha256":document.sha256,"words":text.split_whitespace().count()}));
    }
    Ok(json!({"directory":root,"settings":default_settings(root)?,"prompts":prompts}))
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
fn selected(root: &Path, settings: &WorkspaceSettings, role: &str) -> Result<(Document, String)> {
    let prompts = settings.prompts.as_ref().ok_or("Workspace prompt selections are required")?;
    let document = match role {
        "project_planner" => &prompts.project_planner,
        "worker_protocol" => &prompts.worker_protocol,
        _ => prompts.roles.get(role).ok_or_else(|| format!("No prompt selected for role {role}"))?,
    };
    require(document.default, format!("Workspace uses the removed prompt candidate {role}/{}", document.candidate))?;
    let (current, text) = default_prompt(root, role)?;
    require(current.sha256 == document.sha256, format!("Prompt prompts/{} changed since the workspace selected it", document.candidate))?;
    Ok((current, text))
}
pub fn verify(root: &Path, settings: &WorkspaceSettings) -> Result<()> {
    let roles: Vec<String> = settings.prompts.as_ref().map(|p| p.roles.keys().cloned().collect()).unwrap_or_default();
    ["project_planner", "worker_protocol"].into_iter().chain(roles.iter().map(String::as_str)).try_for_each(|role| selected(root, settings, role).map(|_| ()))
}
pub fn planner(root: &Path, settings: &WorkspaceSettings) -> Result<String> {
    Ok(selected(root, settings, "project_planner")?.1)
}
pub fn worker(root: &Path, settings: &WorkspaceSettings, role: Role, stage: &str) -> Result<Value> {
    let staged = STAGE_KINDS.iter().find(|(name, _)| *name == stage).map(|(_, kind)| *kind);
    let own = settings.prompts.as_ref().is_some_and(|p| p.roles.contains_key(role.name()));
    let kind = staged.unwrap_or(if own { role.name() } else { "worker_protocol" });
    let (document, text) = selected(root, settings, kind)?;
    Ok(json!({"role":kind,"candidate":document.candidate,"sha256":document.sha256,"text":text}))
}
pub fn create(engine: &mut Engine, input: Value, root: &Path) -> Result<Value> {
    let path = PathBuf::from(input["path"].as_str().ok_or("Workspace path required")?);
    require(path.is_absolute() && !path.exists() && !path.components().any(|part| part == Component::ParentDir), "Workspace must be a new absolute directory")?;
    let parent = path.parent().ok_or("Workspace parent required")?.canonicalize()?;
    let project = Path::new(option_env!("COORDINATOR_SOURCE_ROOT").unwrap_or(env!("CARGO_MANIFEST_DIR"))).canonicalize()?;
    require(!parent.starts_with(project), "Workspace data must be outside the coordinator source directory")?;
    let target = parent.join(path.file_name().ok_or("Workspace name required")?);
    require(!engine.state.config.workspaces.contains_key(&target), "Workspace already configured")?;
    let settings_input = if input["settings"].is_null() { default_settings(root)? } else { input["settings"].clone() };
    let mut settings: WorkspaceSettings = serde_json::from_value(settings_input)?;
    settings.validate(engine.state.family)?;
    let pick = |kind: &str| -> Result<Document> { Ok(default_prompt(root, kind)?.0) };
    let roles = role_kinds().into_iter().map(|kind| Ok((kind.to_owned(), pick(kind)?))).collect::<Result<BTreeMap<_, _>>>()?;
    settings.prompts = Some(Selection { project_planner: pick("project_planner")?, worker_protocol: pick("worker_protocol")?, roles });
    fs::create_dir(&target)?;
    let port: u16 = crate::read(&engine.directory.join("port.json")).unwrap_or(4242);
    let result = (|| {
        fs::write(target.join(LIMITS_FILE), serde_json::to_vec_pretty(&settings.limits)?)?;
        fs::write(target.join(AGENTS_FILE), agents_guide(&target, port, settings.updates))?;
        let path = workspace(&target)?;
        let mut state = engine.state.clone();
        require(!state.config.workspaces.contains_key(&path), "Workspace already configured")?;
        state.config.workspaces.insert(path.clone(), settings.clone());
        state.short_name(&path);
        engine.commit(state)?;
        Ok(json!({"workspace":path,"settings":settings}))
    })();
    if result.is_err() {
        let _ = fs::remove_file(target.join(LIMITS_FILE));
        let _ = fs::remove_file(target.join(AGENTS_FILE));
        let _ = fs::remove_dir(&target);
    }
    result
}
pub fn agents_guide(workspace: &Path, port: u16, updates: crate::history::Updates) -> String {
    let w = workspace.to_string_lossy().trim_start_matches(r"\\?\").to_owned();
    let wait = match updates {
        crate::history::Updates::Events => "holds the request until something in the workspace changes or S seconds pass",
        crate::history::Updates::Poll => "answers immediately in this workspace, so call it again to follow progress",
    };
    format!(
        "# Workspace {w}\n\n\
         Every agent working in this workspace starts in this folder.\n\n\
         ## Keep your work in this workspace\n\n\
         Keep everything you create inside `{w}`. Temp files already go to `{w}\\tmp`.\n\n\
         ## Track the work with git\n\n\
         Commit your work as you go. Each task works on its own branch, and the coordinator merges it when the task is accepted.\n\n\
         ## Coordinator endpoints for this workspace\n\n\
         Base URL: `http://127.0.0.1:{port}` (planners also have it in `$COORDINATOR_URL`). \
         Send request bodies as JSON with `Content-Type: application/json`. \
         `W` is this workspace, `{w}`, URL-encoded; passing a request folder inside it instead narrows results to that request.\n\n\
         {endpoints}",
        endpoints = crate::api::guide(&format!("it {wait}"))
    )
}


#[cfg(test)]
#[path = "workspace_prompts_tests.rs"]
mod tests;
