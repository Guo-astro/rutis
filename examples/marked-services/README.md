# 标注的 Rust 服务示例

[English](README.en.md)

一个应用的 TypeScript 和 Python 行按名字调用它的 Rust `Calendar`（[#510](https://github.com/arcships/rutis/issues/510)）。`Calendar` 的 `impl` 上有 `#[rutis::service]`，宿主照常 `root.provide(Calendar::new())`；没有 `build.rs`、清单或 `HostDispatch`，这个 crate 只依赖 `rutis`。

| 文件 | 展示什么 |
| --- | --- |
| [src/lib.rs](src/lib.rs) | 带标注的 `Calendar`：serde 参数和返回值、异步方法、`Result` 错误、按引用传的函数；`plan`、`addSource` 的参数各取一种值（字符串、布尔、浮点、整数、可省略、列表、元组、映射、结构体、枚举、插件定义） |
| [rows/services.d.ts](rows/services.d.ts) | `rutis-host types examples/marked-services -o examples/marked-services/rows/services.d.ts --python examples/marked-services/rows/services.py` 写出的声明（要提交） |
| [rows/services.py](rows/services.py) | 同一条命令写出的 Python 类型：服务是 `Protocol`，结构体是 `TypedDict`（要提交） |
| [rows/calendar.ts](rows/calendar.ts) | Node 行，按声明使用 `calendar` |
| [rows/calendar_row.py](rows/calendar_row.py) | 调用同一个服务的 Python 行，按 `services.py` 标注类型 |
| [tests/rows.rs](tests/rows.rs) | 宿主与检查：两种行、声明和 Python 类型是最新的、`tsc --strict`、pyright 严格模式 |

在仓库根目录运行：

```sh
npm --prefix node/rutis-runtime ci
python3 -m pip install "websockets==15.*"
cargo test -p marked-services-example
```

用法与规则：[在 Rust 应用里嵌入](../../docs/guide/rust-host.md#在类型上标注-rutisservice)。
