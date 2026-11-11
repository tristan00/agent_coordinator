//! Pull-based worker protocol. A local provider bridge may run inference anywhere.
use crate::{engine, model::*, store::*};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub struct Claim {
    pub source: String,
    pub agent_id: String,
    pub name: String,
    #[serde(default)]
    pub model: String,
    pub stages: Vec<String>,
    pub request_id: String,
    pub task_id: Option<String>,
    #[serde(default = "lease_default")]
    pub lease_seconds: u64,
}
fn lease_default() -> u64 {
    120
}
pub(crate) fn worker_key(source: &str, agent: &str) -> String {
    format!("external:{}", json!([source, agent]))
}
fn key(c: &Claim) -> String {
    worker_key(&c.source, &c.agent_id)
}

#[derive(Deserialize)]
pub struct Report {
    pub source: String,
    pub agent_id: String,
    pub name: String,
    #[serde(default)]
    pub model: String,
    pub status: String,
    pub run_id: Option<String>,
    pub task_id: Option<String>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default = "lease_default")]
    pub lease_seconds: u64,
}
pub fn report(s: &mut Snapshot, r: Report) -> AppResult<Agent> {
    if [&r.source, &r.agent_id, &r.name]
        .iter()
        .any(|v| v.trim().is_empty())
        || ![
            "running",
            "idle",
            "completed",
            "failed",
            "waiting",
            "stopped",
        ]
        .contains(&r.status.as_str())
        || r.lease_seconds == 0
    {
        return Err(bad("Invalid worker identity, status, or lease"));
    }
    if let Some(tid) = &r.task_id {
        if !s.tasks.iter().any(|t| &t.id == tid) {
            return Err(bad("Reported task does not exist"));
        }
    }
    let aid = worker_key(&r.source, &r.agent_id);
    let presence = Presence {
        source: r.source.clone(),
        status: r.status.clone(),
        seen_at: now(),
        lease_seconds: r.lease_seconds,
        run_id: r.run_id,
        task_id: r.task_id,
        usage: r.usage,
    };
    if let Some(a) = s.agents.iter_mut().find(|a| a.id == aid) {
        a.name = r.name;
        a.model = r.model;
        a.presence = Some(presence);
        return Ok(a.clone());
    }
    let a = Agent {
        id: aid,
        name: r.name,
        adapter: "external".into(),
        model: r.model,
        stages: vec![],
        instructions: String::new(),
        command: String::new(),
        args: vec![],
        unset_env: vec![],
        endpoint: r.source,
        enabled: true,
        capacity: 1,
        timeout_seconds: 0,
        created_at: now(),
        presence: Some(presence),
        worker: None,
    };
    engine::event(
        s,
        None,
        "agent_connected",
        &a.name,
        "Worker connected",
        json!({"agent_id":a.id}),
    );
    s.agents.push(a.clone());
    Ok(a)
}

pub fn claim(s: &mut Snapshot, mut c: Claim) -> AppResult<Option<Attempt>> {
    c.stages = normalize_stages(c.stages);
    for value in [&c.source, &c.agent_id, &c.name, &c.request_id] {
        if value.trim().is_empty() {
            return Err(bad("Worker identity and request ID are required"));
        }
    }
    if c.lease_seconds == 0
        || c.stages.is_empty()
        || c.stages
            .iter()
            .any(|x| !EXECUTABLE_STAGES.contains(&x.as_str()))
    {
        return Err(bad(
            "Provide executable stages and a positive lease duration",
        ));
    }
    let aid = key(&c);
    // Retransmission after a lost HTTP response must not start another task.
    if let Some(a) = s.attempts.iter().find(|a| {
        a.agent_id == aid
            && a.external
                .as_ref()
                .is_some_and(|e| e.request_id == c.request_id)
    }) {
        return Ok(Some(a.clone()));
    }
    let profile = Agent {
        id: aid.clone(),
        name: c.name,
        adapter: "external".into(),
        model: c.model,
        stages: c.stages,
        instructions: String::new(),
        command: String::new(),
        args: vec![],
        unset_env: vec![],
        endpoint: c.source.clone(),
        enabled: true,
        capacity: 1,
        timeout_seconds: 0,
        created_at: now(),
        presence: Some(Presence {
            source: c.source.clone(),
            status: "idle".into(),
            seen_at: now(),
            lease_seconds: c.lease_seconds,
            run_id: None,
            task_id: None,
            usage: Usage::default(),
        }),
        worker: s
            .agents
            .iter()
            .find(|a| a.id == aid)
            .and_then(|a| a.worker.clone()),
    };
    if let Some(existing) = s.agents.iter_mut().find(|a| a.id == aid) {
        let created = existing.created_at.clone();
        *existing = profile;
        existing.created_at = created;
    } else {
        engine::event(
            s,
            None,
            "agent_connected",
            &profile.name,
            "Worker connected",
            json!({"agent_id":aid,"source":c.source}),
        );
        s.agents.push(profile);
    }
    engine::reconcile(s)?;
    let result = engine::claim_for(s, c.task_id.as_deref(), Some(&aid))?;
    if let Some((mut attempt, _)) = result {
        attempt.external = Some(ExternalLease {
            source: c.source,
            request_id: c.request_id,
            heartbeat_at: now(),
            lease_seconds: c.lease_seconds,
        });
        *s.attempts.iter_mut().find(|a| a.id == attempt.id).unwrap() = attempt.clone();
        if let Some(p) = s
            .agents
            .iter_mut()
            .find(|a| a.id == aid)
            .and_then(|a| a.presence.as_mut())
        {
            p.status = "running".into();
            p.run_id = Some(attempt.id.clone());
            p.task_id = Some(attempt.task_id.clone());
        }
        Ok(Some(attempt))
    } else {
        Ok(None)
    }
}

pub fn heartbeat(s: &mut Snapshot, id: &str) -> AppResult<Value> {
    let a = s
        .attempts
        .iter_mut()
        .find(|a| a.id == id && a.external.is_some())
        .ok_or_else(|| AppError(404, "External attempt not found".into()))?;
    if a.status == "lost" {
        return Ok(json!({"status":"lost","cancel":true}));
    }
    if !["running", "cancelling"].contains(&a.status.as_str()) {
        return Ok(json!({"status":a.status,"cancel":true}));
    }
    a.external.as_mut().unwrap().heartbeat_at = now();
    let response = json!({"status":a.status,"cancel":a.status=="cancelling"});
    let aid = a.agent_id.clone();
    if let Some(w) = s
        .agents
        .iter_mut()
        .find(|a| a.id == aid)
        .and_then(|a| a.worker.as_mut())
    {
        w.seen_at = now();
    }
    if let Some(p) = s
        .agents
        .iter_mut()
        .find(|a| a.id == aid)
        .and_then(|a| a.presence.as_mut())
    {
        p.seen_at = now();
    }
    Ok(response)
}

pub fn acknowledge_stop(s: &mut Snapshot, id: &str) -> AppResult<()> {
    let i = s
        .attempts
        .iter()
        .position(|a| a.id == id && a.external.is_some())
        .ok_or_else(|| AppError(404, "External attempt not found".into()))?;
    if s.attempts[i].status == "lost" {
        s.attempts[i].status = "cancelling".into();
        let tid = s.attempts[i].task_id.clone();
        if let Some(t) = s
            .tasks
            .iter_mut()
            .find(|t| t.id == tid && t.current_attempt.as_deref() == Some(id))
        {
            t.current_attempt = None;
            t.status = "failed".into();
            t.revision = t.revision.saturating_add(1);
            t.updated_at = now();
        }
    }
    engine::fail(s, id, "Worker confirmed stopped", "")
}

/// A final attempt is authoritative over presence reports for that same run.
/// Preserve reports for unrelated work performed by the same worker.
pub fn reconcile_presence(s: &mut Snapshot) {
    for agent in &mut s.agents {
        let Some(presence) = &mut agent.presence else {
            continue;
        };
        let Some(attempt) = s
            .attempts
            .iter()
            .find(|a| a.agent_id == agent.id && presence.run_id.as_deref() == Some(a.id.as_str()))
        else {
            continue;
        };
        let status = match attempt.status.as_str() {
            "completed" => "completed",
            "cancelled" => "stopped",
            "failed" => "failed",
            "send_back" | "blocked" | "needs_input" => "idle",
            _ => continue,
        };
        presence.status = status.into();
        if let Some(finished_at) = &attempt.finished_at {
            presence.seen_at = finished_at.clone();
        }
        presence.usage = attempt.usage.clone();
    }
}

pub fn expire_leases(s: &mut Snapshot) -> AppResult<()> {
    for i in 0..s.attempts.len() {
        let a = &s.attempts[i];
        let Some(lease) = &a.external else {
            continue;
        };
        if !["running", "cancelling"].contains(&a.status.as_str())
            || engine::elapsed(&lease.heartbeat_at) <= lease.lease_seconds.saturating_mul(1000)
        {
            continue;
        }
        let aid = a.id.clone();
        let tid = a.task_id.clone();
        // Keep the workspace fenced until the worker confirms it has stopped.
        s.attempts[i].status = "lost".into();
        s.attempts[i].error = "Worker heartbeat lost; awaiting stop acknowledgement".into();
        if let Some(t) = s
            .tasks
            .iter_mut()
            .find(|t| t.id == tid && t.current_attempt.as_deref() == Some(&aid))
        {
            t.status = "needs_input".into();
            t.blocked_reason = s.attempts[i].error.clone();
            t.revision = t.revision.saturating_add(1);
            t.updated_at = now();
        }
        engine::event(
            s,
            Some(&tid),
            "worker_lost",
            "coordinator",
            "Worker heartbeat lost; workspace remains reserved",
            json!({"attempt_id":aid}),
        );
        if let Some(ti) = s.tasks.iter().position(|task| task.id == tid) {
            crate::callbacks::notify(
                s,
                ti,
                &aid,
                "Worker heartbeat lost; stop acknowledgement is required before releasing the execution directory",
            );
        }
    }
    Ok(())
}
