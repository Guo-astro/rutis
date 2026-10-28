# 迁移到 0.19

[English](migration-0.19.en.md)

0.19 面向宿主持有者。不兼容的只有远程运行时的连接一处（下面第一节）；其余是默认关闭的新选项，各节随 0.19 的改动补齐。

## 远程运行时（#696、#698）

设计：[远程运行时：一条配置写全，一台与多台行为一致](design-remote-runtime-consistency-2026-10-26.md)。

### 新写法：`runtimes.remote` 一条写全

以前一台远程运行时要写两处：`runtimes.remote` 一条，再加一条带 `runtime` 的 `rutis-bridge/peer` 行，两处之间不核对，少写一处行就一直等待。0.19 起一条写全：

```json
// 0.18
"runtimes": { "remote": [{ "name": "gpu", "language": "python" }] },
"rows": [
  { "id": "gpu", "name": "rutis-bridge/peer", "config": { "peer": "gpu", "dial": "wss://gpu.example.com:7443/rutis", "runtime": "gpu" } },
  { "id": "embedder", "name": "gpu:embedder" }
]

// 0.19
"runtimes": { "remote": [{ "name": "gpu", "language": "python", "dial": "wss://gpu.example.com:7443/rutis" }] },
"rows": [
  { "id": "embedder", "name": "gpu:embedder" }
]
```

| 0.18 的节点行 | 0.19 的 `runtimes.remote` |
| --- | --- |
| `"dial": "<地址>"` | `"dial": "<地址>"` |
| `"listen": "<监听器>"` | `"listen": "<监听器>"`（不带 `instances`：一台，对方的 `--id` 是运行时名） |
| `"transport": "<kind>"` | `"transport": "<kind>"`（默认 `websocket`） |

- 多台（`listen` + `instances`）的写法不变。
- **旧写法照常可用，不用改，也不会被转换。** 同一个运行时不要两种都写：`runtimes.remote` 有 `dial` 或 `listen`，又有一条 `"runtime"` 指向它的节点行时，宿主启动报错：

  ```
  remote runtime gpu: link row gpu connects it too: remove the link row, as runtimes.remote connects it itself
  ```

- 旧写法里节点行的 `export`、`import`、`host`、`rows` 等字段，新写法没有：连接只用来运行这个运行时的行。还要这些字段的，继续用节点行。
- token 规则不变：`RUTIS_TOKEN_<名字>`，没有时用 `RUTIS_TOKEN`。新写法的一台 `listen` 没有 token 时启动报错。
- 自建的连接方式（中继、QUIC 等）实现 `Transport`，由嵌入 rutis-host 的 Rust 程序挂上（`Embedding` 新增的 `transports`），`transport` 写它的名字；一台、多台都用它。

### 行为变化

1. **一台远程运行时缺插件，只是这一行失败。** 远端没有某一行的插件时，以前这一行解析失败（`unresolved: no plugin named "gpu:embedder"`），编辑时新增这样的行被拒绝；现在和多台一样：只有这一行失败，其他行照常，新增这样的行不被拒绝，运行时下次连上时重新检查。`run` 打印的变成：

   ```
   embedder: unresolved: entry "embedder" rejected: runtime gpu: this connection does not contain plugin embedder
   ```

   用 Rust 嵌入、按 `LoaderError::NotFound` 判断这种情况的代码，改为 `rutis_loader::report::Unavailable::is(&error)`。

2. **多台缺插件的文字多了运行时名。** `this connection does not contain plugin X` 改为 `runtime <名>: this connection does not contain plugin X`（只按后半句匹配的不受影响）。

3. **只写 `listen`、不写 `instances` 不再报错。** 以前启动报 `listen needs instances`，现在是一台运行时连进来。

4. **运行时缺行约定时的文字统一。** 一台和多台都写 `the <名> runtime lacks rows.v2 and hosts: a runtime speaking the row contract is required (for Node, @arcships/rutis-runtime 0.3.0 or later)`。

5. **新的输出。** 新写法的连接状态行以 `runtime <名>` 开头（旧写法的节点行仍用行 id）；多台的连接开始打印上线、断开、被拒：

   ```
   runtime gpu: link connected
   runtime gpu: link down (gpu.example.com:7443: Connection refused); retrying in 2.1s
   runtime worker: connected worker-1
   runtime worker: disconnected worker-1 (connection lost)
   runtime worker: refused (the worker runtime lacks rows.v2 and hosts: …)
   ```

   `rutis-host status` 在有这类运行时时多出 `connections:` 一节。读 `run` 输出的脚本按行首前缀区分，不受新行影响。

### Rust 嵌入

- `rutis_host::host::Embedding` 新增字段 `transports`。用结构体字面量写全所有字段的代码要加上 `transports: Vec::new()`，或改用 `..Embedding::default()`。
- `rutis_host::control::serve` 在 `Loader` 之后多了一个参数 `Report`（一个返回状态文字的函数，`status` 用它回答）；用 `Host::status_report()` 得到它。
- 运行时一侧、会话协议与 rutis、rutis-loader、rutis-bridge 的 Rust API 都不变。
