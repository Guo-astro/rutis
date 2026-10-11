# rutis 0.8 → 0.9

[中文](migration-0.8-to-0.9.md) · [Release notes](releases/0.9.0.en.md)

Most projects only change their dependency versions and upgrade the host and the language runtimes together. Rust code that builds `Launcher` / `Spawn` / `RowSchema` itself, projects whose scripts check `rutis-host` exit codes, and dylib plugins should also read the matching sections below.

## Dependency versions

Move every rutis package to 0.9:

```toml
# Cargo.toml
rutis = "0.9"
rutis-loader = "0.9"
rutis-bridge = "0.9"
```

```json
"@arcships/rutis": "^0.9.0"
```

```toml
# pyproject.toml
dependencies = ["rutis>=0.9,<0.10"]
```

## Upgrade the host and the language runtimes together

Protocol versions and PLUGIN_API are unchanged, but a 0.9 host forwards a call's origin with a session label (for example `s1/rust:7`). A 0.8 runtime does not accept it and ends the session. Use the same version for the host (`rutis-host`, or embedded `rutis-bridge` / `rutis-loader`), `@arcships/rutis-runtime` and PyPI's `rutis`.

## Minimum versions

| | 0.8 | 0.9 |
| --- | --- | --- |
| Rust | 1.85 | 1.88 |
| Node | 24 | 22 |
| Python | 3.12 | 3.10 |
| websockets (Python's `network`) | 13 | 15 |
| Bun | — | 1.4 |
| Go | — | 1.24 |

## `Launcher` and `Spawn`: use the constructors

`rutis_bridge::runtime::Launcher` and `rutis_bridge::transport::local::Spawn` gained the fields `stdin` / `stdout` / `stderr` and are `#[non_exhaustive]`: they can no longer be built with a struct literal (`..Default::default()` included):

```rust
// 0.8
let launcher = Launcher {
    program: "node".into(),
    args: vec!["runner.mjs".into()],
    inherit_fd: true,
    ..Default::default()
};

// 0.9
let launcher = Launcher::new("node").arg("runner.mjs").inherit_fd();
```

Build a `Spawn` with `Spawn::new(program, peer)` and then set its fields:

```rust
let mut spawn = Spawn::new("python", peer);
spawn.args = vec!["-m".into(), "rutis".into()];
spawn.handover = Handover::DialBack;
```

The fields are still public, so reading and assigning them is unaffected. The standard streams default as in 0.8: stdin is `Stdio::Null`, stdout / stderr are `Stdio::Inherit`.

## `RowSchema` gains `version`

`rutis_bridge::runtime::RowSchema` has a new field `version: Option<String>` (the plugin version the runtime reports). A literal that lists every field needs it, or use `..Default::default()`.

## `rutis-host` exit codes and `check`

- `rutis-host check` exits with 1 when a service a row injects is provided by no row. When the provider is on another node (a `peer:` row, for example), it only notes it. Disabled rows are no longer resolved, so a disabled row whose plugin is not installed no longer fails `check`.
- `run` / `dev` exit with **2** when, after a stop signal, the cleanup does not finish within `--shutdown-timeout` (10 seconds by default). Scripts that treat any non-zero code as failure are unaffected.
- Runtime processes run in their own process group, so Ctrl-C in the terminal reaches only `rutis-host`, which unloads the rows. Under systemd, use `KillMode=mixed`.

## Configuration

- With `rutis.json` given as a relative path (`rutis-host run conf/rutis.json`), rows such as `./plugin.mjs` resolve against the configuration file. In 0.8 they were not found.
- A listener with only `cert` or only `key` is an error. 0.8 silently ran it without TLS.
- Runtime names must be unique; unknown fields in `rows` are warned about.

## Behavior changes

- One message on a local channel is at most 16 MiB; a larger one closes the channel.
- When a plugin in a Node row unloads itself, the loader emits `LoaderChanged::SelfDisposed` and the row becomes disabled (a copy in an instance becomes `Stopped`). In 0.8 it stayed `Running`.
- `python -m rutis` exits with 1 on an error (0 in 0.8); a Python WebSocket listener whose `accept()` fails ends as a failure.

## Dylib plugins

The SDK identity follows the version: **dylib plugins built for 0.8 cannot be loaded by a 0.9 host**. Rebuild them with the 0.9.0 SDK bundle.
