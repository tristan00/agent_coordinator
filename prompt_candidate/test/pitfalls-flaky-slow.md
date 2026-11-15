Tests lose their value in these ways. Avoid them:

- Flaky timing. Never sleep and hope; wait for an observable condition, and control clocks and randomness with fixed seeds.
- Testing the mock. Exercise real behaviour wherever it is cheap enough.
- Shared state. Each test sets up and cleans up its own data and can run alone, in any order.
- Pinning internals. Assert on what a user or caller can observe, not on private details.
- Slow suites. Keep the whole run short enough that people run it after every change.
