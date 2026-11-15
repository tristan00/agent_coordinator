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
    for role in kinds() {
        let mut entries = Vec::new();
        if root.join(role).is_dir() {
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
fn selected(root: &Path, settings: &WorkspaceSettings, role: &str) -> Result<(Document, String)> {
    let prompts = settings.prompts.as_ref().ok_or("Workspace prompt selections are required")?;
    let document = match role {
        "project_planner" => &prompts.project_planner,
        "worker_protocol" => &prompts.worker_protocol,
        _ => prompts.roles.get(role).ok_or_else(|| format!("No prompt selected for role {role}"))?,
    };
    let (current, text) = candidate(root, role, &document.candidate)?;
    require(current.sha256 == document.sha256, format!("Prompt candidate {role}/{} changed since the workspace selected it", document.candidate))?;
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
    let mut settings: WorkspaceSettings = serde_json::from_value(input["settings"].clone())?;
    settings.validate(engine.state.family)?;
    let (planner, planner_text) = candidate(root, "project_planner", input["project_planner"].as_str().ok_or("project_planner candidate required")?)?;
    let (worker, worker_text) = candidate(root, "worker_protocol", input["worker_protocol"].as_str().ok_or("worker_protocol candidate required")?)?;
    let mut roles = BTreeMap::new();
    for (role, selected) in input["roles"].as_object().into_iter().flatten() {
        require(role_kinds().contains(&role.as_str()), format!("Unknown role {role}"))?;
        roles.insert(role.clone(), candidate(root, role, selected.as_str().ok_or("Role prompt candidates must be filenames")?)?.0);
    }
    for (_, kind) in STAGE_KINDS {
        require(roles.contains_key(kind), format!("A {kind} prompt candidate is required"))?;
    }
    settings.prompts = Some(Selection { project_planner: planner, worker_protocol: worker, roles });
    let _ = (planner_text, worker_text);
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
    let skipped = crate::sessions::SKIPPED.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(", ");
    format!(
        "# Workspace {w}\n\n\
         Every agent working in this workspace starts in this folder.\n\n\
         ## Keep your work in this workspace\n\n\
         Keep everything you create under `{w}`: code, data, downloads, scripts, logs and scratch files. \
         Do not write to the system temp folder or any folder outside this workspace. \
         `TEMP`, `TMP` and `TMPDIR` already point to a private folder under `{w}\\tmp`, so `/tmp` and temp-file libraries stay inside the workspace.\n\n\
         ## Delivering results\n\n\
         Each request runs in its own request folder inside this workspace, and names a deliverable folder inside that request folder. \
         `GET /api/sessions/{{id}}` shows them as `workspace` and `deliverable`. \
         Results leave the workspace only by delivery: the coordinator copies the deliverable folder out.\n\n\
         - The deliverable folder is copied out by itself, so it must contain everything the result needs and work without anything outside it, \
         including the rest of the workspace. Use paths relative to the folder.\n\
         - Whenever the deliverable folder holds a complete, working version, `POST /api/sessions/{{id}}/deliver` with the body `{{}}`. \
         Each delivery replaces the previous one, so deliver the first working version early and again after each improvement.\n\
         - Planners have the session id in `$COORDINATOR_SESSION`.\n\
         - When the planner ends its turn, the coordinator makes one final delivery once no task can still run. \
         If the request is stopped first, only what was already delivered is handed in.\n\
         - The copy leaves out symbolic links, `.pyc` files, `{LIMITS_FILE}` at the top of the folder, and folders named {skipped}.\n\n\
         ## Coordinator endpoints for this workspace\n\n\
         Base URL: `http://127.0.0.1:{port}` (planners also have it in `$COORDINATOR_URL`). \
         Send request bodies as JSON with `Content-Type: application/json`. \
         `W` is this workspace, `{w}`, URL-encoded; passing a request folder inside it instead narrows results to that request.\n\n\
         | Endpoint | Use |\n\
         |---|---|\n\
         | `GET /api/task-schema?workspace=W` | Task fields, roles, sizes and this workspace's limits |\n\
         | `GET /api/tasks?workspace=W` | List the tasks |\n\
         | `GET /api/tasks/{{id}}` | Read one task |\n\
         | `POST /api/tasks` | Create a task; its `cwd` must be inside this workspace |\n\
         | `POST /api/tasks/batch` | Create several tasks at once |\n\
         | `PATCH /api/tasks/{{id}}` | Correct a task |\n\
         | `POST /api/tasks/{{id}}/stop` and `/resume` | Stop or resume a task |\n\
         | `POST /api/tasks/{{id}}/replan` | Ask for a task to be replanned |\n\
         | `DELETE /api/tasks/{{id}}` | Remove a task |\n\
         | `GET /api/updates?workspace=W&after=SEQ&wait=S` | Changes after sequence `SEQ`; it {wait} |\n\
         | `GET /api/agents?workspace=W` | Worker models a task can request |\n\
         | `GET /api/attempts/{{id}}/log` | Read an attempt's log |\n\
         | `GET /api/sessions/{{id}}` | Read a planner session and its deliveries |\n\
         | `POST /api/sessions/{{id}}/deliver` | Hand in the session's deliverable folder; planners have their session id in `$COORDINATOR_SESSION` |\n"
    )
}

const LIMIT_RANGES: [(&str, u64, u64); 6] = [("words", 60, 240), ("titleWords", 6, 20), ("criteria", 1, 6), ("summaryWords", 20, 80), ("findings", 1, 6), ("findingWords", 15, 60)];
struct Random(u64);
impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, count: usize) -> usize {
        (self.next() % count as u64) as usize
    }
}
pub fn generate(engine: &mut Engine, input: Value, root: &Path) -> Result<Value> {
    let paths: Vec<PathBuf> = serde_json::from_value(input["paths"].clone()).map_err(|_| "paths must be a list of new absolute directories")?;
    require(!paths.is_empty(), "paths must not be empty")?;
    require(paths.iter().all(|path| path.is_absolute() && !path.exists()), "Every path must be a new absolute directory")?;
    require(paths.iter().collect::<std::collections::BTreeSet<_>>().len() == paths.len(), "Paths must be distinct")?;
    let seed = input["seed"].as_u64().unwrap_or_else(|| crate::now() ^ (u64::from(std::process::id()) << 40));
    let mut random = Random(seed);
    let agents = crate::agents::enabled(&Default::default(), engine.state.family);
    let workers: Vec<String> = agents.iter().filter(|(_, agent)| !agent.planner_only).map(|(id, _)| id.clone()).collect();
    let planners: Vec<String> = agents.iter().filter(|(_, agent)| !agent.worker_only).map(|(id, _)| id.clone()).collect();
    require(!planners.is_empty(), "No agents are enabled for this launcher family; start the coordinator from Claude Code or Codex, or set AGENT_FAMILY")?;
    let listed = catalog(root)?;
    let candidates = |role: &str| -> Result<Vec<String>> {
        let names: Vec<String> = listed[role].as_array().into_iter().flatten().filter_map(|entry| entry["candidate"].as_str().map(str::to_owned)).collect();
        require(!names.is_empty(), format!("prompt_candidate/{role} has no valid prompt"))?;
        Ok(names)
    };
    let (planner_prompts, worker_prompts) = (candidates("project_planner")?, candidates("worker_protocol")?);
    let role_prompts: Vec<(&str, Vec<String>)> = role_kinds().into_iter().map(|role| candidates(role).map(|names| (role, names))).collect::<Result<_>>()?;
    let mut created = Vec::new();
    for path in &paths {
        let limits: serde_json::Map<String, Value> = LIMIT_RANGES.iter().map(|(key, low, high)| (key.to_string(), json!(low + random.next() % (high - low + 1)))).collect();
        let updates = ["poll", "events"][random.below(2)];
        let (planner, worker) = (&planners[random.below(planners.len())], &workers[random.below(workers.len())]);
        let settings = json!({"limits":limits,"updates":updates,"planner":planner,"worker":worker});
        created.push(create(engine, json!({"path":path,"settings":settings,
            "project_planner":planner_prompts[random.below(planner_prompts.len())],"worker_protocol":worker_prompts[random.below(worker_prompts.len())],
            "roles":role_prompts.iter().map(|(role, names)| (role.to_string(), json!(names[random.below(names.len())]))).collect::<serde_json::Map<_, _>>()}), root)?);
    }
    Ok(json!({"seed":seed,"workspaces":created}))
}

#[cfg(test)]
#[path = "workspace_prompts_tests.rs"]
mod tests;
