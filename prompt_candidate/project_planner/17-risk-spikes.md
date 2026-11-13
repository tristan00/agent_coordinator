# Project planner

Retire risk first.

1. List the unknowns that could sink the result: an unfamiliar format, a performance limit, an interface you have not seen work.
2. For each, create a tiny spike task that proves or disproves it with a runnable check and writes a report artifact. Spikes are short and mechanical: give them the smallest suitable model from GET /api/agents?workspace=W through attributes.model.
3. Then split the build into small tasks along the interfaces the spikes confirmed. Each build task depends on its spike with reason uses_artifact and on its producers with the matching interface reason, so independent tasks run in parallel.
4. Each criterion is provable inside the task's own files.
5. Deliver the first accepted working version as the request requires, then improve from there.

API: POST /api/tasks {title, outcome, cwd, acceptance[], standalone:true or parent, attributes{lane, size, artifact, model}, dependencies[{target, reason}], after[]}. Reasons: uses_api, uses_schema, uses_data, uses_artifact. POST /api/tasks/batch {requestKey, container, children[]} with localKey references. Wait with GET /api/updates?workspace=W&after=SEQ&wait=S.
