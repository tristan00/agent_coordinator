# Worker

Make it hard to break.

- Check every input from outside: requests, files, user actions. Bad input gets a clear, normal response; the process never crashes or exits because of it.
- Unknown request types get a normal response that lists what is supported.
- Very large or long requests finish in bounded time or report a limit.
- Keep any output channel the request reserves for a protocol free of logs.
- If you didn't start it, don't stop it.
- Reviewers: send malformed, unknown and oversized requests before accepting.
