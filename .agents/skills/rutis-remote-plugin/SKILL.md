---
name: rutis-remote-plugin
description: Use when a rutis plugin must run on another machine, device or app instead of next to the host — "remote plugin", "远程插件", running a Python/Node/Go plugin on a GPU box, connecting two rutis hosts, exposing a phone app's capabilities. Walks through choosing node link vs remote runtime vs app hosting, writing the plugin in the rutis paradigm, deploying, configuring rutis.json, and the rules that keep it inside the rutis design.
---

# 在 rutis 里添加远程插件

先记住一句话：**rutis 里没有"远程插件"这种东西，只有普通插件，加上一个跑在别处的运行时或节点。** 插件代码与本地完全相同，变的只是它的运行时在哪、rutis 怎么连上它。不要为远程另写一套插件 API、协议或生命周期。

## 1. 先判断是哪一种

| 对方是什么 | 用什么 | rutis 侧的行 |
| --- | --- | --- |
| 另一台机器上的某种语言的运行时，插件由**本 rutis** 装载、配置、启停 | 远程运行时（link + `runtime`） | `<运行时名>:<插件>` |
| 另一个完整的 rutis 宿主，它自己管自己的插件，只共享服务 | 节点 link（`export` / `import`） | 无新行；`import` 的服务按名字可用 |
| 另一个完整的 rutis 宿主，要在它那里装本端写的行 | 节点 link（对端 `host: true`，本端 `rows: true`） | `peer:<对端>/<插件>` |
| 手机 / 桌面应用把自身能力作为插件提供 | 应用托管（[应用接入](../../../docs/design-app-integration-2026-10-14.md)） | `<运行时名>:<插件>`，每条连接一个实例 |

拿不准时问一个问题：**插件该由谁决定装不装、用什么配置？** 本 rutis 决定 → 远程运行时 / 应用托管；对方自己决定 → 节点 link。

## 2. 当前能力（写代码前核对，以代码为准）

| 语言 | 远程监听 `listen:`（rutis 拨入） | 拨出 `dial:`（运行时连 rutis） |
| --- | --- | --- |
| Python | ✅ `python -m rutis listen:wss://…` | 设计已定，未实现（#383） |
| Node / TS | ✅ `node …/@arcships/rutis-runtime/src/runner.mjs listen:wss://…` | 未实现（#384） |
| Go | ✅ 插件二进制 `listen:wss://…` | 未实现（#384） |
| Bun | ❌ 远程都不支持（#384） | ❌ |
| Swift / Kotlin 应用 | 设计已定，未实现（#389、#391） | 同左 |

"一个接入点接多条连接、每条连接一个实例"（多设备、扩缩容）是 #382，未实现。现在一个远程运行时 = rutis 配置里静态写一条。

## 3. 写插件：按范式，和本地一样

- 用该语言的 SDK 写叶子插件：声明 `inject`（用到的服务名）、`provides`（提供的服务名与每个方法的 `sync` / `async`）、配置 schema；`apply(ctx, config)` 里 `ctx.use` / `ctx.provide` / `ctx.effect`。见 `docs/guide/plugin-api.md` 与 `docs/guide/<语言>-plugin.md`。
- 插件**不**决定自己何时运行、不重连、不探测对端是否在线：服务都就绪才 `apply`，任一撤销就停下，回来再启动——远程断线对插件就是一次普通的停止。
- 值按跨进程规则传（JSON 兼容数据复制，函数与对象按引用，整数 ±2^53−1，嵌套 ≤ 64 层）；不要假设同进程。
- 调用别的服务时不要持锁（叶子运行时可重入，你的服务可能在自己的同步调用途中被再次调用）。
- 依赖声明里只写服务名，不写语言、机器、地址。

## 4. 部署（远程运行时，当前的监听方式）

在远程机器上：

1. 把插件**装好**：Python `pip install` 进运行时用的解释器；Node 在项目里 `npm install`；Go 编好二进制。远程运行时只装载已部署的插件，**没有代码上传**。
2. 准备 TLS：非回环地址必须 `wss://`。`RUTIS_CERT` / `RUTIS_KEY` 指向证书与私钥；自签证书时把 CA 交给 rutis 侧的 `RUTIS_CA`。
3. 设 token：`RUTIS_TOKEN`（与 rutis 侧 `RUTIS_TOKEN_<运行时名大写>` 相同）。
4. 启动并监听，`--id` 是本运行时的名字，`--peer` 是控制它的 rutis 的名字：

   ```bash
   RUTIS_TOKEN=… RUTIS_CERT=server.pem RUTIS_KEY=server.key \
     python -m rutis listen:wss://0.0.0.0:7443/rutis --id gpu --peer main /srv/plugins
   ```

## 5. rutis 侧配置

```json
{
  "runtimes": { "remote": [{ "name": "gpu", "language": "python" }] },
  "rows": [
    { "id": "gpu", "name": "rutis-bridge/peer",
      "config": { "peer": "gpu", "dial": "wss://gpu.example.com:7443/rutis", "runtime": "gpu" } },
    { "id": "embedder", "name": "gpu:embedder", "config": { "model": "small" } }
  ]
}
```

- `language`：`python` / `py` / `go` / `node`。
- link 行必须带 `"runtime": "<名>"`，否则对端不会被当成运行时，`<名>:*` 行永远不就绪。
- 行名 `<运行时名>:<插件>`；插件在远程机器上解析。
- 节点 link（`export` / `import` / `host` / `rows`）的写法见 `docs/guide/nodes.md`。

## 6. 验证

- `rutis-host check`：远程运行时的行检查时不连接，缺的服务只提示"可能来自远程"，不算失败。
- `rutis-host run`：看到 `<link 行 id>: link connected`，随后 `<行 id>: running`。
- `link refused: auth-rejected` → token 或证书不对；`link stopped: incompatible` → 协议、地址写法或缺 `runtime`。
- 断开远程进程：rutis 侧这些行停下、用到它们的行停下；恢复后自动重连并重新装载。

## 7. 不要做的事（越过 rutis 设计边界）

| 不要 | 因为 |
| --- | --- |
| 在插件里实现重连、心跳、离线缓存来"保持远程可用" | 连接与生命周期归 rutis；插件只看服务在不在 |
| 往远程运行时上传代码、按路径装载插件 | 远程设计 §7：只装已部署的插件 |
| 非回环用 `ws://` | 非回环必须 TLS；rutis 拨号端会直接拒绝 |
| 在协议或插件里建模"设备""身份" | 设备信息是插件提供的服务；身份属于将来的管理服务 |
| 给同名服务起别名来区分多台机器 | 多连接同名插件用实例区分（#382） |
| 为远程另写插件 API 或让 SDK 依赖 Rust | 运行时侧全部用平台语言实现 |
| 在 SDK 里处理手机前后台、保活 | 业务层的事；SDK 只给 `start` / `stop` |

## 参考

- `docs/guide/nodes.md`（link、远程运行时、安全）、`docs/guide/rutis-host.md`（配置与输出）、`docs/guide/plugin-api.md`
- 设计：`docs/design-remote-plugins-2026-10-03.md`、`docs/design-remote-runtime-dial-2026-10-14.md`（拨出与动态接入）、`docs/design-app-integration-2026-10-14.md`
- 跟踪：#392
