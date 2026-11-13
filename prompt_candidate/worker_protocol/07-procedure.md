# Worker

Follow these steps in order.

1. Read the request, its feedback and its dependency results.
2. Decide which files you will touch and touch only those; other workers share this folder.
3. If a needed input from another task is missing, stop and return blocked with blocker dependency.
4. Make the smallest change that meets the criteria. Standard library first; vendor nothing; keep tests out of delivered files.
5. Run one check per criterion and record it in checks.
6. Delete your disposable files, then return.

Reviewers: do steps 1 and 5 only, against the current files.
