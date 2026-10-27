# Migrating to 0.18

[中文](migration-0.18.md)

0.18 makes failures understandable ([diagnostics design](design-diagnostics-2026-10-26.en.md)). This page grows with the changes of 0.18; below is what is done so far.

## `rutis-host check`: cycles and services provided twice are errors (#676)

In 0.17.2 these two cases were warnings and left the exit status unchanged; from 0.18 they say `rutis-host: error: …` and exit with 1 (with or without `--strict`). A configuration that passed `check` in CI may now fail:

- **Rows whose `inject` wait for each other's services (a cycle)**: none of them starts.

  ```
  rutis-host: error: rows a, b wait for each other (a waits for bsvc from b, b waits for asvc from a): none of them starts; break the cycle in their inject
  ```

  Fix: remove from one row's `inject` a service it does not need; when both need each other, move the shared part into a third row both wait for.

- **Rows declaring the same service in one scope**: only the first to start provides it, the others fail.

  ```
  rutis-host: error: rows one, two all provide llm: only the first to start provides it, the others fail; keep one, or isolate the name in each row
  ```

  Fix: keep one row; when you need both, `isolate` the name: with `{ "llm": "<label>" }` the row providing it and the rows using it share a label, each group with its own copy (`{ "llm": true }` is a copy for that row alone, which other rows cannot use).

Before upgrading, run `rutis-host check` with 0.17.2: the warnings it writes for these two cases are the errors 0.18 reports.

## Two entries on one listener accepting one token: an error at start (#702)

When a listening peer row and a runtime that connects in (`listen` + `instances` of `runtimes.remote`) would accept the same token on one listener (say neither has its own and both fall back to `RUTIS_TOKEN`), 0.17 started, but refused each connection with 403 (it could not tell which entry it was for), and the runtime retried every 30 seconds. From 0.18 `run`, `check` and a configuration reload fail before starting. The same holds for any two entries on one listener: two runtimes that connect in (refused with 403 in 0.17 too), two listening peer rows (in 0.17 every connection reached the later row, and the earlier one never connected).

```
rutis-host: listener public: link row main and remote runtime phone both accept RUTIS_TOKEN, so a connection presenting it cannot be told apart: set RUTIS_TOKEN_MAIN or RUTIS_TOKEN_PHONE to a token of its own
```

Fix: as the error says, set `RUTIS_TOKEN_<name>` for one of them (and have the runtime that connects in present the new token). Such a configuration could not connect in 0.17 either; once fixed, it does.

## Node, Bun and Python runtimes: an unhandled error no longer ends the process (#688)

In 0.17 a promise rejection a plugin left unhandled (or an exception nobody retrieved from a Python task) ended its runtime's process, and every row of that runtime stopped and waited. From 0.18 the process goes on:

- Node under `rutis-host dev`, and Python always, trace the row that produced it: that row fails, saying what and where; the other rows of the runtime run on.

  ```
  weather: failed: unhandled rejection: Error: no region (at file:///…/weather.mjs:12:9)
  ```

- Node under `rutis-host run`, and Bun, cannot tell which row it came from: they write a line saying the source is unknown, and no row fails.

  ```
  runtime node: an unhandled error from an unknown row: Error: no region
      at …
  ```

- A plugin calling `process.exit` / `os._exit` still ends the process, and the runtime first writes `runtime node: row weather called process.exit(1)`.

What to change: a project that recovered by letting the runtime exit on an error and having the host restart it (`restart`) now sees the row fail (or the runtime go on) instead; the runtime does not exit. To restart after an error, handle the error in the plugin; to end the process, call `process.exit` explicitly. See [Plugins in one runtime share its process](guide/plugin-api.en.md#plugins-in-one-runtime-share-its-process) in the plugin API guide.
