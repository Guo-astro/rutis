# rutis Guide

Choose a guide based on what you want to do:

| I want to… | Read |
| --- | --- |
| Write a plugin in TypeScript / JavaScript | [Write a TypeScript plugin](typescript-plugin.en.md) |
| Write a plugin in TypeScript / JavaScript, run by Bun | [Write a Bun plugin](bun-plugin.en.md) |
| Write a plugin in Python | [Write a Python plugin](python-plugin.en.md) |
| Write a plugin in Go | [Write a Go plugin](go-plugin.en.md) |
| Write a plugin in Rust | [Write a Rust plugin](rust-plugin.en.md) |
| Write a macOS plugin in Swift | [Write a Swift plugin](swift-plugin.en.md) |
| Offer an iOS / iPadOS / macOS app's capabilities to rutis | [Host plugins in an app (Swift)](swift-app.en.md) |
| Provide plugins in Kotlin from an Android application | [Hosting plugins in an Android application](kotlin-app.en.md) |
| Look up the plugin API and value passing rules | [Plugin API](plugin-api.en.md) |
| Send and receive events between plugins (any language, process or node) | [Plugin API: events](plugin-api.en.md#events) |
| Run plugins without writing Rust | [rutis-host and rutis.json](rutis-host.en.md) |
| Connect machines and run plugins remotely | [Connect nodes](nodes.en.md) |
| Load plugins in my Rust application | [Embed in a Rust application](rust-host.en.md) |
| Connect an existing Cordis application to rutis, or mount Cordis plugins in Rust | [Cordis](cordis.en.md) |

## Packages

| User | Rust (crates.io) | Node (npm) | Python (PyPI) | Go | Swift (SwiftPM) |
| --- | --- | --- | --- | --- | --- |
| Plugin author | `rutis`; `rutis-sdk` for dylib plugins | `@arcships/rutis` | `rutis` | `github.com/arcships/rutis/go/rutis` | `https://github.com/arcships/rutis` (products `Rutis`, `RutisTest`) |
| Host (runs plugins) | `rutis`, `rutis-loader`, `rutis-bridge`, `rutis-dylib` | `@arcships/rutis-runtime` (Node), `@arcships/rutis-bun` (Bun) | `rutis` | (a plugin binary has its runtime) | (a plugin binary has its runtime) |
| Host without Rust | `rutis-host` | `@arcships/rutis-host` | `rutis-host` | | |

All of these packages, the `rutis` core and the dylib toolchain included, are released together with matching version numbers (a release train, currently 0.17). Use the same version for every rutis package a host or an application installs; how a plugin depends on its SDK (a Python plugin: a lower bound only) is in [plugin API: versions](plugin-api.en.md#versions).

Upgrading from 0.16: see [migrating to 0.17](../migration-0.17.en.md) (Rust plugins and Rust hosts only: one way to write a plugin, what hosts use moved to `rutis::host`, with a migration tool, `rutis-migrate`). Upgrading from 0.13–0.15.1: see [migrating to 0.16](../migration-0.16.en.md) (events are one flat bus; 0.16 removes the 0.15.1 compatibility shims). Upgrading from 0.12: see [migrating to 0.13](../migration-0.13.en.md) (unified events; no mixing with 0.12). Upgrading from 0.8: see [0.8 → 0.9](../migration-0.8-to-0.9.en.md); from 0.7: [0.7 → 0.8](../migration-0.7-to-0.8.en.md). Projects that used `rutis-interop`: see the [migration guide](../migration-interop-to-0.7.en.md).

## Requirements

- Linux, macOS or Windows x64 (MSVC).
- Node 22 or later, or Bun 1.4 or later, for TypeScript / JavaScript plugins.
- Python 3.12 or later for Python plugins.
- Go 1.24 or later only to write Go plugins and build their binaries; running a Go plugin binary needs no Go.
- macOS 14 or later and a Swift 6.2 toolchain only to write Swift plugins and build their binaries; running a Swift plugin binary needs no toolchain, and runs on macOS only.
- JDK 17 and Android 10 (API 29) or later for Kotlin plugins, compiled into an Android application (the packages are `io.github.arcships:rutis` on Maven Central and, at compile time, `io.github.arcships:rutis-ksp`).
- Rust 1.88 or later, only to write Rust plugins or embed rutis in Rust. This version is verified with the repository's `Cargo.lock`; when an older Rust resolves dependencies itself, setting `resolver.incompatible-rust-versions = "fallback"` in the Cargo configuration makes Cargo pick dependency versions that work.
