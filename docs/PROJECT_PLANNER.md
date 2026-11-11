# Project planner contract

Use this contract for the agent that decomposes a project into a task graph. The board's Planning stage prepares one existing task for implementation.

1. Inspect the workspace and define the deliverable and interfaces. Split the work into small, cohesive tasks, each with one outcome, explicit inputs/outputs, and runnable acceptance checks. Size leaf tasks for one implementation/review cycle.
2. Group tasks under epics. Use subtasks when a task needs separately testable pieces; give the parent a concrete integration/checking outcome. Assign each entry a stable `code` and each child its `parent` code. Relay derives child lists from these links.
3. List prerequisite codes in `deps`. A dependency means its reviewed output is needed before this task starts. List prerequisites on each task that consumes them. Independent branches can run concurrently in separate workspaces. Relay runs a parent task after its children and completes an epic when its tasks finish.
4. Produce the JSON manifest below with existing absolute workspace paths. Use `kind: "subtask"` for children of tasks; other entries in `tasks` use `kind: "task"`. Set each task's type, priority (1–4), points (1, 2, 3, 5, 8 or 13), description, and acceptance checks. Describe interfaces and expected evidence in the task itself.
5. Select the project from `GET /api/projects`. Fetch its `api_base + "/bootstrap"` and use the token as `x-relay-token`. Send `{"plan": manifest}` to `/plan/preview` under the same base, resolve validation errors, then submit to `/plan/import`. For updates, include preview revisions in `expected_revisions`, keyed by code. Preserve codes across revisions. Retain the returned code-to-task-ID mapping.

```json
{
  "schema_version": 1,
  "plan_id": "calculator-v1",
  "workspaces": { "app": "/absolute/path/to/calculator" },
  "epics": [
    { "code": "E", "title": "Calculator", "description": "Deliver a tested calculator command", "workspace": "app" }
  ],
  "tasks": [
    { "code": "T", "kind": "task", "parent": "E", "title": "Integrate calculator", "description": "Check the command against the completed arithmetic module.", "workspace": "app", "points": 2, "priority": 2, "task_type": "feature", "deps": [], "acceptance": ["The end-to-end command tests pass."] },
    { "code": "S", "kind": "subtask", "parent": "T", "title": "Arithmetic module", "description": "Export add(a, b) and its unit tests.", "workspace": "app", "points": 2, "priority": 2, "task_type": "feature", "deps": [], "acceptance": ["Unit tests cover positive, negative and zero operands."] },
    { "code": "C", "kind": "subtask", "parent": "T", "title": "Command entry point", "description": "Consume add(a, b), parse two arguments and print the result.", "workspace": "app", "points": 2, "priority": 2, "task_type": "feature", "deps": ["S"], "acceptance": ["Running the command with 17 and 25 prints 42; invalid arguments exit nonzero."] }
  ]
}
```

Configure stage agents and leave the project's automatic workflow enabled. After import, Rust owns task selection, dependency release, stage handoffs, retries, and hierarchy completion. Workers receive prerequisite and child artifacts with their assignments. See the [API and execution settings](../README.md#http-api).

Execution is unlimited by default. Do not insert attempt, sendback, timeout or token budgets into a generated plan unless the user requested them. Set `execution.max_attempts`, `execution.max_sendbacks` and `execution.default_timeout_seconds` to `0` (or omit them) for unlimited execution. The optional manifest `planner_id` binds imported work to its registered planner receiver so blockers can call the originating planner.
