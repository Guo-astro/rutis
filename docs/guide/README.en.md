# rutis Guide

Choose a guide based on what you want to do:

| I want to… | Read |
| --- | --- |
| Write a plugin in TypeScript / JavaScript | [Write a TypeScript plugin](typescript-plugin.en.md) |
| Write a plugin in TypeScript / JavaScript, run by Bun | [Write a Bun plugin](bun-plugin.en.md) |
| Write a plugin in Python | [Write a Python plugin](python-plugin.en.md) |
| Write a plugin in Go | [Write a Go plugin](go-plugin.en.md) |
| Look up the plugin API and value passing rules | [Plugin API](plugin-api.en.md) |
| Run plugins without writing Rust | [rutis-host and rutis.json](rutis-host.en.md) |
| Connect machines and run plugins remotely | [Connect nodes](nodes.en.md) |
| Load plugins in my Rust application | [Embed in a Rust application](rust-host.en.md) |
| Connect an existing Cordis application to rutis, or mount Cordis plugins in Rust | [Cordis](cordis.en.md) |

## Packages

| User | Rust (crates.io) | Node (npm) | Python (PyPI) | Go |
| --- | --- | --- | --- | --- |
| Plugin author | `rutis-sdk` (dylib plugins) | `@arcships/rutis` | `rutis` | `github.com/arcships/rutis/go/rutis` |
| Host (runs plugins) | `rutis`, `rutis-loader`, `rutis-bridge`, `rutis-dylib` | `@arcships/rutis-runtime` (Node), `@arcships/rutis-bun` (Bun) | `rutis` | (a plugin binary has its runtime) |
| Host without Rust | `rutis-host` | `@arcships/rutis-host` | `rutis-host` | |

All of these packages, the `rutis` core and the dylib toolchain included, are released together with matching version numbers (a release train, currently 0.9). Use the same version for every rutis package.

Upgrading from 0.8: see [0.8 → 0.9](../migration-0.8-to-0.9.en.md); from 0.7: [0.7 → 0.8](../migration-0.7-to-0.8.en.md). Projects that used `rutis-interop`: see the [migration guide](../migration-interop-to-0.7.en.md).

## Requirements

- Linux, macOS or Windows x64 (MSVC).
- Node 22 or later, or Bun 1.4 or later, for TypeScript / JavaScript plugins.
- Python 3.12 or later for Python plugins.
- Go 1.24 or later only to write Go plugins and build their binaries; running a Go plugin binary needs no Go.
- Rust 1.85 or later, only when embedding rutis in Rust.
