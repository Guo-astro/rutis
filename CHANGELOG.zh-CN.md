# 更新记录

[English](CHANGELOG.md)

rutis 的所有包用同一个版本号。每个版本的完整说明在 [docs/releases](docs/releases)，中英两份；[GitHub 发布页](https://github.com/arcships/rutis/releases)上是同样的说明。跨次版本升级前，先看那个版本的迁移指南（说明里有链接）。

| 版本 | 日期 | 变化 | 说明 |
| --- | --- | --- | --- |
| 0.19.1 | 2026-10-30 | 修复文档写明却做不到的几处（`ready()` 加 `reconcile` 后行已装载、`run` 启动时校验配置、uv 项目里的 `rutis-host add`、TypeScript 测试超时后退出）；同步 waterfall 补回为 `waterfall(event).sync(terminal)`；`Loader::instance_rows`；别的语言的插件定义在宿主自己的实例里挂副本 | [中文](docs/releases/0.19.1.md) · [English](docs/releases/0.19.1.en.md) |
| 0.19.0 | 2026-10-29 | 面向宿主持有者：`rutis-host call`、`rutis-host add` / `remove` / `upgrade`、插件经服务 `loader` 管理行；远程运行时、设备与节点改为连接行，背后是一个会话插件（协议 4）；测试工具走真实宿主；平台清理。**不兼容** | [中文](docs/releases/0.19.0.md) · [English](docs/releases/0.19.0.en.md) |
| 0.18.0 | 2026-10-28 | 出错时看得懂：写明哪一行、在做什么、哪个文件哪一行；当前状态与时限；配置按 Schema 校验。**不兼容** | [中文](docs/releases/0.18.0.md) · [English](docs/releases/0.18.0.en.md) |
| 0.17.2 | 2026-10-27 | 开发者体验评估里的修复：坏的值只让这次调用失败、等待的行写明在等什么、`dev` 只重新加载变了的那一行 | [中文](docs/releases/0.17.2.md) · [English](docs/releases/0.17.2.en.md) |
| 0.17.1 | 2026-10-26 | 插件定义补齐 Python、Go、Swift、Kotlin；内核与加载器的修复 | [中文](docs/releases/0.17.1.md) · [English](docs/releases/0.17.1.en.md) |
| 0.17.0 | 2026-10-26 | Rust 写插件只有一种写法，宿主用的放进 `rutis::host`，附 `rutis-migrate`；插件定义可以作为值跨语言传递 | [中文](docs/releases/0.17.0.md) · [English](docs/releases/0.17.0.en.md) |
| 0.16.0 | 2026-10-24 | 保留 0.15.1 的平总线，删掉它的转接层 | [中文](docs/releases/0.16.0.md) · [English](docs/releases/0.16.0.en.md) |
| 0.15.1 | 2026-10-23 | 事件恢复成一条平总线，与 Cordis 相同 | [中文](docs/releases/0.15.1.md) · [English](docs/releases/0.15.1.en.md) |
| 0.15.0 | 2026-10-23 | Swift、Kotlin 插件；iOS、Android 应用托管插件 | [中文](docs/releases/0.15.0.md) · [English](docs/releases/0.15.0.en.md) |
| 0.13.0 | 2026-10-19 | 统一的事件：Rust、叶子插件、Cordis、远程运行时与节点在一条总线上 | [中文](docs/releases/0.13.0.md) · [English](docs/releases/0.13.0.en.md) |
| 0.12.1 | 2026-10-18 | 修复 `build::Exports` 遇到 glob 导入成环时卡住 | [中文](docs/releases/0.12.1.md) · [English](docs/releases/0.12.1.en.md) |
| 0.12.0 | 2026-10-18 | 任何语言提供的服务不用声明就能跨语言使用 | [中文](docs/releases/0.12.0.md) · [English](docs/releases/0.12.0.en.md) |
| 0.11.0 | 2026-10-17 | Node、Bun、Python、Go 插件的事件 | [中文](docs/releases/0.11.0.md) · [English](docs/releases/0.11.0.en.md) |
| 0.10.0 | 2026-10-13 | 质量版本：跨进程、跨平台的测试；`rutis.json` 的格式版本 | [中文](docs/releases/0.10.0.md) · [English](docs/releases/0.10.0.en.md) |
| 0.9.1 | 2026-10-11 | 修复 npm 版 `rutis-host` 启动器的 Ctrl-C | [中文](docs/releases/0.9.1.md) · [English](docs/releases/0.9.1.en.md) |
| 0.9.0 | 2026-10-11 | Bun、Go 插件运行时；拒绝跨运行时成环的同步调用 | [中文](docs/releases/0.9.0.md) · [English](docs/releases/0.9.0.en.md) |
| 0.8.0 | 2026-10-08 | Windows 原生支持、按实例管理插件、所有包同一个版本号 | [中文](docs/releases/0.8.0.md) · [English](docs/releases/0.8.0.en.md) |
| 0.7.0 | 2026-10-06 | 发布列车的首次发布：`rutis-host`、多语言运行时与节点互联整合为一套流程 | [中文](docs/releases/0.7.0.md) · [English](docs/releases/0.7.0.en.md) |

0.7.0 之前的版本只在 [GitHub 发布页](https://github.com/arcships/rutis/releases)上。
