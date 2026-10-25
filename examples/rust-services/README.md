# 第三方 Rust 服务示例

[English](README.en.md)

一个应用的 TypeScript 和 Python 行按名字使用**改不了源码的** Rust 服务，不写任何 `HostDispatch`。服务在一个对 rutis-bridge 一无所知的依赖 crate 里：[`calendar`](calendar/src)，其中 `Calendar` 给所有行用，`Notes` 每个 `session` 实例一份。Rust 只允许在定义类型的 crate 里写它的 `impl`，所以应用给每个服务包一层自己的类型，标上 `#[rutis::service]`，方法逐个转发给被包的那个。自己的类型直接在 `impl` 上加标注就行，见 [`examples/marked-services`](../marked-services)。

| 文件 | 展示什么 |
| --- | --- |
| [src/lib.rs](src/lib.rs) | 包好的 `Calendar` 和 `Notes`：标注、逐个转发的方法和它们的文档注释 |
| [build.rs](build.rs) | 每次构建把声明写进 `rows/services.d.ts`（服务本身不需要构建步骤） |
| [rows/services.d.ts](rows/services.d.ts) | 从 Rust 定义生成的 TypeScript 声明（构建会改写它，要提交） |
| [rows/calendar.ts](rows/calendar.ts) | Node 行：serde 结构体参数和返回值、异步方法、带 `code` 和 `data` 的错误、bigint、按引用传的函数、登记在调用方行 `Ctx` 上的订阅 |
| [rows/notes.ts](rows/notes.ts) | 每个 `session` 实例里的 Node 行，使用本实例的 `Notes` |
| [rows/calendar_row.py](rows/calendar_row.py) | 调用同一个 `Calendar` 的 Python 行 |
| [tests/rows.rs](tests/rows.rs) | 宿主：照常提供包好的服务（`Notes` 由实例里的一行提供），以及各项检查（含对声明的 `tsc --strict`） |

在仓库根目录运行：

```sh
npm --prefix node/rutis-runtime ci
python3 -m pip install "websockets==15.*"
cargo test -p rust-services-example
```

用法：[在 Rust 应用里嵌入](../../docs/guide/rust-host.md#第三方类型包一层自己的类型)；能传什么、有哪些限制：[参考](../../docs/guide/rust-host.md#导出服务参考)。
