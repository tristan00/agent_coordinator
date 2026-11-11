# Originating planner callbacks

Relay records a callback in the same SQLite transaction as a failed, blocked or input-requesting worker result. Recovery and callback delivery run independently. An unavailable receiver does not delay a repair agent. Delivery retries without an attempt limit, including after server restarts.

The protocol is HTTP/JSON and does not depend on Codex, Cursor, Claude or a particular local agent. A receiver must be connected to the originating planner's session; registering a worker profile alone does not register a planner callback. A callback is not a promise that the receiver can perform every repair.

Use the workspace's `api_base` from `GET /api/projects` as the prefix below. Fetch `/bootstrap` and send its `token` as `x-relay-token` on API mutations. Each workspace has separate registrations and delivery records.

## Register a receiver

`POST /planners/register`:

```json
{
  "source": "my-agent-runtime",
  "agent_id": "project-planner",
  "name": "Project planner",
  "context": {"conversation_id": "original-planning-session"},
  "callback_url": "http://127.0.0.1:8090/relay-callback",
  "workspace_default": true,
  "enabled": true,
  "task_ids": []
}
```

The response has `planner_id` and `planner`. Re-registering the same `source` and `agent_id` updates the receiver and preserves its identity and outbox. HTTPS callbacks are supported; plain HTTP is accepted on loopback. Redirects are not followed. The session token and approval code are never sent to the receiver.

Task creation and plan manifests accept `planner_id`. A task without an explicit receiver inherits its ancestor's receiver, then the workspace default. `task_ids` binds existing/imported tasks to this receiver without changing their stage. Repair-task callbacks resolve to the original task's planner. Only one receiver is the workspace default at a time. Re-registering with `enabled: false` disables delivery without discarding pending notifications.

Late registration queues the latest unresolved worker failure. A missing receiver is recorded as `planner_callback_unrouted`; Relay does not pretend a callback was delivered.

## Receive and acknowledge

Webhooks receive a body with `protocol_version`, `notification_id`, `kind`, planner identity/context, task and originating-task identities, attempt identity, reason, and related recovery task IDs. Full artifacts and logs remain in the workspace API and run directory. The receiver should persist the notification ID before acting and deduplicate redeliveries.

Return an explicit JSON receipt:

```json
{"acknowledged":"the-exact-notification-id"}
```

HTTP 200 alone does not acknowledge receipt. Connection failure, timeout, malformed acknowledgement or non-success status keeps the item pending. Delivery retries with a short increasing delay, capped at 30 seconds **between attempts**, with no total attempt limit. Transport timeouts let other deliveries proceed and do not stop task execution.

Receivers without an inbound HTTP endpoint can use `POST /planners/{planner_id}/next`. The response is `{"notification": {...}, "retry_after_ms":1000}` or `notification: null`. This polling endpoint rotates among pending items by least recent delivery, so one unsuccessful handler does not starve other incidents. Read `notification.payload` for the event body and `notification.payload.planner.context` for the originating session. Acknowledge through `POST /planners/{planner_id}/ack` with `{"notification_id":"..."}`. The acknowledgement endpoint is idempotent.

Acknowledgement records **receipt only**. It does not complete a task, resolve a blocker, pause recovery, or change execution permissions. If an acknowledged incident remains unresolved and no original/repair execution is running after five minutes, Relay queues a `recovery_follow_up` notification referring to the previous notification ID. Explicit workspace pause, disabled automatic advancement, task pause, cancellation and archival suppress these follow-ups.

An already-queued notification can still be delivered while workspace dispatch is paused. Stopping the server stops the dispatcher. Workspace reset clears registrations and outbox entries along with the board and preserves them in the reset backup.

## Run a command-based receiver

The generic bridge polls Relay and invokes a configured runtime command without a shell:

```sh
python scripts/planner_bridge.py --api-base http://127.0.0.1:4242/api/projects/ID --source local --agent-id planner --workspace-default --context session-id --command-json '["your-runtime","resume","{context}","{notification_file}"]'
```

Supply the resume command supported by the selected runtime. The bridge passes the event on stdin and through `RELAY_NOTIFICATION_FILE`, `RELAY_NOTIFICATION_ID`, `RELAY_PLANNER_CONTEXT`, `RELAY_API_BASE` and `RELAY_PLANNER_ID`. A successful command exit records receipt before acknowledging Relay. An unsuccessful command is retried; it does not complete or abandon the original task. Receipts survive bridge restarts. Runtime handlers must still deduplicate notification IDs because a crash between their side effect and the receipt commit can cause redelivery.

## Inspect delivery

`GET /state` and export include `planners` and `planner_notifications`. Delivery records include attempt count, last delivery time, next delivery time, last error and acknowledgement time. History records queueing, missing receiver and acknowledgement. These records distinguish notification failure from repair failure and task completion.
