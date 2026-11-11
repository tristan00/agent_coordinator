# Execution limit audit

Scope: Rust scheduling, task and agent validation, command execution, worker protocol, HTTP routers, plan import, browser forms, scripts and maintained contracts. This audit accompanies the requested removal of limits that could stop agents. Historical incident reports and saved plans are retained as evidence rather than rewritten.

## Removed failure paths

| Previous restriction | Resulting behavior | Coverage |
|---|---|---|
| Command stdout or stderr over 2,000,000 bytes failed a run | Continuously stream logs to disk; display truncation does not terminate execution | `tests/runner_output.rs` |
| Result file over 2 MB, artifact over 1 MB, HTTP payload over 2 MB rejected work | No application size ceiling; the result contract still requires valid JSON and nonempty deliverables | `tests/runner_output.rs`, `tests/execution_limits.rs` |
| Tasks defaulted to 3 attempts, 3 sendbacks and 900-second execution timeout | Task creation, import and UI default to unlimited (`0`) | `tests/execution_limits.rs` |
| Agents defaulted to a 900-second timeout; external profiles recorded 24 hours | Agent timeout defaults to disabled (`0`) | `tests/execution_limits.rs`, timeout/cancellation tests |
| Task configuration rejected over 20 attempts/sendbacks or timeouts outside 5–86400 seconds | No arbitrary configuration ceiling; positive values remain optional operator limits | `tests/execution_limits.rs` |
| Agent capacity and workspace concurrency capped at 8 | Any positive configured concurrency/capacity | `tests/execution_limits.rs` |
| Retry delay capped at 3600 seconds | Operator may configure any representable nonnegative delay | `tests/execution_limits.rs` |
| Name/title/worker identity/plan code length ceilings and environment-variable count/length ceilings | Nonempty valid values accepted without arbitrary length/count caps | `tests/execution_limits.rs` |
| Worker leases restricted to 10–3600 seconds | Any positive lease; duration arithmetic saturates instead of overflowing | `tests/execution_limits.rs` |
| Recovery began only after retries/sendbacks exhausted, then stopped when its own allowance expired | The first failure creates a dedicated recovery task. Further failures continue through available stages/profiles without a recovery-attempt ceiling; recurrence reopens the same incident | `tests/blocker_recovery.rs` |
| Repair planning demanded proof that the repair already worked | Planning may complete with its actionable plan; Implementation performs the repair and Review uses the existing workflow. No new diagnosis or verification gate | `tests/blocker_recovery.rs` |
| No callback reached the originating planner; acknowledgement could be confused with resolving work | A durable callback queue delivers independently of recovery. Explicit ID acknowledgement records receipt only; unresolved idle incidents receive reminders | `tests/planner_callbacks.rs`, `tests/test_planner_bridge.py` |
| Very large configured timeouts could overflow the runtime clock; an interrupted HTTP response could terminate the planner receiver | Unrepresentable timer deadlines no longer panic; truncated HTTP responses enter the receiver's continuous retry loop | `tests/runner_output.rs`, `tests/test_planner_bridge.py` |

Additional liveness repairs isolate per-task reconciliation errors so malformed state cannot roll back the whole board, notify the originating planner when a route or worker is unavailable, and retry saving a finished worker outcome after temporary database failure. A failed database read is not interpreted as an instruction to kill the worker.

The new unlimited defaults do not silently rewrite explicit limits stored on existing tasks or agents. `max_attempts`, `max_sendbacks`, and `timeout_seconds` equal to `0` are unlimited; `token_budget` equal to `null` or `0` is unlimited. Explicit positive limits remain selectable. Both requested live boards were subsequently deleted entirely, so their former task/profile defaults cannot restart later.

Recovery tasks created by the previous implementation have their old attempt, sendback, timeout and token caps cleared when continuing a failed repair. The original task's explicitly configured token budget is retained and reported to its registered planner if it prevents resumption. Retry pacing is a delay between actions, not an exhaustion count.

The user clarified that **Coordinator** and **Chess960 application v2** must be deleted entirely, without retained backups. Both project directories were removed, including their databases, task/agent records, run directories and backups. Their absence and the empty `data/projects` directory were verified. The coordinator remains stopped. Source code remains in the repository. No production callback receiver is registered for either deleted board.

## Controls retained

- Explicit pause, cancel, archive, maintenance, and human approval instructions.
- Dependency ordering, stage ordering and configured independent review; these express the workflow rather than arbitrary attempt ceilings.
- Execution-directory reservations and external lost-worker fences. Starting another writer without knowing whether the first stopped would corrupt work. These waits require an active recovery/callback path, not deletion of the reservation.
- Positive worker heartbeat leases. A renewable lease is distinct from a fixed task execution timeout.
- Positive configured execution concurrency, syntactically valid task types/stages/JSON, optimistic revisions, valid environment variable syntax, and authenticated local mutations.
- Numeric storage types, database version compatibility, process cleanup after completion/cancellation, and application shutdown.
- Short polling intervals and request-level waits. Returning an empty poll keeps the receiver active; it does not fail a task.
- Upgrade drain/startup observation deadlines. The upgrade script defers an unsafe replacement and restores admissions when a running task has not finished; it does not terminate that task for exceeding the observation window.

## Remaining failure conditions

Disk exhaustion, a process that crashes, missing executable capabilities, denied access, disconnected workers, and invalid result JSON remain real failures. Removing caps does not turn them into successful task completion. The coordinator must keep recovery and planner notification active for unresolved work. A finite opt-in token budget remains an explicit user cost control; its exhaustion must be visible and actionable rather than confused with completion.

No new diagnosis/verification schema or evidence gate was added. A planning agent may hand off its actionable plan without executing an implementation check.

The Python bridge invokes a configured runtime command directly, without a shell or handler timeout. It saves successful receipts before acknowledging the server, retries failed commands and transport errors, and refreshes the session token after server restart. Its local receipt record avoids rerunning a successful handler after a lost acknowledgement. Providers must still tolerate redelivery after a crash between a handler's side effects and the durable receipt write; no exactly-once execution guarantee is claimed.

## Validation

- `scripts/build.ps1 -Test`: 102 Rust tests passed, including actual child-process execution, callback HTTP retry/restart, large output, worker fencing, board isolation and reset behavior.
- `python -m unittest discover -s tests -p test_planner_bridge.py -v`: 6 tests passed, using real local HTTP handlers and child processes.
- `cargo +stable-x86_64-pc-windows-gnu fmt --all -- --check` and `node --check web/app.js`: passed.
- The filesystem recovery scenario fails the original read, hands off a plan without requiring the planner to repair anything, continues through four failed repair executions, creates and reads back the prerequisite on the fifth, then resumes and completes the original operation through independent review.
- A separate regression injects a database write outage after a worker finishes, restores storage, and verifies the same successful outcome is saved without launching another worker.

These tests exercise the coordinator and generic bridge. They do not represent a live run against every provider's CLI or prove that an agent can solve every underlying environmental problem. No production receiver is registered for the deleted boards.
