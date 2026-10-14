> Copied from `docs/design-philosophy.en.md` in the rutis repository (main, 2026-10-14) so this skill can be used on its own. The original is authoritative: https://github.com/arcships/rutis/blob/main/docs/design-philosophy.en.md

## 7. Principles and how to decide

This section follows from the claims above and is written mainly for contributors.

### Principles

1. **The model is implemented once, in the Rust core**; plugins in other languages are leaves with a small SDK ([multi-language design](https://github.com/arcships/rutis/blob/main/docs/design-multilang-runtimes-2026-10-03.en.md)).
2. **Compatibility work happens outside the core** ([mount requirements](https://github.com/arcships/rutis/blob/main/docs/requirements-protocol-plugins.en.md) §1).
3. **Guarantees are written down and pinned by tests**; guarantees not made are written down too ([core design](https://github.com/arcships/rutis/blob/main/docs/design-rust-port.en.md) D31).
4. **Extension points are opened for concrete needs, and kept narrow**; no catch-all hooks ([observation and interception design](https://github.com/arcships/rutis/blob/main/docs/design-cordis-observation.en.md)).
5. **Whatever rutis connects to is a plugin**: runtimes, nodes and mounts are under the same lifecycle, with no special privileges.
6. **Contracts cover only what crossing a boundary needs**: service names and method shapes (sync or async), no general type layer. rutis defines how values convert between languages; whether different implementations of one service return the same data, and can replace each other, is up to the service's authors.
7. **What cannot be done across a boundary becomes a boundary rule**, not a silent degradation.
8. **Dependency declarations contain only service names**, never languages or locations.
9. **Configuration describes the desired state**, which the loader keeps reconciling; the host does not hand-write start and stop sequences.
10. **Explicit over implicit**: implicit mechanisms do not cross a process boundary.

### Deciding whether a capability belongs

1. Does it widen what a host can connect to or adapt to, or help the host understand and iterate on its own connections? A runtime is worth connecting for the ecosystem it brings, not to support one more language.
2. Can it live outside the core?
3. Does it still hold across a process boundary?
4. Does it have a concrete user?
5. Can its guarantees be written as tests?

