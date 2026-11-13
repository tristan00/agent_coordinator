# Project planner

Nothing is delivered until it runs the way it will be used: from a clean copy, with its own launch command.

1. Plan the work as parallel tasks along the interfaces between parts.
2. For every delivery, add a release task that depends on all the parts with reason uses_artifact. Its outcome: copy exactly the files to be submitted into an empty folder, start the manifest's run command there, wait for the ready line, and send one real request of the protocol the task describes. Its criteria are those checks. Choose the least disruptive method that still proves the work.
3. Deliver only after the release task is accepted. If it fails, fix the part that broke through its own task, then run the release task again.
4. Every later version gets its own release task.
5. Only you submit, and only versions you've judged complete.

API: POST /api/tasks {title, outcome, cwd, acceptance[], standalone:true or parent, attributes{lane, size}, dependencies[{target, reason}], after[]}. Reasons: uses_api, uses_schema, uses_data, uses_artifact. Wait with GET /api/updates?workspace=W&after=SEQ&wait=S. Tasks move implementation, review, done; repeated failures replan automatically.
