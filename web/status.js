export function workerStatus(agent, state, now = Date.now()) {
  const active = state.attempts.filter(
    (a) =>
      a.agent_id === agent.id &&
      ["running", "cancelling", "lost"].includes(a.status),
  );
  if (active.some((a) => a.status === "lost")) return "unknown";
  if (active.some((a) => a.status === "cancelling")) return "stopping";
  if (active.length) return "running";
  if (agent.worker) {
    if (
      now - Date.parse(agent.worker.seen_at) >
      agent.worker.lease_seconds * 1000
    )
      return "unknown";
    return agent.worker.assignment ? "assigned" : "idle";
  }
  const presence = agent.presence;
  if (!presence) return agent.enabled ? "idle" : "disabled";
  const reportedAttempt = state.attempts.find(
    (a) => a.agent_id === agent.id && a.id === presence.run_id,
  );
  const terminal = {
    completed: "completed",
    cancelled: "stopped",
    failed: "failed",
    send_back: "idle",
    blocked: "idle",
    needs_input: "idle",
  };
  const status =
    (reportedAttempt && terminal[reportedAttempt.status]) || presence.status;
  if (
    now - Date.parse(presence.seen_at) > presence.lease_seconds * 1000 &&
    !["completed", "failed", "stopped"].includes(status)
  )
    return "unknown";
  return status;
}

// Compare with the signature captured when the DOM was rendered. Recomputing
// both old and new snapshots at the current time hides lease-expiry changes.
export function coordinationSignature(state, now = Date.now()) {
  return JSON.stringify([
    state.coordinator?.error,
    state.coordinator?.mode,
    state.coordinator?.draining,
    state.dispatch,
    state.projects,
    state.agents.map((a) => [a.id, workerStatus(a, state, now)]),
  ]);
}

export function queueTaskState(task, state) {
  const attempt = state.attempts.find(
    (a) =>
      a.id === task.current_attempt &&
      ["running", "cancelling", "lost"].includes(a.status),
  );
  if (attempt)
    return {
      status: { running: "Running", cancelling: "Stopping", lost: "Unknown" }[
        attempt.status
      ],
      reason:
        task.blocked_reason ||
        {
          running: "Execution in progress",
          cancelling: "Waiting for stop acknowledgement",
          lost: "Worker heartbeat lost",
        }[attempt.status],
      next: "Inspect execution",
    };
  if (state.agents.some((a) => a.worker?.assignment?.task_id === task.id))
    return {
      status: "Assigned",
      reason: "Waiting for worker pickup",
      next: "Inspect assignment",
    };
  if (
    state.agents.some(
      (a) =>
        a.presence?.task_id === task.id && workerStatus(a, state) === "running",
    )
  )
    return {
      status: "Running (reported)",
      reason: task.blocked_reason || "Work reported; attempt not claimed",
      next: "Inspect work",
    };
  const repair = state.tasks?.find(
    (candidate) =>
      candidate.recovery_of?.task_id === task.id &&
      !candidate.recovery_of.resumed &&
      !candidate.archived &&
      candidate.status !== "completed",
  );
  if (repair)
    return {
      status: "Awaiting coordinator recovery",
      reason: `Coordinator-owned repair ${repair.key} is resolving this execution blocker.`,
      next: "Inspect recovery",
      target: repair.id,
    };
  if (task.recovery_of)
    return {
      status: "Coordinator recovery",
      reason: task.blocked_reason || state.queue_reasons?.[task.id] || "Queued for agent diagnosis, repair and independent review.",
      next: "Inspect recovery",
    };
  const reason =
    task.blocked_reason ||
    state.queue_reasons?.[task.id] ||
    (state.settings.paused ? "Dispatch paused" : "Ready for dispatch");
  return {
    status: task.status.replaceAll("_", " "),
    reason,
    next: reason.startsWith("Waiting for RL-")
      ? "View dependencies"
      : reason.includes("No eligible agent")
        ? "Connect agent"
        : task.status === "failed" && !state.settings.paused
          ? "Retry scheduled"
          : "Open task",
  };
}
