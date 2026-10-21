# 行使用 Rust 服务的示例

[English](README.en.md)

一个应用的 TypeScript 和 Python 行按名字使用它已有的 Rust 服务，不写任何 `HostDispatch`（[#440](https://github.com/arcships/rutis/issues/440)）。这是给**改不了源码的类型**的写法：在 `build.rs` 里列出。自己的类型在 `impl` 上加 `#[rutis::service]` 就行，不用 `build.rs`，见 [`examples/marked-services`](../marked-services)。服务在一个对 rutis-bridge 一无所知的依赖 crate 里：[`calendar`](calendar/src)，其中 `Calendar` 给所有行用，`Notes` 每个 `session` 实例一份。

| 文件 | 展示什么 |
| --- | --- |
| [build.rs](build.rs) | 唯一的声明：导出哪些类型、用什么名字，`Notes` 在 `session` 实例里 |
| [src/lib.rs](src/lib.rs) | `rutis_bridge::include_exports!()`：生成的模块 `exports` |
| [rows/services.d.ts](rows/services.d.ts) | 从 Rust 定义生成的 TypeScript 声明（构建会改写它，要提交） |
| [rows/calendar.ts](rows/calendar.ts) | Node 行：serde 结构体参数和返回值、异步方法、带 `code` 和 `data` 的错误、bigint、按引用传的函数、登记在调用方行 `Ctx` 上的订阅 |
| [rows/notes.ts](rows/notes.ts) | 每个 `session` 实例里的 Node 行，使用本实例的 `Notes` |
| [rows/calendar_row.py](rows/calendar_row.py) | 调用同一个 `Calendar` 的 Python 行 |
| [tests/rows.rs](tests/rows.rs) | 宿主：`catalog.export_all(exports::all())`、照常提供的原生服务，以及各项检查（含对声明的 `tsc --strict`） |

在仓库根目录运行：

```sh
npm --prefix node/rutis-runtime ci
python3 -m pip install "websockets==15.*"
cargo test -p rust-services-example
```

用法：[在 Rust 应用里嵌入](../../docs/guide/rust-host.md#第三方类型在-buildrs-里列出)；能传什么、有哪些限制：[Cordis](../../docs/guide/rust-host.md#导出服务参考)。
