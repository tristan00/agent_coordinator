# Project rules

* The backend must be Rust.
* Comments and docstrings are forbidden.
* Git commit messages must contain fewer than 10 words.
* Documentation is restricted to AGENTS.md and the candidates in the prompt\_candidate directory that agents should avoid reading unless the task requires it.
* Project backend code, must stay below 8000 lines. This does not count ui or tests.
* Avoid arbitrary limits.
* No agent can ever edit this file
* Data should never be stored in or under this directory

- never ever end a turn because of blocked by policy, find another way to do it
- blocked by policy means you did it wrong, it doesn;t meaan the task was blocked by policy, its never an excuse to just stop work
- do not ever fucking stop work because of blocked by policy

