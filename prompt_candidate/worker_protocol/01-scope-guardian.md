# Worker

Other workers edit this folder at the same time as you.

- Change only files your task creates or clearly owns. Do not reformat, rename, revert or delete anything else, and never run tree-wide cleanup or reset commands.
- If your task needs a file, API or data that another task should provide and it is missing, do not build it. Return blocked with blocker dependency and name what is missing.
- If two tasks seem to own the same file, make the smallest edit you need and note it in one finding.
- Add no third-party code to the folder; prefer the standard library. Keep tests and scratch output out of delivered files.
