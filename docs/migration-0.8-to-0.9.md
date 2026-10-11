# rutis 0.8 → 0.9

[English](migration-0.8-to-0.9.en.md) · [发布说明](releases/0.9.0.md)

大多数项目只需要改依赖版本，并把宿主和语言运行时一起升级。自己构造 `Launcher` / `Spawn` / `RowSchema` 的 Rust 代码、在脚本里检查 `rutis-host` 退出码的项目，以及写 dylib 插件的项目，还要看下面对应的小节。

## 改依赖版本

所有 rutis 包都改到 0.9：

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

## 宿主和语言运行时一起升级

协议版本和 PLUGIN_API 都没有变，但 0.9 的宿主转发调用时，调用来源带会话标签（例如 `s1/rust:7`），0.8 的运行时不认识这种格式，会结束会话。宿主（`rutis-host`、或嵌入的 `rutis-bridge` / `rutis-loader`）与 `@arcships/rutis-runtime`、PyPI 的 `rutis` 要用同一个版本。

## 最低版本

| | 0.8 | 0.9 |
| --- | --- | --- |
| Rust | 1.85 | 1.88 |
| Node | 24 | 22 |
| Python | 3.12 | 3.10 |
| websockets（Python 的 `network`） | 13 | 15 |
| Bun | — | 1.4 |
| Go | — | 1.24 |

## `Launcher` 和 `Spawn`：用构造函数

`rutis_bridge::runtime::Launcher` 和 `rutis_bridge::transport::local::Spawn` 新增了 `stdin` / `stdout` / `stderr` 字段，并标为 `#[non_exhaustive]`，不能再用结构体字面量构造（包括 `..Default::default()`）：

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

`Spawn` 用 `Spawn::new(program, peer)` 构造，再给字段赋值：

```rust
let mut spawn = Spawn::new("python", peer);
spawn.args = vec!["-m".into(), "rutis".into()];
spawn.handover = Handover::DialBack;
```

字段仍然是公开的，读取和赋值不受影响。标准流默认值与 0.8 相同：stdin 为 `Stdio::Null`，stdout / stderr 为 `Stdio::Inherit`。

## `RowSchema` 新增 `version`

`rutis_bridge::runtime::RowSchema` 新增字段 `version: Option<String>`（运行时报告的插件版本）。完整写出所有字段的字面量要补上它，或改用 `..Default::default()`。

## `rutis-host` 的退出码与 `check`

- `rutis-host check` 发现某行注入的服务没有任何行提供时以 1 退出。提供方在别的节点上（例如 `peer:` 行）时只提示，不算失败。被禁用的行不再解析，没装插件的禁用行不再让 `check` 失败。
- `run` / `dev` 收到停止信号后，在 `--shutdown-timeout`（默认 10 秒）内没有清理完时以 **2** 退出。只判断"非 0 即失败"的脚本不受影响。
- 运行时进程放在自己的进程组里，终端的 Ctrl-C 只发给 `rutis-host`，由它卸载各行。用 systemd 管理时建议 `KillMode=mixed`。

## 配置

- 以相对路径给出 `rutis.json` 时（`rutis-host run conf/rutis.json`），`./plugin.mjs` 这样的行相对配置文件解析。0.8 里这种写法解析不到文件。
- 监听器只给 `cert` 或只给 `key` 时报错。0.8 会悄悄不用 TLS。
- 运行时名字必须唯一；`rows` 里的未知字段会警告。

## 行为变化

- 本地通道的单条消息上限是 16 MiB，超过时关闭通道。
- Node 行里的插件卸载自己时，loader 收到 `LoaderChanged::SelfDisposed`，这一行转为禁用（实例里的副本是 `Stopped`）。0.8 里它一直显示为 `Running`。
- `python -m rutis` 出错时以 1 退出（0.8 以 0 退出）；Python 的 WebSocket 监听在 `accept()` 失败时以失败结束。

## dylib 插件

SDK 身份随版本变化，**0.8 时期编译的 dylib 插件不能被 0.9 宿主加载**，要用 0.9.0 的 SDK 构建包重新编译。
