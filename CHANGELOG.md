# Changelog

[中文](CHANGELOG.zh-CN.md)

Every rutis package shares one version number. Each release has full notes in [docs/releases](docs/releases), in English and Chinese; the same notes are on the [GitHub releases](https://github.com/arcships/rutis/releases) page. Before upgrading across a minor version, read that version's migration guide (linked from its notes).

| Version | Date | What changed | Notes |
| --- | --- | --- | --- |
| 0.19.1 | 2026-10-30 | Fixes where the documented behaviour did not hold (rows loaded after `ready()` + `reconcile`, config checked at `run` start, `rutis-host add` in uv projects, TypeScript tests exiting after a timeout); synchronous waterfall back as `waterfall(event).sync(terminal)`; `Loader::instance_rows`; copies of foreign plugin definitions in a host's own instances | [English](docs/releases/0.19.1.en.md) · [中文](docs/releases/0.19.1.md) |
| 0.19.0 | 2026-10-29 | For host owners: `rutis-host call`, `rutis-host add` / `remove` / `upgrade`, plugins managing rows through the `loader` service; remote runtimes, devices and nodes as connection rows over one session plugin (protocol 4); test tools on the real host; platform cleanup. **Breaking** | [English](docs/releases/0.19.0.en.md) · [中文](docs/releases/0.19.0.md) |
| 0.18.0 | 2026-10-28 | Errors you can understand: which row, what it was doing, which file and line; current state and time limits; config checked against the schema. **Breaking** | [English](docs/releases/0.18.0.en.md) · [中文](docs/releases/0.18.0.md) |
| 0.17.2 | 2026-10-27 | Fixes from the developer-experience evaluation: bad values fail only their call, waiting rows say what they wait for, `dev` reloads one row | [English](docs/releases/0.17.2.en.md) · [中文](docs/releases/0.17.2.md) |
| 0.17.1 | 2026-10-26 | Plugin definitions in Python, Go, Swift and Kotlin; kernel and loader fixes | [English](docs/releases/0.17.1.en.md) · [中文](docs/releases/0.17.1.md) |
| 0.17.0 | 2026-10-26 | One way to write a plugin in Rust, host APIs under `rutis::host`, `rutis-migrate`; plugin definitions as values across languages | [English](docs/releases/0.17.0.en.md) · [中文](docs/releases/0.17.0.md) |
| 0.16.0 | 2026-10-24 | Keeps the flat event bus of 0.15.1 and removes its compatibility shims | [English](docs/releases/0.16.0.en.md) · [中文](docs/releases/0.16.0.md) |
| 0.15.1 | 2026-10-23 | Events are one flat bus again, as in Cordis | [English](docs/releases/0.15.1.en.md) · [中文](docs/releases/0.15.1.md) |
| 0.15.0 | 2026-10-23 | Swift and Kotlin plugins; iOS and Android apps hosting plugins | [English](docs/releases/0.15.0.en.md) · [中文](docs/releases/0.15.0.md) |
| 0.13.0 | 2026-10-19 | Unified events across Rust, leaf plugins, Cordis, remote runtimes and nodes | [English](docs/releases/0.13.0.en.md) · [中文](docs/releases/0.13.0.md) |
| 0.12.1 | 2026-10-18 | Fixes `build::Exports` hanging on glob-import cycles | [English](docs/releases/0.12.1.en.md) · [中文](docs/releases/0.12.1.md) |
| 0.12.0 | 2026-10-18 | Services provided in any language are used across languages without declaring them | [English](docs/releases/0.12.0.en.md) · [中文](docs/releases/0.12.0.md) |
| 0.11.0 | 2026-10-17 | Events for Node, Bun, Python and Go plugins | [English](docs/releases/0.11.0.en.md) · [中文](docs/releases/0.11.0.md) |
| 0.10.0 | 2026-10-13 | Quality release: cross-process and cross-platform tests; `rutis.json` format version | [English](docs/releases/0.10.0.en.md) · [中文](docs/releases/0.10.0.md) |
| 0.9.1 | 2026-10-11 | Fixes Ctrl-C in the npm `rutis-host` launcher | [English](docs/releases/0.9.1.en.md) · [中文](docs/releases/0.9.1.md) |
| 0.9.0 | 2026-10-11 | Bun and Go plugin runtimes; sync wait cycles refused across runtimes | [English](docs/releases/0.9.0.en.md) · [中文](docs/releases/0.9.0.md) |
| 0.8.0 | 2026-10-08 | Native Windows support, per-instance plugin management, one version for every package | [English](docs/releases/0.8.0.en.md) · [中文](docs/releases/0.8.0.md) |
| 0.7.0 | 2026-10-06 | First release of the release train: `rutis-host`, language runtimes and networked nodes in one workflow | [English](docs/releases/0.7.0.en.md) · [中文](docs/releases/0.7.0.md) |

Releases before 0.7.0 are on the [GitHub releases](https://github.com/arcships/rutis/releases) page only.
