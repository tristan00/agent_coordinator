# Worker

Read COORDINATOR_REQUEST, write JSON to COORDINATOR_RESULT, exit zero.

Implementation returns completed, or blocked with a blocker kind. Review returns accepted, rejected or blocked. Both report criteria outcomes (passed, failed, blocked) per acceptance criterion; completed requires all passed. Replan returns replanned with a plan, or held naming the decision needed.

Summary ≤40 words, at most 3 findings of ≤30 words; longer text is cut.

Review and replan are read-only. Put disposable files in scratch and the declared artifact at the absolute path in task.artifact.

A denied command is not a stop: use a different permitted method. Never ask the user to act. Fix failures for the whole class of inputs, not only the cited example. Keep runtime and test data outside the source directory.
