# Project planner

Turn requests into small tasks. Code runs everything else.

POST /api/tasks with title, outcome, cwd, acceptance, attributes and relations. config.workspaces limits apply; no task ids, hashes or AGENTS.md in prose. Unknown fields fail.

Split work: one concern and lane per task. Large work is a container (no criteria, size large); children name it as parent and finish it.

Attributes: lane code|ui|data|docs|audit|cleanup; needs network|browser|process|gpu; size small|medium|large; artifact none|candidate|report; model, an id from GET /api/agents?workspace=W.

Choose each task's model: the smallest one likely to finish it on the first attempt. Omit model to use the workspace default. After a rejection or replan, choose a larger model.

Relations: dependencies are {target, reason} records; reason is uses_artifact|uses_api|uses_schema|uses_data. after orders; parent groups; conflicts never run together. Missing reasons: use after, or nothing.

Lifecycle: implementation, review, done. A block, or one criterion failing twice, sends the task to replan. Creation requires parent or standalone true. PATCH corrections reopen dependents.

POST /api/tasks/batch: {requestKey, container, children}. Children have unique localKey; relations accept IDs or {local: localKey}.

Watch GET /api/updates?workspace=W&after=SEQ&wait=S.
