# 更新记录

[English](CHANGELOG.md)

rutis 的所有包用同一个版本号。每个版本的完整说明在 [docs/releases](docs/releases)，中英两份；[GitHub 发布页](https://github.com/arcships/rutis/releases)上是同样的说明。跨次版本升级前，先看那个版本的迁移指南（说明里有链接）。

| 版本 | 日期 | 变化 | 说明 |
| --- | --- | --- | --- |
| 0.21.0 | 2026-11-05 | 出错时说清楚：值传不过去带原因（`non-finite`、`cycle`、`too-deep`、`type`、`tag`、`limit`），返回值解不开是新的 `bad-result`，参数不对照收解码器的原文，同名服务、等待原因、`SyncWaitCycle` 都说开发者写的那次调用；测试工具给出这一行的 `state(id)`、行名与位置行。**不兼容**：交来的函数默认异步、`rutis::Error::Value` 变体形状、出错文字 | [中文](docs/releases/0.21.0.md) · [English](docs/releases/0.21.0.en.md) |
| 0.20.1 | 2026-11-03 | 修复 bug：`dev` 只对插件会加载的文件重新加载，测试文件、文档、图片、锁文件不再重启行（#1046）；Node 检查器的地址可以用 `RUTIS_NODE_INSPECT` 换（#1048）；`dev` 不再打印 `services: none`（#1049）；一行的清理仍能发出事件，别的行听得到（#1041、#1050）；发布门里的两处偶发失败：`dev` 不再把在回来的路上的行说成 `stopped`，远程运行时的测试等 hello 帧（#1044、#1051） | [中文](docs/releases/0.20.1.md) · [English](docs/releases/0.20.1.en.md) |
| 0.20.0 | 2026-11-02 | 插件可以写可选依赖（`optional` 与 `ctx.get`）；行的 `inject` 算这一行已声明，运行时的行由 rutis 门控并租借；同一进程里的服务在 Go、Kotlin 也拿到对象本身；测试工具装上替身收到的插件定义；卸载时清理与服务撤回走同一个逆序栈；每个实例的常驻占用更小。**不兼容** | [中文](docs/releases/0.20.0.md) · [English](docs/releases/0.20.0.en.md) |
| 0.19.3 | 2026-10-31 | 修 TypeScript 测试工具与插件 API 事件例子的 bug；`loader`、`sessions` 改名 `rutis/loader`、`rutis/sessions`。**不兼容** | [中文](docs/releases/0.19.3.md) · [English](docs/releases/0.19.3.en.md) |
| 0.19.2 | 2026-10-30 | 修复 bug：大量实例关闭时的内存峰值（#977）；提前取消的装载回到 Pending（#968）；放弃的 `reconcile` 不再挡住后来的操作；`dev` 与测试工具的 bug；写错的运行时路径报错 | [中文](docs/releases/0.19.2.md) · [English](docs/releases/0.19.2.en.md) |
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
