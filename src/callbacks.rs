//! Durable, provider-neutral delivery to the planner that originated a task.
//! Delivery is independent of scheduling and never gates worker progress.
use crate::{engine, model::*, store::*};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Deserialize)]
pub struct Registration {
    pub source: String,
    pub agent_id: String,
    pub name: String,
    #[serde(default)]
    pub callback_url: Option<String>,
    #[serde(default)]
    pub context: Value,
    #[serde(default)]
    pub workspace_default: bool,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub task_ids: Vec<String>,
}
fn enabled() -> bool {
    true
}

pub fn register(s: &mut Snapshot, request: Registration) -> AppResult<PlannerRegistration> {
    if request.source.trim().is_empty()
        || request.agent_id.trim().is_empty()
        || request.name.trim().is_empty()
    {
        return Err(bad("Planner source, agent_id and name are required"));
    }
    if let Some(endpoint) = &request.callback_url {
        let url = reqwest::Url::parse(endpoint).map_err(|_| bad("Invalid planner callback URL"))?;
        let local = url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if (url.scheme() != "https" && !(url.scheme() == "http" && local))
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(bad(
                "Planner callbacks require HTTPS or loopback HTTP, without URL credentials or fragments",
            ));
        }
    }
    for task_id in &request.task_ids {
        if !s.tasks.iter().any(|task| &task.id == task_id) {
            return Err(AppError(
                404,
                format!("Task {task_id} not found in this workspace"),
            ));
        }
    }
    let old = s
        .planners
        .iter()
        .find(|p| p.source == request.source && p.agent_id == request.agent_id);
    let planner = PlannerRegistration {
        id: old.map(|p| p.id.clone()).unwrap_or_else(id),
        created_at: old.map(|p| p.created_at.clone()).unwrap_or_else(now),
        source: request.source,
        agent_id: request.agent_id,
        name: request.name,
        callback_url: request.callback_url,
        context: request.context,
        workspace_default: request.workspace_default,
        enabled: request.enabled,
    };
    if planner.workspace_default {
        for registered in &mut s.planners {
            registered.workspace_default = false;
        }
    }
    if let Some(index) = s.planners.iter().position(|p| p.id == planner.id) {
        s.planners[index] = planner.clone();
    } else {
        s.planners.push(planner.clone());
    }
    for task in &mut s.tasks {
        if request.task_ids.contains(&task.id) && task.planner_id.as_ref() != Some(&planner.id) {
            task.planner_id = Some(planner.id.clone());
            engine::touch(task);
        }
    }
    engine::event(
        s,
        None,
        "planner_registered",
        &planner.source,
        format!("Registered planner receiver {}", planner.name),
        json!({"planner_id":planner.id,"workspace_default":planner.workspace_default}),
    );
    // Registering/reconnecting after a failure must not miss its notification.
    let unresolved: Vec<_> = s
        .tasks
        .iter()
        .enumerate()
        .filter(|(_, task)| {
            !task.archived && !["completed", "cancelled"].contains(&task.status.as_str())
        })
        .filter_map(|(index, task)| {
            s.attempts
                .iter()
                .rev()
                .find(|a| a.task_id == task.id)
                .filter(|a| {
                    ["blocked", "needs_input", "failed", "lost"].contains(&a.status.as_str())
                })
                .map(|a| (index, a.id.clone(), task.blocked_reason.clone()))
        })
        .collect();
    for (index, attempt, reason) in unresolved {
        notify(s, index, &attempt, &reason);
    }
    // Missing-route incidents may have no attempt at all. Reconnect also picks
    // up their still-current durable unrouted records.
    let mut seen = HashSet::new();
    let unrouted: Vec<_> = s
        .events
        .iter()
        .rev()
        .filter(|e| e.kind == "planner_callback_unrouted")
        .filter_map(|e| {
            let task_id = e.task_id.as_ref()?;
            if !seen.insert(task_id.clone()) {
                return None;
            }
            let index = s.tasks.iter().position(|t| {
                &t.id == task_id
                    && !t.archived
                    && !["completed", "cancelled"].contains(&t.status.as_str())
                    && e.data["visit"].as_u64() == Some(t.visit as u64)
            })?;
            Some((
                index,
                e.data["attempt_id"].as_str()?.to_owned(),
                e.data["reason"]
                    .as_str()
                    .unwrap_or("Planner callback was unavailable")
                    .to_owned(),
            ))
        })
        .collect();
    for (index, attempt, reason) in unrouted {
        notify(s, index, &attempt, &reason);
    }
    Ok(planner)
}

fn origin<'a>(s: &'a Snapshot, task: &'a Task) -> &'a Task {
    task.recovery_of
        .as_ref()
        .and_then(|incident| s.tasks.iter().find(|t| t.id == incident.task_id))
        .unwrap_or(task)
}
fn receiver<'a>(s: &'a Snapshot, task: &'a Task) -> Option<&'a PlannerRegistration> {
    let mut current = Some(origin(s, task));
    let mut visited = HashSet::new();
    while let Some(task) = current {
        if !visited.insert(&task.id) {
            break;
        }
        if let Some(id) = &task.planner_id {
            // An explicitly selected receiver never silently falls back to a different planner.
            return s.planners.iter().find(|p| &p.id == id && p.enabled);
        }
        current = task
            .parent_id
            .as_ref()
            .and_then(|id| s.tasks.iter().find(|p| &p.id == id));
    }
    s.planners.iter().find(|p| p.workspace_default && p.enabled)
}

pub fn notify(s: &mut Snapshot, task_index: usize, attempt_id: &str, reason: &str) {
    let Some(task) = s.tasks.get(task_index).cloned() else {
        return;
    };
    let Some(planner) = receiver(s, &task).cloned() else {
        if !s.events.iter().any(|event| {
            event.kind == "planner_callback_unrouted"
                && event.task_id.as_ref() == Some(&task.id)
                && event.data["attempt_id"] == attempt_id
        }) {
            engine::event(
                s,
                Some(&task.id),
                "planner_callback_unrouted",
                "coordinator",
                "No originating planner receiver is registered; recovery continues independently",
                json!({"attempt_id":attempt_id,"reason":reason,"visit":task.visit,"stage":task.stage}),
            );
        }
        return;
    };
    if s.planner_notifications
        .iter()
        .any(|n| n.planner_id == planner.id && n.task_id == task.id && n.attempt_id == attempt_id)
    {
        return;
    }
    let original = origin(s, &task);
    let original_id = original.id.clone();
    let attempt = s.attempts.iter().find(|attempt| attempt.id == attempt_id);
    let payload = json!({
        "protocol_version":1, "kind":"worker_blocked", "workspace_name":s.settings.project_name,
        "planner":{"id":planner.id,"source":planner.source,"agent_id":planner.agent_id,"context":planner.context},
        "task":{"id":task.id,"key":task.key,"title":task.title,"stage":task.stage,"status":task.status},
        "origin_task":{"id":original.id,"key":original.key,"title":original.title},
        "attempt":{"id":attempt_id,"stage":attempt.map(|a| a.stage.as_str()),"status":attempt.map(|a| a.status.as_str()),"agent_id":attempt.map(|a| a.agent_id.as_str())},
        "reason":reason,
        "recovery_task_ids":s.tasks.iter().filter(|t|t.recovery_of.as_ref().is_some_and(|r|r.task_id==original_id)).map(|t|t.id.clone()).collect::<Vec<_>>(),
        "instruction":"Investigate the blocker and help the recovery agent make progress. Acknowledge receipt separately; receipt does not resolve the incident or complete the task."
    });
    enqueue(
        s,
        &planner.id,
        &task.id,
        &original_id,
        attempt_id,
        reason,
        payload,
    );
}

fn enqueue(
    s: &mut Snapshot,
    planner_id: &str,
    task_id: &str,
    origin_task_id: &str,
    attempt_id: &str,
    reason: &str,
    mut payload: Value,
) {
    let notification_id = id();
    let at = now();
    payload["notification_id"] = json!(notification_id);
    payload["created_at"] = json!(at);
    s.planner_notifications.push(PlannerNotification {
        id: notification_id.clone(),
        planner_id: planner_id.into(),
        task_id: task_id.into(),
        origin_task_id: origin_task_id.into(),
        attempt_id: attempt_id.into(),
        reason: reason.into(),
        payload,
        created_at: at.clone(),
        last_delivery_at: None,
        delivery_attempts: 0,
        next_delivery_at: at,
        acknowledged_at: None,
        last_error: String::new(),
    });
    engine::event(
        s,
        Some(task_id),
        "planner_callback_queued",
        "coordinator",
        "Queued callback to the originating planner",
        json!({"planner_id":planner_id,"notification_id":notification_id,"attempt_id":attempt_id}),
    );
}

pub fn acknowledge(
    s: &mut Snapshot,
    planner_id: &str,
    notification_id: &str,
) -> AppResult<PlannerNotification> {
    let index = s
        .planner_notifications
        .iter()
        .position(|n| n.id == notification_id && n.planner_id == planner_id)
        .ok_or_else(|| {
            AppError(
                404,
                "Planner notification not found in this workspace".into(),
            )
        })?;
    if s.planner_notifications[index].acknowledged_at.is_none() {
        s.planner_notifications[index].acknowledged_at = Some(now());
        s.planner_notifications[index].last_error.clear();
        let notification = s.planner_notifications[index].clone();
        engine::event(
            s,
            Some(&notification.task_id),
            "planner_callback_acknowledged",
            "planner",
            "Planner acknowledged receipt; recovery remains independent",
            json!({"planner_id":planner_id,"notification_id":notification_id}),
        );
    }
    Ok(s.planner_notifications[index].clone())
}

/// Pull receivers may reconnect and receive the same unacknowledged notification.
pub fn next(s: &mut Snapshot, planner_id: &str) -> AppResult<Option<PlannerNotification>> {
    if !s.planners.iter().any(|p| p.id == planner_id && p.enabled) {
        return Err(AppError(
            404,
            "Enabled planner receiver not found in this workspace".into(),
        ));
    }
    let notification = s
        .planner_notifications
        .iter_mut()
        .filter(|n| n.planner_id == planner_id && n.acknowledged_at.is_none())
        .min_by(|a, b| {
            a.last_delivery_at
                .cmp(&b.last_delivery_at)
                .then(a.delivery_attempts.cmp(&b.delivery_attempts))
        });
    if let Some(n) = notification {
        n.delivery_attempts = n.delivery_attempts.saturating_add(1);
        n.last_delivery_at = Some(now());
        Ok(Some(n.clone()))
    } else {
        Ok(None)
    }
}

fn elapsed(at: &str) -> i64 {
    DateTime::parse_from_rfc3339(at)
        .map(|at| (Utc::now() - at.with_timezone(&Utc)).num_seconds())
        .unwrap_or(i64::MAX)
}

/// Receipt does not dispose of an unresolved incident. If neither the original
/// nor its repair is running, remind the planner instead of silently stopping.
pub fn remind_unresolved(s: &mut Snapshot) {
    if s.settings.paused || !s.settings.auto_advance {
        return;
    }
    let mut seen = HashSet::new();
    let reminders: Vec<_> = s
        .planner_notifications
        .iter()
        .rev()
        .filter(|n| seen.insert((n.planner_id.clone(), n.origin_task_id.clone())))
        .filter(|n| {
            n.acknowledged_at
                .as_ref()
                .is_some_and(|at| elapsed(at) >= 300)
        })
        .filter(|n| s.planners.iter().any(|p| p.id == n.planner_id && p.enabled))
        .filter(|n| {
            s.tasks.iter().any(|t| {
                t.id == n.origin_task_id
                    && !t.archived
                    && !["completed", "cancelled", "paused"].contains(&t.status.as_str())
            })
        })
        .filter(|n| {
            !s.tasks.iter().any(|t| {
                (t.id == n.origin_task_id
                    || t.recovery_of
                        .as_ref()
                        .is_some_and(|r| r.task_id == n.origin_task_id))
                    && t.current_attempt.as_ref().is_some_and(|id| {
                        s.attempts.iter().any(|a| {
                            &a.id == id && ["running", "cancelling"].contains(&a.status.as_str())
                        })
                    })
            })
        })
        .cloned()
        .collect();
    for previous in reminders {
        let mut payload = previous.payload.clone();
        payload["kind"] = json!("recovery_follow_up");
        payload["previous_notification_id"] = json!(previous.id);
        payload["instruction"] = json!(
            "The incident is still unresolved and no related execution is running. Continue investigating and arrange the next recovery action. This reminder does not require a user response."
        );
        enqueue(
            s,
            &previous.planner_id,
            &previous.task_id,
            &previous.origin_task_id,
            &previous.attempt_id,
            &previous.reason,
            payload,
        );
    }
}

fn claim_due(s: &mut Snapshot) -> Vec<(PlannerNotification, String)> {
    remind_unresolved(s);
    let mut work = Vec::new();
    for notification in &mut s.planner_notifications {
        if notification.acknowledged_at.is_some() || elapsed(&notification.next_delivery_at) < 0 {
            continue;
        }
        let Some(endpoint) = s
            .planners
            .iter()
            .find(|p| p.id == notification.planner_id && p.enabled)
            .and_then(|p| p.callback_url.clone())
        else {
            continue;
        };
        notification.delivery_attempts = notification.delivery_attempts.saturating_add(1);
        notification.last_delivery_at = Some(now());
        // A durable short delivery lease survives crashes; it is never a retry ceiling.
        notification.next_delivery_at = (Utc::now() + ChronoDuration::seconds(30)).to_rfc3339();
        work.push((notification.clone(), endpoint));
    }
    work
}

async fn deliver(
    client: reqwest::Client,
    store: Arc<Store>,
    notification: PlannerNotification,
    endpoint: String,
) {
    let receipt = async {
        let response = client
            .post(endpoint)
            .json(&notification.payload)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("Callback returned HTTP {}", response.status()));
        }
        let value: Value = response
            .json()
            .await
            .map_err(|e| format!("Callback did not return a JSON acknowledgement: {e}"))?;
        if value["acknowledged"].as_str() != Some(&notification.id) {
            return Err("Callback has not acknowledged this notification ID".to_owned());
        }
        Ok(())
    }
    .await;
    let result = store.mutate(|s| {
        // Reset may have cleared the outbox while the HTTP request was in flight.
        if !s
            .planner_notifications
            .iter()
            .any(|n| n.id == notification.id)
        {
            return Ok(());
        }
        match receipt {
            Ok(()) => {
                acknowledge(s, &notification.planner_id, &notification.id)?;
            }
            Err(message) => {
                let entry = s
                    .planner_notifications
                    .iter_mut()
                    .find(|n| n.id == notification.id)
                    .unwrap();
                if entry.acknowledged_at.is_none() {
                    entry.last_error = message;
                    let delay = (1i64 << entry.delivery_attempts.saturating_sub(1).min(5)).min(30);
                    entry.next_delivery_at =
                        (Utc::now() + ChronoDuration::seconds(delay)).to_rfc3339();
                }
            }
        }
        Ok(())
    });
    if let Err(error) = result {
        eprintln!("Could not persist planner delivery state; delivery will retry: {error}");
    }
}

/// Runs separately from the scheduler; a stalled receiver cannot hold a worker.
pub fn start_dispatcher(store: Arc<Store>, stopping: Arc<AtomicBool>) {
    tokio::spawn(async move {
        let client = loop {
            match reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .timeout(Duration::from_secs(10))
                .build()
            {
                Ok(client) => break client,
                Err(error) => {
                    eprintln!("Planner callback transport unavailable; retrying: {error}")
                }
            }
            if stopping.load(Ordering::Relaxed) {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        };
        let mut deliveries = tokio::task::JoinSet::new();
        while !stopping.load(Ordering::Relaxed) {
            match store.mutate(|s| Ok(claim_due(s))) {
                Ok(work) => {
                    for (notification, endpoint) in work {
                        deliveries.spawn(deliver(
                            client.clone(),
                            store.clone(),
                            notification,
                            endpoint,
                        ));
                    }
                }
                Err(error) => eprintln!("Planner outbox unavailable; retrying: {error}"),
            }
            while deliveries.try_join_next().is_some() {}
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        deliveries.abort_all();
    });
}
