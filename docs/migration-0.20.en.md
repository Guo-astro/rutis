# Migrating to 0.20

[中文](migration-0.20.md)

0.20 is not released yet; this guide is written together with the incompatible changes, one section per change, and completed before the release as [#694](https://github.com/arcships/rutis/issues/694) says. Only the changes since 0.19 are written here, and removed forms are removed outright, not deprecated.

The incompatible changes, by kind, each with the section saying how to change:

- **Unload order**: cleanups and service withdrawals share one stack and run in reverse, one at a time ([Unload order](#unload-order-1010)).

The rest is new, and changes nothing already written.

## Unload order (#1010)

Design: [Unload order: withdrawals and cleanups on one reverse stack](design-unload-order-2026-11-01.en.md). All six languages (Go, Python, Bun, Kotlin, Swift, Node) behave this way.

### What changed

A row's unload runs **one** stack: `provide` puts "withdraw this service" on it, `ctx.effect` and the cleanup `apply` returns put cleanups on it. Unloading runs it in reverse, one entry at a time, each withdrawal in the place it was registered.

| Case | 0.19 | 0.20 |
| --- | --- | --- |
| A cleanup registered **after** `provide` reads the service its row provided | Go, Python, Bun, Kotlin, Swift: `not-declared`; Node: it reads it | All six read it: the object itself |
| A cleanup registered **before** `provide` reads it | As above: the five say `not-declared`, Node reads it | None of the six reads it: `use` says `not-declared`, `get` returns the empty value |
| Another row reads the service while this row unloads | The five: the withdrawal deleted it before the cleanups, `unavailable`; Node: it still read it until this row's disposals finished | All six say `unavailable`, from the moment this row starts unloading |
| A cleanup registering another cleanup | Kotlin wrote a line and did not run it; Go, Python, Bun, Swift dropped it silently; Node handed it to Cordis's disposals | All six write `rutis: row <row> is not active: cleanup is not registered` (Node: `plugin <plugin>`), and that cleanup does not run |
| Several cleanups starting together (Node) | Node's `_unload` starts every disposal at once with `Promise.all` | All six run one at a time, latest first |

### What to change

- **Do not wait for another cleanup inside a cleanup.** With one entry at a time, a cleanup waiting for another one (a promise the later cleanup would resolve, say) waits for good: one `rows.unload` has no per-entry budget in any of the six, so the row never answers and the host waits too.
- **Do not register a cleanup from a cleanup.** Unloading runs the stack as it stood when it started, so a cleanup registered while unloading does not run (the fourth row above). Do the work in one cleanup.
- **Reading a service your row provides from a cleanup no longer fails** (when the cleanup is registered after `provide`): no code change is needed, but do not conclude from it that the withdrawal already ran — to read the service, register the cleanup after `provide`; one registered before still does not read it.
- **Do not rely on other rows reading it while the cleanups run**: Node used to allow that and now does not, as in the other five.
- **Unloading may get slower**: cleanups go from "all at once" (Node) or "one by one, withdrawal first" (the five) to "in reverse, one at a time", so a row with many slow cleanups unloads more slowly. The six do not agree on a cleanup deadline: see the table under "Host owners" in §3 of [Unload order](design-unload-order-2026-11-01.en.md) — Kotlin times each entry in `dispose`, Go and Swift give the whole unloading one budget when the session ends, and Python, Node and Bun give up only when the process exits.

### What it affects

- Plugin authors: a plugin working on 0.19 needs changes only if it relies on cleanups running in parallel, or on registering a cleanup from a cleanup. Neither was promised on 0.19 either; it just happened to hold then.
- Host owners: the frame order does not change — announced services are still withdrawn before `rows.unload` is answered, and the reply still comes after every cleanup has run; `--shutdown-timeout` means the same, and slow cleanups behave as the table above says.
