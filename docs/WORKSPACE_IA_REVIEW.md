# Relay workspace architecture review

Open [the prototype](workspace-prototype.html) in a browser. If the browser restricts storage for local files, serve the docs directory with `python -m http.server 8765 --bind 127.0.0.1 --directory docs` and open `http://127.0.0.1:8765/workspace-prototype.html`. Start each run with fresh fixture storage and use a desktop viewport around 1280px and a mobile viewport around 390px. Workspace changes stay in the browser tab. The prototype uses sample data and has no connection to a Relay server.

## Independent review

An independent reviewer inspected the source and ran generated links and mutation handlers in a Node VM with a simulated DOM. Local browser use was disabled in Work settings, and native Chrome was denied by the computer-use approval check. Desktop and mobile interaction, focus and layout remain unverified.

| Goal | First choice and result | Scope or return |
| --- | --- | --- |
| Identify partial usage and find work needing attention | All workspaces showed 1,800 input tokens reported by 2 of 6 attempts. The reviewer first tried the static aggregate attention metric, then used Atlas's attention link. | The link opens the full Atlas queue, which also contains ready and running work. The revised queue links back to All workspaces. |
| Compare local results | Metrics, then Birch in the workspace chooser. Birch Metrics showed no reported input usage. | The page persists across workspace switches. Switching from Workspace settings opens Task board. The current chooser entry is now a status, so reselecting it cannot clear the current view. |
| Inspect epic and recovery evidence | Epics → child → Attempts. Queue → blocked task → linked repair. | The epic link returns to the child context. The repair returns to the original task, then to the filtered queue. The revised details identify coordinator recovery and its current phase. |
| Recover archived work | Archive → task → Restore → History. | The confirmation names Atlas. History records restoration, and the item moves to Task board. |
| Inspect assignment and change routing | Agents for the worker assignment and profile capacity. Workspace settings → Workflow for workspace routing. | Assignment drill-down returns to Agents with the profile context intact. Agent capacity and workspace launch capacity are separate controls. |
| Rename, find a backup, and distinguish dispatch from maintenance | General for rename and Data for backup. Queue for workspace pause and Global settings for host maintenance. | The reviewer predicted Atlas-only pause and host-wide maintenance. Pause names Atlas and keeps active attempts running. Returning from Global settings uses browser Back. |
| Recover from an invalid link | All workspaces from the unavailable-workspace view. | The fixture renders the recovery link and offers Retry for a simulated connection interruption. |

The first pass identified lost queue search and agent profile context, an indirect return from overview drill-downs, and indistinguishable backup and export payloads. The revised simulated walkthrough verified context restoration. The fixture now has distinct sample download payloads, attempt details and activity events. Separating worker registrations from agent profiles and keeping the current workspace entry inert followed the independent rerun.

## Remaining acceptance checks

Give reviewers the goals in the first column without the expected paths. Record their first choice, wrong turns, predicted scope before each mutation and how they return. Check keyboard-only access, Escape and trigger focus, viewport fit, long workspace names, browser Back and Forward, dirty form navigation, and Retry. Rerun any journey where the reviewer predicts the wrong scope, cannot find a required destination or cannot recover context.

The architecture inventory specifies task creation and editing, populated artifacts, command-agent configuration, full stage routing and independent review. The fixture demonstrates representative controls. Those additional journeys and real polling, save conflicts, delayed responses and API isolation remain unverified. The production regression suite passed all 12 existing frontend tests. It exercises the current controller, including the old mixed selector, so its result is a baseline.

Independent desktop and mobile browser walkthroughs and renewed user acceptance remain open. Use the existing task dependencies to sequence production navigation after those checks.
