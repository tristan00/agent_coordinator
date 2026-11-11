//! Durable, coordinator-owned assignment queues for provider-neutral workers.
use crate::{engine, external, model::*, store::*};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct Registration {
    pub source: String,
    pub agent_id: String,
    pub name: String,
    #[serde(default)]
    pub model: String,
    pub stages: Vec<String>,
    #[serde(default = "lease_default")]
    pub lease_seconds: u64,
}
fn lease_default() -> u64 {
    120
}

pub fn register(s: &mut Snapshot, mut r: Registration) -> AppResult<Agent> {
    r.stages = normalize_stages(r.stages);
    if [&r.source, &r.agent_id, &r.name]
        .iter()
        .any(|v| v.trim().is_empty())
        || r.stages.is_empty()
        || r.stages
            .iter()
            .any(|v| !EXECUTABLE_STAGES.contains(&v.as_str()))
        || r.lease_seconds == 0
    {
        return Err(bad(
            "Provide worker identity, executable stages, and a positive lease duration",
        ));
    }
    let aid = external::worker_key(&r.source, &r.agent_id);
    if let Some(a) = s.agents.iter_mut().find(|a| a.id == aid) {
        a.name = r.name;
        a.model = r.model;
        a.stages = r.stages;
        a.enabled = true;
        let assignment = a.worker.as_mut().and_then(|w| w.assignment.take());
        a.worker = Some(WorkerRegistration {
            source: r.source,
            seen_at: now(),
            lease_seconds: r.lease_seconds,
            assignment,
        });
        return Ok(a.clone());
    }
    let a = Agent {
        id: aid,
        name: r.name,
        model: r.model,
        adapter: "external".into(),
        stages: r.stages,
        instructions: String::new(),
        command: String::new(),
        args: vec![],
        unset_env: vec![],
        endpoint: r.source.clone(),
        enabled: true,
        capacity: 1,
        timeout_seconds: 0,
        created_at: now(),
        presence: Some(Presence {
            source: r.source.clone(),
            status: "idle".into(),
            seen_at: now(),
            lease_seconds: r.lease_seconds,
            run_id: None,
            task_id: None,
            usage: Usage::default(),
        }),
        worker: Some(WorkerRegistration {
            source: r.source,
            seen_at: now(),
            lease_seconds: r.lease_seconds,
            assignment: None,
        }),
    };
    engine::event(
        s,
        None,
        "worker_registered",
        &a.name,
        "Worker connected for automatic assignments",
        json!({"agent_id":a.id}),
    );
    s.agents.push(a.clone());
    Ok(a)
}

pub fn connected(agent: &Agent) -> bool {
    agent.enabled
        && agent
            .worker
            .as_ref()
            .is_some_and(|w| engine::elapsed(&w.seen_at) <= w.lease_seconds.saturating_mul(1000))
}

pub fn reserved(s: &Snapshot, task: &Task) -> bool {
    s.agents
        .iter()
        .filter_map(|a| a.worker.as_ref()?.assignment.as_ref())
        .any(|offer| {
            if offer.task_id == task.id {
                return true;
            }
            if offer.workspace.is_empty() || task.workspace.is_empty() {
                return false;
            }
            match (
                std::fs::canonicalize(&offer.workspace),
                std::fs::canonicalize(&task.workspace),
            ) {
                (Ok(a), Ok(b)) => a.starts_with(&b) || b.starts_with(&a),
                _ => false,
            }
        })
}

fn valid(s: &Snapshot, offer: &Assignment) -> bool {
    s.tasks.iter().any(|t| {
        t.id == offer.task_id
            && !t.archived
            && t.status == "ready"
            && t.revision == offer.task_revision
            && t.stage == offer.stage
            && t.visit == offer.visit
    })
}

pub fn expire_assignments(s: &mut Snapshot) {
    for i in 0..s.agents.len() {
        let Some(offer) = s.agents[i]
            .worker
            .as_ref()
            .and_then(|w| w.assignment.clone())
        else {
            continue;
        };
        let w = s.agents[i].worker.as_ref().unwrap();
        if !connected(&s.agents[i])
            || !valid(s, &offer)
            || engine::elapsed(&offer.offered_at) > w.lease_seconds.saturating_mul(1000)
        {
            s.agents[i].worker.as_mut().unwrap().assignment = None;
            engine::event(
                s,
                Some(&offer.task_id),
                "assignment_released",
                "coordinator",
                "Unstarted assignment returned to queue",
                json!({"assignment_id":offer.id}),
            );
        }
    }
}

/// Scheduler decides task order and role routing even when no client is polling.
/// Offers are durable reservations, not attempts or proof that execution started.
pub fn dispatch(s: &mut Snapshot) -> AppResult<usize> {
    expire_assignments(s);
    if s.settings.paused {
        return Ok(0);
    }
    let mut tasks: Vec<_> = s
        .tasks
        .iter()
        .filter(|t| {
            !t.archived && t.status == "ready" && EXECUTABLE_STAGES.contains(&t.stage.as_str())
        })
        .cloned()
        .collect();
    tasks.sort_by_key(|t| (t.priority, t.created_at.clone()));
    let mut count = 0;
    for t in tasks {
        let worker = s
            .agents
            .iter()
            .filter(|a| connected(a) && a.worker.as_ref().is_some_and(|w| w.assignment.is_none()))
            .find(|a| engine::eligible_for(s, &t, Some(&a.id)).is_ok())
            .map(|a| a.id.clone());
        let Some(aid) = worker else { continue };
        let offer = Assignment {
            id: id(),
            task_id: t.id.clone(),
            task_revision: t.revision,
            stage: t.stage.clone(),
            visit: t.visit,
            workspace: t.workspace.clone(),
            offered_at: now(),
        };
        s.agents
            .iter_mut()
            .find(|a| a.id == aid)
            .unwrap()
            .worker
            .as_mut()
            .unwrap()
            .assignment = Some(offer.clone());
        engine::event(
            s,
            Some(&t.id),
            "task_assigned",
            "coordinator",
            "Assigned next stage to a registered worker",
            json!({"assignment_id":offer.id,"agent_id":aid,"stage":t.stage}),
        );
        count += 1;
    }
    Ok(count)
}

pub fn poll(s: &mut Snapshot, worker_id: &str) -> AppResult<()> {
    let agent = s
        .agents
        .iter_mut()
        .find(|a| a.id == worker_id && a.worker.is_some() && a.enabled)
        .ok_or_else(|| {
            AppError(
                404,
                "Worker is not registered for automatic assignments".into(),
            )
        })?;
    agent.worker.as_mut().unwrap().seen_at = now();
    if let Some(p) = &mut agent.presence {
        p.seen_at = now();
    }
    Ok(())
}

/// Repeated delivery returns the same active attempt. A worker persists its ID
/// before invoking a provider, then heartbeats and posts its result as usual.
pub fn receive(s: &mut Snapshot, worker_id: &str) -> AppResult<Option<Attempt>> {
    let i = s
        .agents
        .iter()
        .position(|a| a.id == worker_id && a.worker.is_some() && a.enabled)
        .ok_or_else(|| AppError(404, "Worker registration not found".into()))?;
    if let Some(a) = s.attempts.iter().find(|a| {
        a.agent_id == worker_id
            && a.external.is_some()
            && ["running", "cancelling", "lost"].contains(&a.status.as_str())
    }) {
        return Ok(Some(a.clone()));
    }
    if s.settings.paused {
        return Ok(None);
    }
    expire_assignments(s);
    let Some(offer) = s.agents[i].worker.as_mut().unwrap().assignment.take() else {
        return Ok(None);
    };
    let lease = s.agents[i].worker.as_ref().unwrap().clone();
    let claimed = engine::claim_for(s, Some(&offer.task_id), Some(worker_id));
    let (mut attempt, _) = match claimed {
        Ok(Some(run)) => run,
        Ok(None) => return Ok(None),
        Err(AppError(409, _)) => return Ok(None), // Readiness changed after the offer.
        Err(e) => return Err(e),
    };
    attempt.external = Some(ExternalLease {
        source: lease.source.clone(),
        request_id: offer.id,
        heartbeat_at: now(),
        lease_seconds: lease.lease_seconds,
    });
    *s.attempts.iter_mut().find(|a| a.id == attempt.id).unwrap() = attempt.clone();
    s.agents[i].presence = Some(Presence {
        source: lease.source,
        status: "running".into(),
        seen_at: now(),
        lease_seconds: lease.lease_seconds,
        run_id: Some(attempt.id.clone()),
        task_id: Some(attempt.task_id.clone()),
        usage: Usage::default(),
    });
    Ok(Some(attempt))
}

pub fn disconnect(s: &mut Snapshot, worker_id: &str) -> AppResult<()> {
    let a = s
        .agents
        .iter_mut()
        .find(|a| a.id == worker_id)
        .ok_or_else(|| AppError(404, "Worker not found".into()))?;
    a.worker = None;
    Ok(())
}
