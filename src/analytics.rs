use crate::{
    Result,
    engine::{State, Task},
    metrics,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
struct Query {
    from: Option<u64>,
    to: Option<u64>,
    filters: BTreeMap<String, Value>,
    group_by: Option<String>,
}
const GROUPS: [&str; 9] = ["workspace", "lane", "size", "priority", "needs", "artifact", "criteria", "relations", "container"];
fn features(state: &State, task: &Task) -> Value {
    let relations = task.dependencies.len() + task.after.len() + task.conflicts.len() + usize::from(task.parent.is_some());
    json!({"workspace":task.workspace.as_ref().map(|w| w.to_string_lossy()).unwrap_or_else(|| task.cwd.clone().into()),
        "lane":task.attributes.lane,"size":task.attributes.size,"priority":task.attributes.priority(),"needs":task.attributes.needs,"artifact":task.attributes.artifact,
        "criteria":task.acceptance.len(),"relations":relations,"container":task.container(),"children":state.children(&task.id).count(),"words":task.definition_words()})
}
fn outcomes(task: &Task) -> Value {
    let count = |stage: &str, status: Option<&str>| task.attempts.iter().filter(|a| a.stage == stage && status.is_none_or(|s| a.status() == Some(s))).count();
    let first = task.attempts.iter().filter(|a| a.result.is_some()).take(2).filter_map(|a| a.status()).collect::<Vec<_>>();
    let tokens: u64 = task.attempts.iter().filter_map(|a| a.evidence.usage.as_ref()?.total_tokens).sum();
    json!({"stage":task.stage,"attempts":task.attempts.len(),"implementations":count("implementation", None),"reviews":count("review", None),
        "replans":count("replan", None),"rejections":count("review", Some("rejected")),"blocks":count("implementation", Some("blocked")) + count("review", Some("blocked")),
        "held":task.blocked,"errors":task.attempts.iter().filter(|a| a.error.is_some()).count(),
        "firstPass":(task.stage == "done" && !task.container()).then(|| u8::from(first == ["completed", "accepted"])),
        "revisions":task.revision,"tokens":tokens,"timing":metrics::lifecycle(task)})
}
pub fn report(state: &State, input: Value) -> Result<Value> {
    let query: Query = serde_json::from_value(input)?;
    crate::require(query.group_by.as_deref().is_none_or(|g| GROUPS.contains(&g)), "groupBy must be workspace, lane, size, priority, needs, artifact, criteria, relations or container")?;
    let window = |at: Option<u64>| at.is_none_or(|at| query.from.is_none_or(|from| at >= from) && query.to.is_none_or(|to| at < to));
    let mut rows = Vec::new();
    for task in state.tasks.iter().chain(&state.archived) {
        let features = features(state, task);
        let outcomes = outcomes(task);
        if !window(outcomes["timing"]["createdAt"].as_u64()) || !query.filters.iter().all(|(key, value)| &features[key] == value) {
            continue;
        }
        rows.push(json!({"id":task.id,"title":task.title,"features":features,"outcomes":outcomes}));
    }
    let mut groups = BTreeMap::<String, Vec<&Value>>::new();
    for row in &rows {
        let key = query.group_by.as_ref().map_or("all".into(), |g| row["features"][g].to_string());
        groups.entry(key).or_default().push(row);
    }
    let mean = |rows: &[&Value], path: &str| {
        let values: Vec<f64> = rows.iter().filter_map(|r| r.pointer(path)?.as_f64()).collect();
        json!({"n":values.len(),"mean":(!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)})
    };
    let summary: BTreeMap<_, _> = groups
        .iter()
        .map(|(key, rows)| {
            (
                key.clone(),
                json!({"tasks":rows.len(),"firstPass":mean(rows, "/outcomes/firstPass"),"attempts":mean(rows, "/outcomes/attempts"),"rejections":mean(rows, "/outcomes/rejections"),
                "replans":mean(rows, "/outcomes/replans"),"leadMs":mean(rows, "/outcomes/timing/leadMs")}),
            )
        })
        .collect();
    Ok(json!({"metricDefinitionVersion":crate::history::METRIC_DEFINITION_VERSION,"groupBy":query.group_by,"groups":summary,"tasks":rows,
        "definitions":{"firstPass":"Done non-container task whose first implementation completed and first review accepted","replans":"Replan attempts, triggered by blocks or repeated failure of one criterion"}}))
}
