# 迁移到 0.18

[English](migration-0.18.en.md)

0.18 让出错的时候看得懂（[诊断设计](design-diagnostics-2026-10-26.md)）。本页随 0.18 的改动逐项补齐；下面是已经完成的部分。

## 错误（#677）

### 所有语言

- **出错文字会变。** 行失败时显示插件自己的话，前面不再有 `plugin failed: `；rutis 自己的报错按诊断设计 §4.5 改写。按文字匹配错误的代码请改为按错误码判断。
- **rutis 自己的 `BindingError` 带错误码。** 类型名不变，新增 `code` 与 `data`（各语言读法与插件抛出的错误相同：JavaScript `error.code`、Python `error.code`、Go `RemoteError.Code`……）：

  | code | 什么时候 | `data` |
  | --- | --- | --- |
  | `not-declared` | 用了没写进 `inject` 的服务 | `{service}` |
  | `unavailable` | 服务现在没有提供者 | `{service, reason}` |
  | `no-method` | 方法不存在 | `{service, method, methods}` |
  | `bad-argument` | 参数个数或类型不对 | `{expected, received}` 等 |
  | `bad-value` | 值传不过去，或消息超过上限 | — |
  | `peer-gone` | 调用进行中对面的运行时退出或断开 | — |

  `not-declared` 与各语言 SDK 自己产生的那几类随后补上；协议层面的拒绝（一端没按约定办事）没有码。

### Rust 宿主

- **`ErrorKind::PluginFailed` 显示时没有 `plugin failed: ` 前缀。**
- **插件的错误不再是字符串。** 行失败、调用插件的服务失败时，用 `rutis_bridge::thrown(&error)` 读出插件抛出的错误（`name`、`message`、`code()`、`data()`、`stack()`、`site()`：是哪一行、在做什么）。loader 的 `Failure`、`ReconcileReport` 不变。
- **`rutis_bridge::session::Error` 新增分支 `Binding { code, message, data }`。** rutis 拒绝一次调用时用它：方法不存在、参数不对、服务不在（带码），以及协议层面的拒绝（`code` 为 `None`）。`Error::Value` 只表示值传不过去。对 `session::Error` 逐个分支 `match` 的代码要加上这一支；判断是哪一类时用 `Error::code()`（`Value` 为 `bad-value`，`Transport` 为 `peer-gone`）：

  | 0.17 | 0.18 |
  | --- | --- |
  | `Err(Error::Value(m)) if m.starts_with("unknown service method")` | `Err(e) if e.code() == Some("no-method")` |
  | `Err(Error::Value(m)) if m.contains("is not available")` | `Err(e) if e.code() == Some("unavailable")` |
  | `Err(Error::Value(m))`（值传不过去） | 不变；或 `e.code() == Some("bad-value")` |
