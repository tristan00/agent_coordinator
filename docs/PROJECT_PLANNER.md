# Project planner

Turn requests into small tasks. Code runs everything else.

POST /api/tasks with title, outcome, cwd, acceptance, attributes and relations. Enforced: title ≤12 words, ≤3 criteria, ≤120 words total, no task ids, hashes or AGENTS.md in prose. Unknown fields fail.

Split work: one concern and lane per task. Large work is a container (no criteria, size large); children name it as parent and finish it.

Attributes: lane code|ui|data|docs|audit|cleanup; needs network|browser|process|gpu, which workers must provide; priority 0-3; size small|medium|large; artifact none|candidate|report; deploy for candidates.

Relations: dependencies are {target, reason} records; reason is uses_artifact|uses_api|uses_schema|uses_data (published in versions.dependencyReasons). They pass results and artifacts; after orders; parent groups; conflicts never run together; related links; supersedes archives and rewires dependents. Writable paths serialize writers. Missing reasons: use after, or nothing. Schema 4 keeps legacy null reasons, blocking dispatch until classified; recorded relationship-plan.json decisions apply during migration.

Lifecycle: implementation, review, done. A block, or one criterion failing twice, sends the task to replan: a read-only agent returns a rewrite or children, applied by code. Two failed replans hold the task; resume clears it. Usage limits pause dispatch until reset.

Creation requires parent or standalone true, exclusively. Omitted grouping survives PATCH; legacy missing choices block dispatch until classified. PATCH corrections reopen dependents. POST /api/tasks/{id}/replan forces a plan. Board stop and resume require actor human. Accepted deploy tasks deploy automatically. The notify command receives held, usage and deploy events.

Taskless redeploy: agent-coordinator.exe --redeploy SOURCE_DIR|EXE|recover, or POST /api/deployment {source|candidate|recover:true} polling its receipt. Pauses and workers survive; failures roll back.

POST /api/tasks/batch: {requestKey, container, children}. Container requires explicit grouping; children inherit it and cwd, with unique localKey. Relations accept IDs or {local: localKey}. Returns {container: ID, children: key-to-ID mapping}. Tasks and history commit atomically. Exact replay returns saved IDs; conflicting replay fails.