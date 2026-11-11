import { test } from "node:test";
import assert from "node:assert/strict";
import {
  workerStatus,
  coordinationSignature,
  queueTaskState,
} from "../../web/status.js";

function state() {
  return {
    revision: 1,
    coordinator: { mode: "running", checked_at: "old" },
    attempts: [],
    agents: [
      {
        id: "worker",
        enabled: true,
        presence: {
          status: "running",
          run_id: "first",
          seen_at: "2026-11-10T00:00:00Z",
          lease_seconds: 120,
        },
      },
    ],
  };
}
const start = Date.parse("2026-11-10T00:00:00Z");

test("machine recovery links blocked work to its coordinator-owned repair", () => {
  const original = { id: "original", status: "blocked" };
  const repair = { id: "repair", key: "RL-012", status: "ready", recovery_of: { task_id: "original", resumed: false } };
  const snapshot = { tasks: [original, repair], agents: [], attempts: [], settings: { paused: false } };
  assert.deepEqual(queueTaskState(original, snapshot), {
    status: "Awaiting coordinator recovery",
    reason: "Coordinator-owned repair RL-012 is resolving this execution blocker.",
    next: "Inspect recovery", target: "repair",
  });
  assert.equal(queueTaskState(repair, snapshot).status, "Coordinator recovery");
});

test("claimed execution takes priority over a presence report for the same task", () => {
  const s = state();
  const task = {
    id: "task",
    current_attempt: "first",
    status: "running",
    blocked_reason: "",
  };
  s.agents[0].presence.task_id = task.id;
  s.attempts.push({ id: "first", agent_id: "worker", status: "running" });
  assert.equal(queueTaskState(task, s).status, "Running");
  assert.equal(queueTaskState(task, s).reason, "Execution in progress");
  s.attempts[0].status = "cancelling";
  assert.equal(queueTaskState(task, s).status, "Stopping");
});

test("worker expiry changes the rendered signature without a database change", () => {
  const s = state();
  const rendered = coordinationSignature(s, start);
  assert.equal(workerStatus(s.agents[0], s, start), "running");
  assert.equal(workerStatus(s.agents[0], s, start + 121000), "unknown");
  assert.notEqual(coordinationSignature(s, start + 121000), rendered);
  s.coordinator.checked_at = "new";
  assert.equal(coordinationSignature(s, start), rendered);
});

test("a confirmed stop immediately overrides a stale running report", () => {
  const s = state();
  s.attempts.push({ id: "first", agent_id: "worker", status: "cancelled" });
  assert.equal(workerStatus(s.agents[0], s, start), "stopped");
  assert.equal(workerStatus(s.agents[0], s, start + 121000), "stopped");
  s.agents[0].presence.run_id = "unrelated-new-work";
  assert.equal(workerStatus(s.agents[0], s, start), "running");
});

test("an unresolved lost attempt remains unknown until stop acknowledgement", () => {
  const s = state();
  s.attempts.push({ id: "first", agent_id: "worker", status: "lost" });
  assert.equal(workerStatus(s.agents[0], s, start), "unknown");
  s.attempts[0].status = "cancelling";
  assert.equal(workerStatus(s.agents[0], s, start), "stopping");
});

test("automatic assignment is not falsely counted as running execution", () => {
  const s = state();
  s.agents[0].worker = {
    seen_at: s.agents[0].presence.seen_at,
    lease_seconds: 120,
    assignment: { task_id: "queued" },
  };
  assert.equal(workerStatus(s.agents[0], s, start), "assigned");
  s.agents[0].worker.assignment = null;
  assert.equal(workerStatus(s.agents[0], s, start), "idle");
  assert.equal(workerStatus(s.agents[0], s, start + 121000), "unknown");
});
