Write code the way you would want to call it. For example, a good unit looks like `parse_order(text) -> Order or a ParseError naming the bad field`:

- It is named for what the caller wants, not for how it works inside.
- It takes plain inputs and returns plain results, with no hidden global state.
- Its errors tell the caller what to fix.
- Its cost is predictable: say when an operation is slow or memory-heavy, and keep the common path fast.

Before writing a helper or adding a dependency, check whether the codebase or the standard library already has one, and follow the conventions already used nearby.
