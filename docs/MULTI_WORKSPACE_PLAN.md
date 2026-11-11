# Multi workspace buildout

> **Architecture reopened.** The revised hierarchy is ready for walkthrough in the [interactive prototype](workspace-prototype.html). See the [independent review and walkthrough](WORKSPACE_IA_REVIEW.md) for validation status. Production navigation requires desktop/mobile walkthrough evidence and renewed user acceptance.

Relay should answer two distinct questions: what needs attention across all my workspaces, and what is happening inside this workspace? A workspace is the existing persisted project/board. A task's filesystem workspace becomes an **execution directory** in user-facing copy; it is not another navigation scope. Existing project API identifiers and URLs remain compatible.

## Audit and decisions

| Current surface or setting | Owner and destination | Change |
| --- | --- | --- |
| Project dropdown, generic Workspace breadcrumb | Workspace identity | Use Workspace consistently and show its actual name. Provide All workspaces as an explicit destination. |
| Task board, Epics, Queue & blockers | Selected workspace / Work | Preserve task actions and local queue; never aggregate task mutations. |
| Agents and worker registrations | Selected workspace / Agents | Label their scope. The same worker ID in different workspaces represents separate registrations. |
| Workflow tab | Workspace settings / Workflow | Consolidate routes, concurrency, retry delay, independent review and automatic advancement in one settings destination. Legacy workflow navigation redirects here. |
| Project name and data directory | Workspace settings / General | Editable name, read-only storage location, scoped save feedback. Validate trimmed names and duplicates. |
| Pause coordination | Selected workspace / Queue & blockers | Name the workspace and explain that active attempts continue. Link to host maintenance separately when active. |
| Backups and export | Workspace settings / Data | Explicitly identify the affected workspace. Never imply a single-board backup covers the host. |
| Archived tasks inside Settings | Selected workspace / Archive | Move to a work destination with restore and preserved history. |
| Insights | Workspace / Metrics | Keep local results and usage; add equivalent aggregate visibility under All workspaces. |
| Activity | Selected workspace / Activity | Retain durable local history. Global overview provides workspace drill-down rather than mixing task identities. |
| Host drain | Global settings / Maintenance | Expose maintenance state and control with explicit all-workspace impact. Existing runtime-only lifecycle remains; restart clears maintenance. |
| Version, host data root | Global settings / Server | Read-only diagnostics. Explain workspace backup boundaries. |
| Theme | Global settings / Appearance | Browser preference, shared across workspaces; label that it applies only to this browser. Keep quick theme toggle. |
| Timeout, attempt/sendback/token budgets, task assignments, approval | Task editor/detail | Keep task-owned execution policy. Agent capacity, executable, environment and timeout remain agent-owned. |

## Navigation and interaction contract

Global destinations are All workspaces and Global settings. All workspaces owns creation and links to each workspace's Task board, Queue & blockers and Metrics. Global settings owns Maintenance, Appearance and Server. The workspace chooser lists only persisted workspace identities. Desktop uses a sidebar and mobile uses a labeled Navigation panel beside the independent workspace chooser. Both show the workspace name and current destination outside the panel.

The local hierarchy groups Task board, Epics, Queue & blockers and Archive under Work. Agents has its own entry. Metrics and Activity appear under Reporting & history. Workspace settings contains General, Workflow and Data. The Work and Reporting & history labels require no landing pages. Task and agent details show the containing workspace. Execution directory belongs to task execution settings.

Use `?scope=global&page=overview` and `?scope=global&page=global-settings` for global views, and `?project=<id>&page=<page>` for workspace views. Keep `insights` as the Metrics route and accept the `metrics` alias. Route the old `workflow` page to Workspace settings / Workflow. Explicit URLs win over validated per-workspace preferences. Back, Forward and reload retain scope. Switching from a global page or Workspace settings opens the destination Task board. Other local views retain the page and clear task, dialog and filter state. Render only data for the requested scope. An unknown workspace links to All workspaces. Returning from a task drill-down restores the originating workspace view and focus.

All workspaces shows aggregate work/attempt/usage metrics and a row/card per workspace with name, pause/maintenance state, task totals, attention, running attempts, configured capacity and a direct board link. Empty, unavailable, loading, disconnected and partial usage states must be explicit. Cross-workspace sums are server summaries, not downloads of every task artifact/log.

## Data and API contract

- Add `GET /api/overview`: `{ scope: "global", generated_at, metrics, workspaces, server }`. Each workspace includes existing project identity, `metrics`, `paused`, `concurrency` and health/dispatch context. `server` exposes `draining`, `build_id`, `version`, and `data_dir`. No token, task bodies or artifacts in aggregates.
- Add `GET /api/projects/{id}/metrics` using the same metric definitions as the aggregate endpoint. Metrics: nonarchived non-epic `tasks`, `completed`, `attention` (blocked/needs_input/failed), `running_attempts`, all-history `attempts`, `failed_attempts`, `sendbacks`, `duration_ms`, nullable `input_tokens`, `output_tokens`, `cost_usd`, and separate coverage counters `input_token_attempts`, `output_token_attempts`, `cost_attempts`. Unknown values stay null; mixed availability displays coverage, not a misleading complete total. Sum absolute counts before computing any rates. Worker registrations are not globally deduplicated people.
- `GET /api/global/settings` returns the server diagnostics/maintenance object. `PATCH /api/global/settings` accepts only boolean `draining`; invalid/unknown fields fail. Keep existing `/api/admin/drain` for upgrade compatibility.
- Workspace settings PATCH becomes a validated partial update, optionally guarded by `expected_revision` (snapshot revision). Preserve full-settings clients. Workspace name trim/length/duplicate validation applies to rename and create. Only the explicitly addressed workspace changes; stale guarded saves fail with 409.
- Continue host guards and cross-project execution-directory locking. Global maintenance blocks new admissions everywhere including newly created workspaces; workspace pause only affects that workspace. Do not invent a host concurrency setting without a scheduler implementation.

## Execution plan and ownership

1. **MW-IA — Scope audit and architecture** (3 points): deliver the inventory, sitemap, ownership matrix and clickable prototype. Run independent desktop and mobile walkthroughs, fix critical findings, and obtain renewed user acceptance before production navigation work.
2. **MW-API — Global summaries and settings boundaries** (5 points, depends on MW-IA): backend agent owns Rust API/model/host changes and regression tests. Deliver aggregate/local metrics, host settings, atomic scoped partial saves, name validation and isolation coverage.
3. **MW-UI — Scope-first navigation and settings** (8 points, depends on MW-IA): frontend agent owns web app/style and frontend tests. Deliver global overview/settings, local destinations, settings consolidation, Archive, safe switching/deep links, scope/coverage copy and empty/error states against the fixed API contract.
4. **MW-QA — Integration and user journey verification** (5 points, depends on MW-API and MW-UI): independent reviewer audits combined code, exercises two-workspace scenarios, and reports actionable defects. Root resolves integration defects, builds embedded assets/server, and verifies the running result.

Implementers edit disjoint file sets. Use task dependencies to sequence work and leave normal scheduler dispatch in place. Attach implementation and review evidence before advancing each stage.

## Acceptance matrix

1. Two workspaces with distinct tasks, usage and settings produce exact per-workspace and global counts. Archived tasks/epics do not inflate work totals; unavailable and partially reported usage are honest; empty workspaces have valid summaries.
2. Changing name, workflow or pause in A leaves B untouched. A stale settings form cannot overwrite a newer guarded save; partial updates preserve unrelated fields. Duplicate/blank/oversized names fail consistently.
3. Global maintenance affects every workspace and resumes without changing their pause preferences. Mutations without the session token fail. Legacy project endpoints/default board and restart persistence still work.
4. A user can reach global metrics/settings and workspace metrics/settings directly; breadcrumbs show the real workspace. Reload/back/forward, mobile navigation, unknown IDs and invalid stored pages are safe.
5. Workflow has one canonical settings home; archive is under work; all existing task, agent, backup/export, activity and restore capabilities remain reachable. Editing a form is not erased by polling.
6. Run Rust formatting and the full locked test suite, frontend tests/build and browser smoke checks at desktop/mobile sizes. Upgrade the local server only after checks, preserving all boards through the existing backup/drain workflow.

## Boundaries

This buildout does not add destructive workspace deletion, cross-workspace task moves, global agent inheritance, shared workflow defaults, authentication/teams, or worktree provisioning. These need separate product contracts; the current local single-user host and isolated board storage remain intact.

## Critical recovery correction

The Chess960 incident exposed a scheduler ownership gap: an implementation agent returned `blocked` for unavailable git/index and registry access, and all dependent work stopped with no repair owner. Agent-produced failures must remain coordinator-owned. A status badge asking the user to unblock work is not a recovery mechanism.

The recovery path first sends eligible agent blockers back through planning with the original failure evidence and bounded attempts. When ordinary recovery is exhausted or a route is unavailable, create a durable supervisor repair task linked to the original attempt. The repair task uses available configured agents, diagnoses authorized alternatives, implements a correction and passes review before the original stage is retried. Keep original evidence, make repair ownership visible, deduplicate scheduler passes and prohibit recursive repair-task explosions. Never treat a blocked task as complete, silently relax execution permissions, or turn an agent failure into an unsolicited user action item. A repair that cannot execute remains explicitly owned by the coordinator rather than assigned to the user; this is not a guarantee that unavailable runtime capabilities can be manufactured.

Add regression coverage for blocker routing, absent/disabled roles, exhaustion, repair identity, review-gated resumption, pause behavior and repeat reconciliation. The user separately authorized resetting the failed Chess960 application v2 board. Provide a named-project reset with session authentication, exact ID/name confirmation, active-run/fence rejection, backup and transactional board-only clearing. Preserve chess source files and all other boards. This reset is incident remediation, not general workspace deletion.
