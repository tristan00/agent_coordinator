# Worker API v1

External runtimes execute assignments from Relay using the same [result contract](../README.md#command-adapter) as command agents. Select a project from `GET /api/projects` and use its `api_base` in place of `/api` in every path below. Fetch its `/bootstrap` and send the token as `x-relay-token` on mutations. Refresh the token after restart. A worker identity consists of `source` and `agent_id` within that project.

## Automatic assignments

Register each role with `POST /api/workers/register`:

~~~json
{"source":"runtime","agent_id":"builder","name":"Builder","stages":["implementation"],"lease_seconds":120}
~~~

The response contains `worker_id`. A separate reviewer identity satisfies independent-review routing. `model` is optional.

Poll `POST /api/workers/next` with `{"worker_id":"…","wait_seconds":20}`. Relay selects the task; the request contains no task ID.

| Response | Runtime action |
|---|---|
| `attempt: null` | Poll again after `retry_after_ms` |
| `next_action: execute` | Persist the attempt ID, execute `attempt.input`, heartbeat and submit the result |
| `next_action: stop_and_acknowledge` | Stop that execution and acknowledge through `/stopped` |

Long polls are capped at 30 seconds and one third of the worker lease. Inbox polling and execution heartbeats renew worker presence. `POST /api/workers/disconnect` with `worker_id` removes future assignments without acknowledging an active execution's termination.

## Execution lifecycle

| POST endpoint | Body / behavior |
|---|---|
| `/api/external/{id}/heartbeat` | Returns `status` and `cancel`; send at least every third of the lease interval |
| `/api/external/{id}/result` | Stage result; accepted completion releases eligible follow-up work |
| `/api/external/{id}/stopped` | Acknowledges that the underlying execution has stopped |

Assignments are durable and may be redelivered. A runtime keeps one receiver per identity, deduplicates execution by persisted attempt ID and retains results until acknowledged. Retrying a result is idempotent. Heartbeats continue during provider execution; `cancel: true` requests termination before `/stopped`.

Unreceived offers expire back into the queue without consuming an attempt. Started external attempts survive server restart. A lost heartbeat fences the workspace until stop acknowledgement. Reconnect uses the saved execution identity rather than starting it again. HTTP connection failures and upgrade-related 503 responses are retried with backoff.

Relay performs stage transitions, dependency checks, continuous failure recovery and sendbacks (unlimited by default). Paused projects stop new dispatch. Blocked tasks, approval gates and exhausted budgets remain visible without stopping unrelated eligible work. Manual `complete_stage` actions are recorded as human work; worker results carry attempt identity and usage.

## Explicit claims

Legacy integrations can use `POST /api/external/claim`:

~~~json
{"source":"runtime","agent_id":"builder","name":"Builder","stages":["implementation"],"request_id":"unique-dispatch-id","lease_seconds":120}
~~~

The response contains an `attempt` or null. Optional `task_id` selects a task explicitly, including while automatic dispatch is paused. Claims still enforce dependencies, budgets and review separation. They do not register an automatic inbox receiver.

Persist `request_id` before the request. Retransmission returns the original attempt, which may already be terminal; only a running attempt is executable. A new dispatch uses a new request ID.

## Activity reports

`POST /api/workers/report` accepts `source`, `agent_id`, `name`, `status`, `lease_seconds`, and optional `model`, `task_id`, `run_id` and `usage`. Status is `running`, `idle`, `waiting`, `completed`, `failed` or `stopped`. Reports expire with their lease and only affect activity display; they create no execution ownership or task completion.

Worker leases must be positive and have no fixed upper bound. A lease measures liveness between heartbeats; it is not an execution deadline. The coordinator still fences a lost execution until stop acknowledgement to prevent concurrent writes to the same directory.

## Planner callbacks

The originating planning runtime can register an independent callback receiver. Failures notify that receiver while recovery continues. See [Planner callbacks](PLANNER_CALLBACKS.md) for durable webhook/poll delivery, receipt acknowledgements and the generic command bridge.
