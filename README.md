# Relay

A Rust task board and agent scheduler for one machine, with embedded SQLite and browser assets. The server needs no Python or Node runtime.

## Run

~~~sh
cargo build --release --locked
target/release/relay-coordinator --data-dir ./data --port 4242
~~~

On this Windows checkout:

~~~powershell
.\scripts\build.ps1 -Release
.\scripts\run.ps1
~~~

Open **http://127.0.0.1:4242**. Use **All workspaces** for aggregate metrics and **Global settings** for server maintenance and browser appearance. Select a named workspace for its task board, agents, queue, metrics, activity and archive. **Workspace settings** groups General, Workflow and Data controls. Each workspace has its own tasks, workers, settings, history and scheduler. Separate browser tabs and API clients select workspaces independently. The Windows helper detects the portable compiler in `.tools`; other installations need their platform's Rust and C toolchains. Windows was tested locally; macOS and Linux are configured in CI.

## Tasks and agents

1. Add a command agent in **Agents**, or connect an external runtime through the [worker API](docs/WORKER_PROTOCOL.md).
2. Choose stage agents in **Workspace settings / Workflow**. Independent review requires a different agent profile from implementation.
3. Create epics, tasks and subtasks with acceptance criteria, dependencies, priorities and optional execution directories. **Queue & blockers** explains waiting work; the task drawer contains attempts, artifacts and history.

The scheduler advances **Backlog → Planning → In progress → Review → Done**, selects eligible workers, releases dependent tasks and continues recovery after failures. Attempts, sendbacks and execution time are unlimited by default; positive values explicitly configured on a task or agent opt into limits. Parent tasks wait for their children; epics complete automatically. A sendback starts a new stage visit and preserves earlier results. Workspace pause stops new dispatch; running attempts continue. New workspaces start unpaused, and restarts preserve that setting.

The [Project planner contract](docs/PROJECT_PLANNER.md) guides initial decomposition into modular tasks, subtasks and dependencies. The board's Planning stage prepares an individual task for implementation.

Changing requirements returns started work to Planning and supersedes its evidence. Reopening dependencies invalidates downstream completion. Equal or nested execution directories are serialized across workspaces; separate directories can run concurrently. Directory paths do not restrict OS permissions. Git worktree creation and merging are not included.

## Command adapter

Relay launches an executable directly with an argument array in the task execution directory, or `data/runs/<attempt-id>` when none is set. It supplies handoff JSON on stdin containing the task, stage, configured instructions, artifacts, dependencies and feedback.

An agent's optional `unset_env` array removes named inherited environment variables before launch. The [result JSON schema](schemas/result.schema.json) supports CLIs with structured output.

| Argument placeholder | Value |
|---|---|
| `{input}` | Handoff JSON file |
| `{output}` | Result JSON file |
| `{workspace}` | Working directory |
| `{prompt_file}` | Prompt file |
| `{prompt}` | Prompt text |

`RELAY_INPUT`, `RELAY_OUTPUT` and `RELAY_ATTEMPT_ID` expose the same paths and attempt identity. A compatible executable returns the result on stdout, as its final JSON line, or at `RELAY_OUTPUT`:

~~~json
{
  "outcome": "complete",
  "summary": "Description of the completed stage",
  "artifacts": [
    {"name": "result.txt", "content": "Deliverable or evidence"}
  ],
  "model": null,
  "usage": {
    "input_tokens": null,
    "output_tokens": null,
    "cost_usd": null,
    "source": "unavailable"
  }
}
~~~

The result contract applies to command and external workers:

- `complete`: nonempty summary and at least one nonempty artifact.
- `send_back`: a reason and an earlier executable `send_back_to` stage.
- `blocked`, `needs_input`, `failed`: a reason.
- Optional planning `estimate`: `points` (1, 2, 3, 5, 8 or 13), `confidence` (`low`, `medium`, `high`) and `reason`.
- Optional usage: `reported`, `estimated` or `unavailable`; unknown values are null. Model and usage are worker-reported.

Command output streams to run log files without a size-based failure. Artifact bodies, result files and API payloads have no application size ceiling. Command completion requires process exit and a valid result. Remaining descendants are terminated when execution ends. A timeout of `0` disables the timeout; if positive task or agent timeouts are explicitly configured, the smaller positive value applies. Attempts and sendbacks also use `0` for unlimited. Token budgets are optional and use recorded totals between attempts; `null` or `0` disables the budget.

The optional **Ollama** adapter calls a local `/api/chat` endpoint, defaulting to `http://127.0.0.1:11434`. It generates text artifacts and has no filesystem or command tools. Ollama and its models are installed separately.

## HTTP API

`GET /api/projects` lists workspace IDs and `api_base` paths. `POST /api/projects` with `{"name":"Coordinator"}` creates a board. The API retains the `projects` name for compatibility. Prefix workspace endpoints below with that project's `api_base` in place of `/api`, for example `/api/projects/{id}/tasks`. The unscoped `/api/...` endpoints address the default board, except the explicitly global endpoints. `GET /api/bootstrap` returns the build ID and shared session token. Mutations use `x-relay-token`; tokens change on restart. The API accepts loopback hosts and same-origin browser requests. It trusts local processes under the same OS account.

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/state` | Tasks, attempts, artifacts, agents, events and queue state |
| POST | `/api/tasks` | Create task |
| PATCH | `/api/tasks/{id}` | Edit using its current `revision` |
| POST | `/api/tasks/{id}/action` | Start, pause, cancel, resume, retry, send back, archive, restore, comment or manually complete a stage |
| POST | `/api/tasks/{id}/run` | Run a stage using its current `revision` |
| POST | `/api/agents` | Create agent profile |
| PATCH | `/api/agents/{id}` | Update agent profile |
| PATCH | `/api/settings` | Update workspace settings; partial fields and optional `expected_revision` are supported |
| GET | `/api/projects/{id}/metrics` | Workspace metric summary with usage coverage |
| GET | `/api/overview` | Aggregate metrics and summaries for all workspaces |
| GET / PATCH | `/api/global/settings` | Host diagnostics and boolean `draining` maintenance control |
| POST | `/api/plan/preview` | Validate a plan and preview changes |
| POST | `/api/plan/import` | Apply a plan atomically |
| POST | `/api/backup` | Create SQLite backup |
| GET | `/api/export` | Export workspace JSON |
| POST | `/api/admin/drain` | Set `draining` to stop or resume admissions across all projects during upgrades |

Worker endpoints and reconnect semantics are in [WORKER_PROTOCOL.md](docs/WORKER_PROTOCOL.md).

### Coordinator owned recovery

The first worker failure, blocker or input request creates a linked recovery task and independently queues a callback to the originating planner, when registered. Recovery starts immediately rather than waiting for retries to exhaust. Planning hands off an actionable repair plan; it does not have to execute the repair or prove that it worked. Implementation performs the repair, and Review evaluates the result under the workspace's existing workflow. No additional diagnosis schema or verification gate is imposed.

If a recovery attempt fails, the scheduler continues with the same incident and its history, changes stage when a suitable agent is available, and prefers another capable profile. Recovery has no attempt, sendback or execution-time ceiling. A reviewed repair queues the original task to continue its own work; a recurring failure reopens the same recovery task. A planning agent that cannot act can hand the investigation to Implementation. Recovery does not claim that an unresolved task is complete.

Planner callbacks use durable HTTP delivery or a polling receiver, independently of recovery execution. Delivery retries until the receiver acknowledges the notification ID; acknowledgement records receipt only. Unresolved incidents with no related execution receive follow-up notifications. Any runtime can implement the [planner callback protocol](docs/PLANNER_CALLBACKS.md), or use `scripts/planner_bridge.py` to invoke its provider-specific command. A receiver must be registered before callbacks can reach it; a missing receiver is recorded while recovery continues.

Explicit pause, cancel, archive, approval instructions and opt-in original-task token budgets remain in effect. Recovery preserves execution-directory ownership and existing access controls. An unavailable worker or permission remains visible to recovery and the registered planner. Task drawers and the queue link original work to its repair. See the [execution limit audit](docs/EXECUTION_LIMIT_AUDIT.md) for removed failure caps and retained controls.

For an explicitly authorized reset of a named workspace, `POST /api/projects/{id}/reset` requires the session token and `{ "project_id": "<exact id>", "confirm_name": "<exact name>" }`. Active runs, lost-worker fences and outstanding assignments prevent reset. Relay creates a backup, then transactionally clears only that board's tasks, agents, attempts, artifacts, events and settings while preserving its name and ID. Source files and execution files are untouched. The default workspace cannot be reset through this endpoint.

### Workspace and global scope

Workspace pages use `?project=<id>&page=<page>`. Global pages use `?scope=global&page=overview` or `?scope=global&page=global-settings`. Old project links and default-board API clients continue to work.

Global maintenance stops new admissions across every workspace; running work continues. It does not change workspace pause preferences and clears on restart. Theme is a preference for this browser. Backup/export remain workspace-specific; preserving the complete data directory is necessary to restore all workspaces.

Task totals exclude archived tasks and epics. Attempt history, execution time, sendbacks and reported usage retain historical work. Unknown token/cost values are null; coverage counters show how many attempts supplied each value, so partial totals are not represented as complete usage. Global summaries sum the same per-workspace definitions and do not include task artifacts or execution logs.

Workspace settings accept partial updates. Send the snapshot `revision` as `expected_revision` to reject stale saves with HTTP 409. Workspace names are trimmed, nonempty, and unique ignoring case. Existing full-settings clients remain supported. The [multi-workspace architecture and acceptance plan](docs/MULTI_WORKSPACE_PLAN.md) records setting ownership and links to the proposed navigation prototype.

### Plan import

Both endpoints accept `plan` and optional `expected_revisions` and `adopt` maps. A plan has `schema_version: 1`, a stable `plan_id`, `workspaces` mapping aliases to existing absolute directories, and `epics`/`tasks` arrays. Each entry has a unique `code`, `title`, `description` and workspace alias. Tasks also have `points`, `task_type`, `priority` and `acceptance`; optional fields include `kind` (`task` or `subtask`), `parent`, `deps`, `constraints` and `human_approval`. `epic` remains an alias for `parent`. `planner_id` optionally binds every imported task to a registered planner receiver. `execution` optionally supplies timeout, attempt, sendback and token limits; omitted execution limits are unlimited. Rust validates references, hierarchy types and cycles before importing atomically.

Preview returns create/adopt/update/unchanged operations. Imports of changed entries use preview revisions in `expected_revisions`, keyed by code. `adopt` maps codes to existing task UUIDs with matching hierarchy, dependencies and approval mode. Unchanged imports preserve execution history; omitted entries remain on the board.

### Approval gates

Tasks with `human_approval: true` release dependents after **Record approval**. **Reopen approval** invalidates dependent completion. API actions `approve` and `reopen_approval` require the current revision, a reason and `x-relay-approval`.

Interactive startup prints an operator code. For redirected startup, `--approval-code-stdin` reads a randomly generated code of at least 24 printable, non-space ASCII characters. It remains in server memory and is absent from worker handoffs. Without either startup method, protected approvals remain unavailable.

## Persistence and upgrades

The data directory holds the default board's `relay.sqlite`, `relay.lock`, `runs/<attempt>/` and `backups/`. Additional boards use `projects/<id>/` with the same layout. Existing single-board data opens as the default project. Database backups include artifacts/history; execution files are separate. Restore a backup as `relay.sqlite` in a new data directory, then launch with `--data-dir` pointing there. Preserve the complete data directory to restore all projects.

Interrupted command attempts follow the retry policy. External attempts retain their leases and workspace ownership until their runtime acknowledges stopping; see the worker protocol. Cancellation cannot undo completed filesystem changes.

`--version` and `/api/bootstrap` identify the server build. On Windows, `scripts/upgrade.ps1 -Port 4242` backs up every project, drains the server, replaces it and verifies preserved records.

## Development

~~~sh
cargo fmt --all -- --check
cargo test --locked
cargo build --release --locked
~~~

`scripts/build.ps1 -Test` selects this checkout's portable Windows toolchain. Frontend component changes use `pnpm install --frozen-lockfile` and `pnpm build` in `frontend/`; rebuild Rust to embed the generated assets. See the [frontend build notes](frontend/README.md).
