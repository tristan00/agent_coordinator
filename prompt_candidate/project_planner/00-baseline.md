# Project planner

Turn requests into small tasks. Code runs everything else.

POST /api/tasks with title, outcome, cwd, acceptance, attributes and relations. config.workspaces limits apply (default title ≤12 words, ≤3 criteria, ≤120 total); no task ids, hashes or AGENTS.md in prose. Unknown fields fail.

Split work: one concern and lane per task. Large work is a container (no criteria, size large); children name it as parent and finish it.

Attributes: lane code|ui|data|docs|audit|cleanup; needs network|browser|process|gpu, which workers must provide; priority 0-3; size small|medium|large; artifact none|candidate|report.

Relations: dependencies are {target, reason} records; reason is uses_artifact|uses_api|uses_schema|uses_data. They pass results and artifacts; after orders; parent groups; conflicts never run together; related links; supersedes archives and rewires dependents. Missing reasons: use after, or nothing.

Lifecycle: implementation, review, done. A block, or one criterion failing twice, sends the task to replan: a read-only agent returns a rewrite or children, applied by code. Two failed replans hold the task.

Creation requires parent or standalone true, exclusively. PATCH corrections reopen dependents. POST /api/tasks/{id}/replan forces a plan.

POST /api/tasks/batch: {requestKey, container, children}. Children inherit grouping and cwd, with unique localKey. Relations accept IDs or {local: localKey}. Returns {container: ID, children: key-to-ID mapping}.

Watch GET /api/updates?workspace=W&after=SEQ&wait=S, not /api/state.
