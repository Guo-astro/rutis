# rutis 指南

按你要做的事选。

## 给宿主写插件

| 我要… | 读 |
| --- | --- |
| 用 TypeScript / JavaScript 写插件 | [写一个 TypeScript 插件](typescript-plugin.md) |
| 用 TypeScript / JavaScript 写插件，在 Bun 里运行 | [写一个 Bun 插件](bun-plugin.md) |
| 用 Python 写插件 | [写一个 Python 插件](python-plugin.md) |
| 用 Go 写插件 | [写一个 Go 插件](go-plugin.md) |
| 用 Rust 写插件 | [写一个 Rust 插件](rust-plugin.md) |
| 用 Swift 写 macOS 插件 | [写一个 Swift 插件](swift-plugin.md) |
| 把 iOS / iPadOS / macOS 应用的能力提供给 rutis | [在应用里托管插件（Swift）](swift-app.md) |
| 在 Android 应用里用 Kotlin 提供插件 | [Android 应用托管插件](kotlin-app.md) |

## 用 rutis-host 组装插件

| 我要… | 读 |
| --- | --- |
| 不写 Rust，装别人写的插件、写 `rutis.json`、运行并试一下 | [rutis-host 与 rutis.json](rutis-host.md) |
| 在另一台机器上运行插件 | [rutis-host：远程运行时](rutis-host.md#远程运行时) |

## 做一个开放插件的宿主

| 我要… | 读 |
| --- | --- |
| 在自己的 Rust 产品里接入 rutis，把插件能力开放给别的开发者 | [做一个开放插件的宿主](open-host.md) |

## 参考

| 我要… | 读 |
| --- | --- |
| 查插件能做什么、值怎样传递 | [插件 API](plugin-api.md) |
| 在插件之间（任何语言、任何进程、任何节点）收发事件 | [插件 API：事件](plugin-api.md#事件) |
| 在 Rust 应用里加载插件：API 参考 | [在 Rust 应用里嵌入](rust-host.md) |
| 把多台机器连起来 | [连接节点](nodes.md) |
| 让已有的 Cordis 应用和 rutis 互通，或在 Rust 里挂载 Cordis 插件 | [Cordis](cordis.md) |

## 包

| 谁装 | Rust（crates.io） | Node（npm） | Python（PyPI） | Go | Swift（SwiftPM） |
| --- | --- | --- | --- | --- | --- |
| 插件作者 | `rutis`；dylib 插件用 `rutis-sdk` | `@arcships/rutis` | `rutis` | `github.com/arcships/rutis/go/rutis` | `https://github.com/arcships/rutis`（产品 `Rutis`、`RutisTest`） |
| 宿主（运行插件） | `rutis`、`rutis-loader`、`rutis-bridge`、`rutis-dylib` | `@arcships/rutis-runtime`（Node）、`@arcships/rutis-bun`（Bun） | `rutis` | （插件二进制自带运行时） | （插件二进制自带运行时） |
| 不写 Rust 的宿主 | `rutis-host` | `@arcships/rutis-host` | `rutis-host` | | |

这些包（包括内核 `rutis` 和 dylib 工具链）一起发布、版本号相同（发布列车）。宿主或应用装的各个 rutis 包请使用同一个版本；插件对 SDK 怎样写依赖（Python 插件只写下限）见 [插件 API：版本](plugin-api.md#版本)。

从 0.18 升级见[迁移到 0.19](../migration-0.19.md)（远程运行时的连接、宿主与运行时一起升级、平台清理）。从 0.17 升级见[迁移到 0.18](../migration-0.18.md)，从 0.16 升级见[迁移到 0.17](../migration-0.17.md)（只影响 Rust 插件和 Rust 宿主），从 0.13–0.15.1 升级见[迁移到 0.16](../migration-0.16.md)（事件是一条平总线；0.16 删掉 0.15.1 的兼容层）。从 0.12 升级见 [迁移到 0.13](../migration-0.13.md)（统一的事件，不与 0.12 混用）。从 0.8 升级见 [0.8 → 0.9](../migration-0.8-to-0.9.md)，从 0.7 升级见 [0.7 → 0.8](../migration-0.7-to-0.8.md)。用过 `rutis-interop` 的项目见[迁移指南](../migration-interop-to-0.7.md)。

## 环境

- Linux、macOS 或 Windows x64（MSVC）。
- Node 22 或更高，或 Bun 1.4 或更高（TypeScript / JavaScript 插件）。
- Python 3.12 或更高（Python 插件）。
- Go 1.24 或更高（只在写 Go 插件、构建二进制时需要；运行 Go 插件二进制不需要）。
- macOS 14 或更高与 Swift 6.2 工具链（只在写 Swift 插件、构建二进制时需要；运行 Swift 插件二进制不需要工具链，但只在 macOS 上运行）。
- JDK 17 与 Android 10（API 29）或更高（Kotlin 插件，编进 Android 应用；不发布到 Maven，从本仓库的源码引用 `kotlin/rutis`，见 [Android 应用托管插件](kotlin-app.md#1-依赖)）。
- Rust 1.88 或更高（只在写 Rust 插件、在 Rust 里嵌入时需要）。这个版本是按仓库的 `Cargo.lock` 验证的；用更旧的 Rust 自己解析依赖时，可以在 Cargo 配置里设 `resolver.incompatible-rust-versions = "fallback"`，让 Cargo 选能用的依赖版本。
