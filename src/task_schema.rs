use crate::engine::{DEPENDENCY_REASONS, State};
use crate::history;
use serde_json::{Value, json};
use std::path::Path;

const ROLE_NOTES: [(&str, &str); 19] = [
    ("worker", "Default when no other role fits"),
    ("analytics", "Reports what data shows"),
    ("backend-code", "Builds system logic and interfaces"),
    ("ui-code", "Builds screens and controls"),
    ("design", "Decides how the product looks and feels"),
    ("optimization", "Makes things faster or cheaper"),
    ("test", "Finds and reproduces failures"),
    ("docs", "Writes how to install, run and use"),
    ("data", "Fetches and prepares data, with sources"),
    ("audit", "Read-only: inspects and reports"),
    ("abstraction", "Finds the principles behind examples"),
    ("playtest", "Plays it as its person would"),
    ("research", "Finds what is known, with sources"),
    ("art", "Makes what people see and hear"),
    ("art-direction", "Sets the visual style everything follows"),
    ("modelling", "Builds the shapes of things people see"),
    ("technical-art", "Builds shaders, materials and lighting"),
    ("animation", "Makes things move with life and weight"),
    ("vfx", "Makes effects like smoke, splashes and weather"),
];
const EFFORTS: [&str; 5] = ["minimal", "low", "medium", "high", "xhigh"];

fn ids(description: &str, local: bool) -> Value {
    let id = json!({"type":"string","minLength":1,"description":"A task id from this workspace"});
    let item = if local { json!({"oneOf":[id, {"type":"object","properties":{"local":{"type":"string","minLength":1}},"required":["local"],"additionalProperties":false,"description":"A sibling's localKey in the same batch"}]}) } else { id };
    json!({"type":"array","items":item,"uniqueItems":true,"description":description})
}

fn attributes(state: &State, key: Option<&Path>) -> Value {
    let classes = history::classification_schema();
    let models: Vec<String> = crate::agents::enabled(&state.settings(key).agents, state.family).into_keys().collect();
    json!({"type":"object","additionalProperties":false,"description":"Every attribute goes inside this object, never at the top level of the task",
        "properties":{
            "role":{"type":"string","enum":history::ROLES,"default":"worker","description":"Who does the work; roleNotes says what each role is for"},
            "size":{"type":"string","enum":history::SIZES,"description":"large is only allowed on a container; leaf tasks are small or medium"},
            "needs":{"type":"array","uniqueItems":true,"default":[],"items":{"type":"string","enum":state.config.capabilities},"description":"A list, never a string. Only capabilities the workers in this workspace provide"},
            "priority":{"type":"integer","minimum":0,"maximum":3,"default":2,"description":"0 is highest"},
            "artifact":{"type":"string","enum":history::ARTIFACTS,"default":"none","description":"Containers must use none"},
            "deploy":{"type":"boolean","default":false,"description":"true requires artifact candidate"},
            "languages":classes["languages"],
            "functions":classes["functions"],
            "model":{"type":"string","enum":models,"description":"Optional worker model for this task; not allowed on containers"}}})
}

fn task(state: &State, key: Option<&Path>, child: bool) -> Value {
    let limits = state.settings(key).limits;
    let models: Vec<String> = crate::agents::enabled(&state.settings(key).agents, state.family).into_keys().collect();
    let reason = json!({"type":"string","enum":DEPENDENCY_REASONS,"description":"What the dependent uses from the target's result"});
    let target = if child { json!({"oneOf":[{"type":"string","minLength":1},{"type":"object","properties":{"local":{"type":"string","minLength":1}},"required":["local"],"additionalProperties":false}]}) } else { json!({"type":"string","minLength":1}) };
    let mut properties = json!({
        "title":{"type":"string","minLength":1,"description":format!("At most {} words", limits.title_words)},
        "outcome":{"type":"string","minLength":1,"description":"The observable result when the task is done"},
        "cwd":{"type":"string","minLength":1,"description":"Absolute path of an existing folder inside this workspace, with forward slashes"},
        "acceptance":{"type":"array","maxItems":limits.criteria,"items":{"type":"string","minLength":1},"description":format!("At most {} criteria. An empty list makes the task a container that groups child tasks and runs no worker", limits.criteria)},
        "dependencies":{"type":"array","items":{"type":"object","properties":{"target":target,"reason":reason},"required":["target","reason"],"additionalProperties":false},"description":"Tasks whose results this task uses; it starts only after they are done"},
        "after":ids("Tasks that must settle before this one starts, without using their results", child),
        "conflicts":ids("Tasks that must never run at the same time as this one", child),
        "related":ids("Tasks that are related but impose no ordering", child),
        "model":{"type":["string","null"],"enum":models.iter().map(|m| json!(m)).chain([Value::Null]).collect::<Vec<_>>()},
        "effort":{"type":["string","null"],"enum":EFFORTS.iter().map(|e| json!(e)).chain([Value::Null]).collect::<Vec<_>>()},
        "attributes":attributes(state, key)
    });
    if child {
        properties["localKey"] = json!({"type":"string","minLength":1,"description":"Unique within the batch; siblings refer to this child with {\"local\":localKey}"});
        properties["cwd"]["description"] = json!("Optional; children inherit the container's cwd");
        return json!({"type":"object","additionalProperties":false,"properties":properties,"required":["localKey","title","outcome"],
            "if":{"required":["acceptance"],"properties":{"acceptance":{"minItems":1}}},"then":{"properties":{"attributes":{"properties":{"size":{"not":{"const":"large"}}}}}}});
    }
    properties["key"] = json!({"type":"string","minLength":1,"description":"Optional stable key; creating again with the same key in the same folder updates that task"});
    properties["supersedes"] = ids("Idle tasks without children that this new task replaces; their dependents are rewired to it", false);
    properties["parent"] = json!({"type":["string","null"],"description":"Id of the container this task belongs to"});
    properties["standalone"] = json!({"type":"boolean","description":"true when the task belongs to no container"});
    json!({"type":"object","additionalProperties":false,"properties":properties,"required":["title","outcome","cwd"],
        "if":{"required":["acceptance"],"properties":{"acceptance":{"minItems":1}}},"then":{"properties":{"attributes":{"properties":{"size":{"not":{"const":"large"}}}}}},
        "oneOf":[{"required":["parent"],"properties":{"parent":{"type":"string"}}},{"required":["standalone"],"properties":{"standalone":{"const":true}}}],
        "description":"Exactly one grouping: parent (a container id) or standalone true"})
}

fn rules(state: &State, key: Option<&Path>) -> Vec<String> {
    let settings = state.settings(key);
    let limits = &settings.limits;
    let mut rules = vec![
        format!("The title, outcome and acceptance criteria together are at most {} words; split larger work into child tasks under a container.", limits.words),
        format!("A title is at most {} words and a task has at most {} acceptance criteria.", limits.title_words, limits.criteria),
        "A task with an empty acceptance list is a container: it groups child tasks, runs no worker, uses artifact none and sets no model.".into(),
        "size large is only allowed on a container. Leaf tasks are small or medium.".into(),
        "Every attribute (role, size, needs, priority, artifact, deploy, languages, functions, model) goes inside attributes, never at the top level.".into(),
        format!("needs is a list and may only name capabilities the workers here provide: {}.", serde_json::to_string(&state.config.capabilities).unwrap_or_default()),
        "deploy true requires artifact candidate.".into(),
        "Task ids, digests and AGENTS.md belong in relations, never in the title, outcome or criteria.".into(),
        "cwd must be an existing folder inside this workspace.".into(),
        "No task may relate to itself, and dependencies and after may not form a cycle or point at the task's own container.".into(),
        "A batch creates one container and its children together. requestKey makes it idempotent within this workspace: the same key with the same body returns the first result, and the same key with a different body is rejected.".into(),
        "PATCH /api/tasks/{id} takes the same fields, all optional; send only what changes. A task cannot be edited while its worker runs.".into(),
        format!("After {} failed attempts on the same criterion a task goes to replanning.", history::STRIKES),
    ];
    if settings.evaluator.is_some() {
        let n = settings.plan_attempts();
        rules.push(format!("Tasks the planner creates or changes start as proposed. A plan evaluator checks a proposed task once every task it depends on or runs after has been accepted. A task kicked back {n} times is rejected and never runs, and so is every task that depends on it."));
    }
    rules
}

pub fn describe(state: &State, key: Option<&Path>) -> Value {
    let container = {
        let mut container = task(state, key, false);
        container["properties"]["acceptance"] = json!({"type":"array","maxItems":0,"description":"Always empty: the container groups the children"});
        container
    };
    json!({
        "task":task(state, key, false),
        "batch":{"type":"object","additionalProperties":false,"required":["requestKey","container","children"],"properties":{
            "requestKey":{"type":"string","minLength":1,"description":"Unique per batch within this workspace"},
            "container":container,
            "children":{"type":"array","minItems":1,"items":task(state, key, true)}}},
        "rules":rules(state, key),
        "roles":history::ROLES,
        "roleNotes":ROLE_NOTES.iter().map(|(role, note)| (role.to_string(), json!(note))).collect::<serde_json::Map<_, _>>(),
        "sizes":history::SIZES,
        "needs":state.config.capabilities,
        "artifacts":history::ARTIFACTS,
        "priority":"attributes.priority: 0 (highest) to 3; omitted means 2",
        "dependencyReasons":DEPENDENCY_REASONS,
        "classifications":history::classification_schema(),
        "limits":state.settings(key).limits.value()
    })
}
