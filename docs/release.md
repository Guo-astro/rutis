# 发布

## 发布列车

除 `rutis-cli`（`cli-v*`，release-cli.yml）之外，这些包同一个版本、一起发布（0.8.0 起包括内核和 dylib 工具链）：

| 注册表 | 包 |
| --- | --- |
| crates.io | `rutis`（内核）、`rutis-bridge`、`rutis-loader`、`rutis-host`；dylib 工具链 `rutis-sdk`、`rutis-dylib`、`rutis-dylib-meta`、`rutis-dylib-launcher` |
| npm | `@arcships/rutis`、`@arcships/rutis-runtime`、`@arcships/rutis-host`（及 `@arcships/rutis-host-{linux,darwin}-{x64,arm64}`） |
| PyPI | `rutis`、`rutis-host`（各平台的 wheel） |
| GitHub Release | `rutis-host` 二进制 |

## 步骤

1. 改版本号：`scripts/train.mjs` 里 `crates` 列出的每个 crate 的 Cargo.toml（以及它们之间、和工作区其他 crate 对它们的依赖版本），`node/rutis`、`node/rutis-runtime`、`node/rutis-host` 的 package.json（`@arcships/rutis-host` 依赖的运行时和平台包版本），`python/rutis/pyproject.toml` 和 `python/rutis/rutis/peer.py` 的 `IMPLEMENTATION`，`crates/rutis-host/pyproject.toml` 里 `rutis` 的范围。`node scripts/train.mjs` 检查它们一致，CI 也会跑。
2. 合并到 main，CI 的 `release-dry-run` 通过（各包都能打包）。
3. 发布前在两台机器上跑一次冒烟（下文）。
4. 打 tag `vX.Y.Z` 并推送。release.yml：核对版本 → 构建四个平台的二进制和 wheel → 对已发布过的 crate 做 semver 检查，按依赖顺序（`node scripts/train.mjs --crates`）发布 crate，再发布 npm 包、PyPI 包 → 创建 GitHub Release（说明取自 `docs/releases/X.Y.Z.en.md`，打 tag 前要写好）。各步只发布注册表里还没有的版本，中途失败时修好后重新运行即可。

需要的配置：GitHub environment `release`（`CARGO_TOKEN`、`NPM_TOKEN`）、`pypi` 和 `pypi-host`。PyPI 上 `rutis` 的 trusted publisher 指向 release.yml 与 environment `pypi`，`rutis-host` 的指向 environment `pypi-host`：两个项目的 publisher 不能完全相同，否则一次发布拿到的令牌只对其中一个有效。

插件 API（`PLUGIN_API`，SDK 与运行时各有一份）只在插件看到的接口不兼容时增加；会话协议版本（`rutisProtocol` 与 `rutis_bridge::session::PROTOCOL`）在线格式不兼容时增加。

## 冒烟

```text
# 监听方（证书对应它的主机名）
cargo run -p rutis-bridge --features websocket --example smoke -- \
    listen 0.0.0.0:7443 --cert server.pem --key server.key --token secret

# 拨号方
cargo run -p rutis-bridge --features websocket --example smoke -- \
    dial wss://<监听方主机名>:7443/rutis --ca ca.pem --token secret
```

要看到：拨号方每秒打印 `clock: <n>`；断网后 30 秒内两边报告心跳超时并等待重连，恢复后 `ready, session <n+1>`；重启监听方后拨号方退避重连；错误的 token 是 `AuthRejected … 403`，不信任的 CA 是 `AuthRejected … UnknownIssuer`。

再用发布的包从零走一遍 [写一个 TypeScript 插件](guide/typescript-plugin.md) 和 [写一个 Python 插件](guide/python-plugin.md)。

每晚的 stress 工作流还跑两个浸泡测试（link 反复断开重连、进程反复拉起结束），检查文件描述符、线程数不增长、进程都被回收。

## 0.8.0

内核和 dylib 工具链从这一版起并入列车，只推 `v0.8.0`；`rutis-v*` tag 和 publish-rutis.yml 停用。`rutis-sdk`、`rutis-dylib`、`rutis-dylib-meta`、`rutis-dylib-launcher` 是首次发布到 crates.io。发布前在 PyPI 上把 `rutis-host` 的 trusted publisher 重新加上，environment 填 `pypi-host`（#154）。

## 0.7.0

0.7.0 是列车的首次发布，基于内核 0.6.1，只推 `v0.7.0`，不打 `rutis-v*`。除 `rutis-loader`（此前有 0.1.0）外各包在注册表上都是新名字：PyPI 需先为 `rutis`、`rutis-host` 配置 pending publisher。发布成功后撤下 crates.io 和 npm 上的旧 `rutis-interop`。
