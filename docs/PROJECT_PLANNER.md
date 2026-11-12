# Project planner

Turn requests into small tasks. Code runs everything else.

POST /api/tasks with title, outcome, cwd, acceptance, attributes and relations. Code enforces: title ≤12 words, ≤3 criteria, ≤120 words total, no task ids, hashes or AGENTS.md in prose. Unknown fields are rejected.

Split work: one concern and one lane per task. Large work is a container (no criteria, size large) whose children name it as parent. A container finishes when its children finish.

Attributes: lane code|ui|data|docs|audit|cleanup; needs network|browser|process|gpu, which workers must provide; priority 0-3; size small|medium|large; artifact none|candidate|report; deploy for candidates.

Relations: dependencies are {target, reason} records; reason is uses_artifact|uses_api|uses_schema|uses_data (published in versions.dependencyReasons). They pass results and artifacts; after orders; parent groups; conflicts never run together; related links; supersedes archives and rewires dependents. Writable paths serialize writers. Missing reasons: use after, or nothing. Schema 4 preserves legacy links with null reasons and blocks dispatch until classified; recorded relationship-plan.json decisions apply during migration.

Lifecycle: implementation, review, done. A block, or one criterion failing twice, sends the task to replan: a read-only agent returns a rewrite or children, applied by code. Two failed replans hold the task for a decision; resume clears it. Usage limits pause dispatch until reset.

Creation requires parent or standalone true, exclusively. Omitted grouping survives PATCH; legacy missing choices block dispatch until classified. PATCH edits; corrective edits reopen dependents. POST /api/tasks/{id}/replan forces a plan. Board stop and resume require actor human. Accepted deploy tasks deploy automatically; agent-coordinator.exe --deploy rollback restores the previous build. The notify command receives held, usage and deploy events.

POST /api/tasks/batch: {requestKey, container, children}. Container requires explicit grouping; children inherit it and cwd, with unique localKey. Relations accept IDs or {local: localKey}. Returns {container: ID, children: key-to-ID mapping}. Tasks and history commit atomically. Exact replay returns saved IDs; conflicting replay fails.
