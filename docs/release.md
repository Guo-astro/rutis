# 发布

## 发布列车

除 `rutis-cli`（`cli-v*`，release-cli.yml）之外，这些包同一个版本、一起发布（0.8.0 起包括内核和 dylib 工具链）：

| 注册表 | 包 |
| --- | --- |
| crates.io | `rutis`（内核）及其宏 `rutis-macros`、`rutis-bridge`、`rutis-loader`、`rutis-host`；dylib 工具链 `rutis-sdk`、`rutis-dylib`、`rutis-dylib-meta`、`rutis-dylib-launcher`；迁移工具 `rutis-migrate` |
| npm | `@arcships/rutis`、`@arcships/rutis-runtime`、`@arcships/rutis-bun`、`@arcships/rutis-host`（及 `@arcships/rutis-host-{linux,darwin}-{x64,arm64}`、`@arcships/rutis-host-win32-x64`） |
| PyPI | `rutis`、`rutis-host`（各平台的 wheel） |
| Go | `github.com/arcships/rutis/go/rutis`（tag `go/rutis/vX.Y.Z`，由 release.yml 打） |
| Maven Central | `io.github.arcships:rutis`（Kotlin Multiplatform：jvm、android）、`io.github.arcships:rutis-ksp`（没有凭据时跳过，见下文） |
| GitHub Release | `rutis-host` 二进制 |

## 步骤

1. 改版本号：`scripts/train.mjs` 里 `crates` 列出的每个 crate 的 Cargo.toml（以及它们之间、和工作区其他 crate 对它们的依赖版本），`node/rutis`、`node/rutis-runtime`、`bun/rutis-bun`、`node/rutis-host` 的 package.json（`@arcships/rutis-host` 依赖的运行时和平台包版本），`python/rutis/pyproject.toml` 和 `python/rutis/rutis/peer.py` 的 `IMPLEMENTATION`，`go/rutis/version.go` 的 `Version`，`kotlin/rutis/gradle.properties` 的 `version` 和 `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/Rutis.kt` 的 `VERSION`（Kotlin SDK，发布到 Maven Central），`swift/rutis/Sources/Rutis/Version.swift` 的 `sdkVersion`（Swift SDK，随根仓库的 `vX.Y.Z` tag 发布），`crates/rutis-host/pyproject.toml` 里 `rutis` 的范围。`node scripts/train.mjs` 检查它们一致，CI 也会跑。指南里写出的版本号（`README.md`、`README.zh-CN.md`、`docs/guide/` 的 `rust-host`、`cordis`、`typescript-plugin`、`python-plugin`、`README` 等，搜上一个版本号）也在这个 PR 里改，两种语言都改。Python 插件对 `rutis` 的下限（`python-plugin`、`plugin-api` 里的 `rutis>=0.17`，#672）不改，只在插件 API 变化时提高；`rutis-host new` 的模板写的是生成它的 rutis-host 自己的版本，不用手改。同一个 PR 里保存本版本的持久化样本（`tests/fixtures/persisted/`，见下文[持久化样本](#持久化样本)）。
2. 合并 release PR（PR 上 CI 的 `release-dry-run` 通过，各包都能打包）。不用再等 main 上或每晚的运行：打 tag 后 release.yml 自己跑全部 CI 任务（第 4 步）。想在打 tag 前把整个发布门先走一遍，在分支上运行 `gh workflow run release.yml --ref <分支> -f dry-run=true`（可加 `-f tag=vX.Y.Z` 核对版本号）：发布门的任务都跑，发布、GitHub Release 和发布后验证都跳过，不用到任何 secret 和 environment。
3. 发布前在两台机器上跑一次网络冒烟（下文）。
4. 打 tag `vX.Y.Z` 并推送。release.yml：核对版本 → **发布门**，以下同时开始，全部通过才发布：全部 CI 任务（调用 `ci.yml`，`all=true`、`release=true`；各任务跑在 `ci.yml` 给它的 runner 上：Linux 任务按仓库变量 `LINUX_RUNNER`，没设置时是 GitHub 的 runner，macOS、Windows 任务见 [CI](ci.md) §10、§11；跨版本测试只和比这个版本旧的发布版本对接，见下文）；构建五个平台的二进制、wheel 和 npm 包（package.yml，与每晚的运行同一份构建步骤），再在五个平台上从这些文件安装并运行（install.yml）；Windows 上的 dylib 测试（dylib-windows.yml，含 SDK 可复现检查）。有一项不通过就什么都不发布 → 对已发布过的 crate 做 semver 检查，按依赖顺序（`node scripts/train.mjs --crates`）发布 crate，再发布 npm 包、PyPI 包（发布的就是发布门装过的文件；发布和发布后的验证都明确使用官方的 crates.io、npm、PyPI 和 Go 模块代理，不受 runner 上镜像配置的影响），Go 模块的 tag `go/rutis/vX.Y.Z` 也等发布门通过才打 → 创建 GitHub Release（说明取自 `docs/releases/X.Y.Z.en.md`，打 tag 前要写好）→ **发布后验证**：等注册表能取到这个版本（`node scripts/train.mjs --npm` 列出的每个 npm 包都要能取到：`@arcships/rutis`、`@arcships/rutis-runtime`、`@arcships/rutis-bun`、`@arcships/rutis-host` 和 `@arcships/rutis-host` 的 optionalDependencies 列出的五个平台包；最多等 15 分钟，超时时列出还取不到的包），在 Linux 上从注册表安装 npm、PyPI、二进制三种渠道并运行，`cargo install` 和 Go 模块，再照着指南里标记过的示例走一遍。各步只发布注册表里还没有的版本，中途失败时修好后重新运行即可（重新运行失败的任务，或 `gh workflow run release.yml --ref vX.Y.Z -f dry-run=false`：`dry-run=false` 只能在 tag 上运行）。重新运行时这个版本可能已经发布了一部分：发布门里的跨版本测试（`tests/e2e/tests/cross_version.rs`）因此设了 `RUTIS_E2E_ONLY_OLDER=1`，不和这个版本本身对接，只和更早的发布版本对接。
5. 发布开始后任何一步失败时（包括 `go` 或 `github` 失败让发布后验证被跳过、只发布了一部分），release.yml 自动开一个 issue（指派给维护者；发布门失败时什么都没发布，不开 issue，失败的运行本身写明原因），正文列出每个任务的结果。看运行的日志，判断是包本身坏了还是验证或注册表的问题：包坏了时，npm 上 `npm deprecate @arcships/<包>@X.Y.Z "<原因>"`，PyPI 上在项目页面 yank 这个版本，crates.io 上 `cargo yank --version X.Y.Z <crate>`，然后发布修正版本 X.Y.(Z+1)；不撤回已经发布的文件。

需要的配置：GitHub environment `release`（`CARGO_TOKEN`、`NPM_TOKEN`）、`pypi` 和 `pypi-host`。PyPI 上 `rutis` 的 trusted publisher 指向 release.yml 与 environment `pypi`，`rutis-host` 的指向 environment `pypi-host`：两个项目的 publisher 不能完全相同，否则一次发布拿到的令牌只对其中一个有效。

Maven Central（release.yml 的 `maven` 任务，vanniktech maven-publish 插件）用 GitHub environment `maven` 的四个 secret；缺任何一个，这一步在运行摘要里写明缺哪些并跳过，其余照常发布。维护者一次性配置：

1. 用 GitHub 账号登录 [Central Portal](https://central.sonatype.com)，在 Namespaces 里添加 `io.github.arcships`，按页面给的验证码在 arcships 组织下建一个同名的公开空仓库完成验证（验证后可删）。
2. 在 Account → Generate User Token 生成用户令牌：其 username、password 分别是 `MAVEN_CENTRAL_USERNAME`、`MAVEN_CENTRAL_PASSWORD`。
3. 生成签名用的 PGP 密钥并公开公钥：`gpg --quick-gen-key "arcships rutis" rsa4096 sign 0`，`gpg --keyserver keyserver.ubuntu.com --send-keys <密钥 id>`。`gpg --armor --export-secret-keys <密钥 id>` 的全部输出是 `MAVEN_SIGNING_KEY`，口令是 `MAVEN_SIGNING_KEY_PASSWORD`。
4. 写入 environment：`gh secret set MAVEN_CENTRAL_USERNAME --env maven`，其余三个同样（`gh secret set MAVEN_SIGNING_KEY --env maven < key.asc`）。

发布的是 `io.github.arcships:rutis`（含 `rutis-jvm`、`rutis-android`）与 `io.github.arcships:rutis-ksp`，自动 release；任务等部署发布完成才结束（最多一小时）；Central Portal 已发布这个版本的模块跳过（经 Portal 的 publisher API 查询，不看会滞后的 repo1.maven.org）。Portal 要发布完成（上传后 10～30 分钟）才说已发布，所以上传前先在缓存里记下这个版本上传过：重跑时若之前上传过、模块还没发布，先等它发布（最多 45 分钟），不重复上传；到时仍未发布（部署失败，或上次没真正上传）才重新上传。所以重跑安全。Maven Central 同步到搜索页可能要几十分钟，发布后验证不等它。

插件 API（`PLUGIN_API`，SDK 与运行时各有一份）只在插件看到的接口不兼容时增加。Python 插件对 `rutis` 只写下限，和运行时共用宿主环境里的同一个 `rutis`，所以 Python 的 `rutis` 在 `PLUGIN_API` 不变时不删、不改插件导入的名字：要去掉的名字保留下来，用到时给出弃用警告，到 `PLUGIN_API` 增加时才删。`PLUGIN_API` 增加时，运行时要继续加载旧的插件 API（像 0.13 继续加载 API 2 那样）；做不到时，拒绝的报错要写明插件用哪个版本的 SDK 重新构建。会话协议版本（`rutisProtocol` 与 `rutis_bridge::session::PROTOCOL`）在线格式不兼容时增加。

## 持久化样本

`tests/fixtures/persisted/<版本>/` 存每个发布版本写下的持久化数据，测试用当前代码读每一份，结果必须与当时读出的一样（风险 J3）：

| 文件 | 内容 | 测试 |
| --- | --- | --- |
| `rutis.json` | 一份尽量用到该版本所有字段的配置 | `cargo test -p rutis-host --test persisted` |
| `rutis.expected.json` | 该版本读出的结果（规范化，路径相对样本目录写成 `<dir>`） | 同上 |
| `cordis.patch.yml` | dsh 的用户层，尽量用到该版本支持的写法 | `cargo test -p rutis-dsh --test persisted` |
| `cordis.patch.expected.json` | 该版本读出的 patch 列表 | 同上 |

发布时：

1. 把上一个版本的目录复制成 `tests/fixtures/persisted/X.Y.Z/`；这一版给 `rutis.json` 或用户层加了字段、写法的，在样本里用上。
2. 用本版本的代码重新生成本版本的期望：`RUTIS_PERSISTED_WRITE=X.Y.Z cargo test -p rutis-host --test persisted` 和 `RUTIS_PERSISTED_WRITE=X.Y.Z cargo test -p rutis-dsh --test persisted`（只写 `X.Y.Z/` 下的期望文件）；检查差异，再不带变量跑一遍。
3. 旧版本的样本和期望不改。旧样本读不出原来的结果，就是不兼容变化（[质量规范](quality-standard.md) §9.3）；决定不再支持某个格式时，把样本移到 `rejected/<名字>/`，旁边的 `rutis.error` 写明错误信息，信息里要说明怎么迁移。

`rutis.json` 有格式版本（顶层 `format`，当前是 1）：改了它的含义就把 `FORMAT`（`crates/rutis-host/src/config.rs`）加一，写进发布说明。dsh 的 `cordis.patch.yml` 是 Cordis 的配置格式，没有版本，兼容只靠这些样本。

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

[写一个 TypeScript 插件](guide/typescript-plugin.md) 和 [写一个 Python 插件](guide/python-plugin.md) 都由发布后验证照着走（标记过的示例，`tools/doc-examples.mjs`）。

每晚的 stress 工作流还跑两个浸泡测试（link 反复断开重连、进程反复拉起结束），检查文件描述符、线程数不增长、进程都被回收。

## 0.17.2

0.17.2 是补丁版本：修复开发者体验评估（#663）里发现的、在用的人会碰到的问题：#665、#666、#667、#668、#672、#675、#676 的前半、#682（第 4 条挪到 0.18）、#701（#703–#707 及验收后的修复）。所有包均为 0.17.2；见[发布说明](releases/0.17.2.md)。没有迁移指南：没有 API 删除或改名。

补丁版本要和 0.17.1 兼容：`cargo-semver-checks` 与 0.17.1 比较，不接受不兼容的 Rust API 改动（这一版只新增 API）。`rutis.json` 的 `format` 仍是 1，配置没有变化：持久化样本从 0.17.1 复制，期望不变；dsh 的用户层没变，`cordis.patch.yml` 原样复制。协议不变，没有新的能力；发布门的跨版本测试与 0.17.1 相遇，新增的一格 `plugins_of_a_previous_sdk_under_the_current_runtimes` 检查上一版 SDK 构建的插件在当前运行时下运行。Python 插件从这一版起只写 `rutis` 的下限（#672）：插件 API 版本不变时，Python 的 `rutis` 不删、不改插件导入的名字；指南里安装 Python 运行时的 `rutis==<版本>` 每次发布照常改。

## 0.17.1

0.17.1 是补丁版本：插件定义补上 Python、Go、Swift、Kotlin（#642 的 P2、P3：#654、#653），并修复 #619、#620（#652）与 #621（#651）；事件符合性用例 `rutis/row` 的偶发失败（#659，发布门里出现过）是测试的等待不够，由 #661 修正测试执行器。所有包均为 0.17.1；见[发布说明](releases/0.17.1.md)。没有迁移指南：没有 API 删除或改名。

补丁版本要和 0.17.0 兼容：`cargo-semver-checks` 与 0.17.0 比较，不接受不兼容的 Rust API 改动（这一版的 Rust 改动都在内部）。`rutis.json` 的 `format` 仍是 1，配置没有变化：持久化样本从 0.17.0 复制，期望不变；dsh 的用户层没有变化，`cordis.patch.yml` 原样复制。协议不变；发布门（`RUTIS_E2E_ONLY_OLDER=1`）的跨版本测试与 0.17.0 相遇。0.17.0 的宿主已经声明 `plugins`，所以 0.17.1 的 Python、Go、Swift、Kotlin 运行时接在 0.17.0 宿主下也能交出插件定义；跨版本测试没有插件定义的场景，只按 `events-v2` 判断，不用改。`persisted` 测试要求存在的样本列表补上 0.16.0、0.17.0、0.17.1（之前两个版本漏了）。

## 0.17.0

0.17.0 收敛 Rust 的公开 API（#617、#643），删除 `build.rs` 的服务清单（#647），并新增插件定义（#642 的 P1：Rust、Node、Bun）。所有包均为 0.17.0；见[发布说明](releases/0.17.0.md)与[迁移到 0.17](migration-0.17.md)。

迁移工具 `rutis-migrate` 首次发布到 crates.io，排在列车最后（它不依赖其他 rutis crate）：`CARGO_TOKEN` 要能发布新的 crate。0.x 的次版本号变化允许不兼容的改动，所以 `cargo-semver-checks` 与 0.16.0 比较时接受 Rust API 的改动。

`rutis.json` 的 `format` 仍是 1，配置没有变化：持久化样本从 0.16.0 复制，期望不变；dsh 的用户层没有变化，`cordis.patch.yml` 原样复制。协议只新增能力 `plugins`，两端都声明才用；发布门（`RUTIS_E2E_ONLY_OLDER=1`）的跨版本测试与 0.16.0 相遇，0.16.0 一端不声明 `plugins`，会话照常工作，只是不传插件定义。

## 0.16.0

0.16.0 保留 0.15.1 的平总线，删掉那个版本为了保持源码兼容留下的转接层（`Dispatch::scope`、各 SDK 的作用域辅助函数与测试工具的发出方作用域）。同时带上 0.15.1 之后就绪的修复：带 cfg 的服务方法、导出中的泛型别名实参、dev 时混合语言行的重载，以及 `Ctx::is_shutting_down()`。所有包均为 0.16.0；见[发布说明](releases/0.16.0.md)与[迁移到 0.16](migration-0.16.md)。

`rutis.json` 的 `format` 仍是 1，配置没有变化：持久化样本从 0.15.1 复制，期望不变。事件线上格式不变；发布门（`RUTIS_E2E_ONLY_OLDER=1`）的跨版本测试与 0.15.1 相遇。仍在 0.15.1 的一端会继续按自己的作用域过滤，所以要让各处都按平总线送达，所有端都要升级。

## 0.15.1

事件恢复成一条平总线（#604、#605）：送达只按名字选监听器，作用域只是过滤用的描述。这是相对 0.13–0.15.0 的行为变化。`Dispatch::scope`、各 SDK 的作用域辅助函数与测试工具的发出方作用域保留源码兼容，但被忽略。所有包均为 0.15.1；见[发布说明](releases/0.15.1.md)与[迁移到 0.15.1](migration-0.15.1.md)。

`rutis.json` 的 `format` 仍是 1，配置没有变化：持久化样本从 0.15.0 复制，期望不变。dsh 的用户层没有变化，`cordis.patch.yml` 原样复制。事件线上格式不变；发布门（`RUTIS_E2E_ONLY_OLDER=1`）的跨版本测试与 0.15.0 相遇，会话正常工作，但仍在 0.15.0 的一端会继续按自己的作用域过滤，所以要让各处都按平总线送达，所有端都要升级。

## 0.15.0

`rutis-macros`（`#[rutis::service]`，#510）首次发布到 crates.io，在列车里排在 `rutis` 之前（`rutis` 依赖它）：`CARGO_TOKEN` 要能发布新的 crate。Kotlin SDK（`io.github.arcships:rutis`、`io.github.arcships:rutis-ksp`）首次发布到 Maven Central；environment `maven` 缺 secret 时这一步跳过并在运行摘要里写明，其余照常发布。Swift 包没有注册表，SwiftPM 直接取 `v0.15.0` tag（第一个带根 `Package.swift` 的版本）。0.14 没有单独发布，它的内容在这一版里。

`rutis.json` 的 `format` 仍是 1，持久化样本从 0.13.0 复制，加上 0.13 之后新增的写法：`runtimes.go` 的 `restart`（`"on-failure"`），`runtimes.swift`（`binaries`、`idle`、`restart: "exit"`，行 `swift-MacKit:clipboard`），远程运行时的 `language` 用 `"swift"`、`"kotlin"`（连进来的 iPhone、Android 应用，各配一个 instanced 分组）和 `"kt"`（宿主拨号的应用，配一个节点行）。persisted 测试的期望照旧只记不是默认值的 `restart`，`swift` 与 `go` 记同样的字段，旧样本的期望不变。dsh 的用户层自 0.13.0 以来没有变化，`cordis.patch.yml` 原样复制。发布门（`RUTIS_E2E_ONLY_OLDER=1`）的跨版本测试与 0.13.0 相遇，两边都声明 `events-v2`，相遇正常工作（`bridge/cross-version-meetings`），早于 0.13 的版本仍被拒绝（`events/old-peer-refused`）。

## 0.13.0

第一个带统一事件（events-v2）的版本。`rutis.json` 的 `format` 仍是 1，持久化样本从 0.12.1 复制，节点行 `office` 改用 0.13 的写法：`export` 加上事件前缀 `order/*`，新增 `import: ["stock/changed"]`（事件与服务共用 `export` / `import`，一项可以是以 `*` 结尾的前缀）；节点行的配置原样交给加载器，期望里就是这两处。dsh 的用户层自 0.12.1 以来没有变化，`cordis.patch.yml` 原样复制。统一事件（#464）删除了节点行（`rutis-bridge/peer`）配置里的 `events: { out, in }`：0.12.1 及更早的样本都没有写它，期望不变；写了它的旧文件，`rutis-host check` 与启动都拒绝，信息给出改写（`the peer config field events was removed in rutis 0.13: …`，见[迁移到 0.13](migration-0.13.md#节点连接)）。这是第一次因为删除一种写法往 `rejected/` 放样本：`rejected/peer-events/` 的 `rutis.json` 有一个写着 `events` 的节点行，旁边的 `rutis.error` 是这条信息。这也是第一次跨版本相遇只验证拒绝：0.13 与每个更早的发布版本在握手时互相拒绝（`events/old-peer-refused`）；发布门（`RUTIS_E2E_ONLY_OLDER=1`）与 0.12.1 相遇，全部是拒绝。从 0.13.1 起上一版本也声明 `events-v2`，相遇恢复为正常工作（`bridge/cross-version-meetings`）。

## 0.12.0

持久化样本的 `rutis.json` 加上 0.11 之后新增的 `restart`：`runtimes.node` 为 `"on-failure"`，`runtimes.py` 为 `"exit"`，`runtimes.bun` 为 `"no"`（默认值）；`format` 仍是 1。persisted 测试的期望记下不是默认值的 `restart`，所以 `"no"` 不出现在期望里，旧样本的期望不变。dsh 的用户层自 0.11.0 以来没有变化，`cordis.patch.yml` 原样复制。这也是第一次在发布门里跑跨版本相遇（#426，CI 的 `e2e` 任务）：发布门设了 `RUTIS_E2E_ONLY_OLDER=1`，只和更早的发布版本对接，这一版是 0.11.0。

## 0.11.0

为了让 leaf 插件事件（#343）早点可用而提前发布的一版。持久化样本的 `rutis.json` 加上 0.10 之后新增的远程运行时字段：`runtimes.remote[]` 的 `listen` 与 `instances`（连进来的运行时，配一个顶层 `instanced` 分组）和 `"language": "bun"`；`format` 仍是 1。persisted 测试的期望从这一版起也记下远程运行时的 `listen` 与 `instances`（写了才有），并检查运行时名字和连进来的运行时的配置，旧样本的期望不变。

## 0.10.0

第一次在发布 PR 里保存持久化样本（0.8.0–0.9.1 的样本是 #284 事后补的），也是 `rutis.json` 第一次有顶层 `format`：没写 `format` 的旧文件按 1 读，所以 0.10.0 的期望与 0.9.1 相同，只是样本多了 `"format": 1`。[写一个 Python 插件](guide/python-plugin.md) 的示例从这一版起由发布后验证照着走（#259），不再人工走。

## 0.9.0

Bun 运行时 `@arcships/rutis-bun` 和 Go 模块 `go/rutis` 首次发布。npm 上 `@arcships/rutis-bun` 是新包名，`NPM_TOKEN` 要能在 `@arcships` 下发布新包；Go 模块由 release.yml 打 `go/rutis/v0.9.0` tag 发布，Go module proxy 从这个 tag 取。发布前 `rutis-host new --lang bun|go` 生成的项目装不上依赖。这也是第一次走发布门和发布后验证（#254）：linux-arm64、darwin-x64 两个安装格子在打 tag 前还没在 CI 上跑过。

## 0.8.0

内核和 dylib 工具链从这一版起并入列车，只推 `v0.8.0`；`rutis-v*` tag 和 publish-rutis.yml 停用。`rutis-sdk`、`rutis-dylib`、`rutis-dylib-meta`、`rutis-dylib-launcher` 是首次发布到 crates.io。

## 0.7.0

0.7.0 是列车的首次发布，基于内核 0.6.1，只推 `v0.7.0`，不打 `rutis-v*`。除 `rutis-loader`（此前有 0.1.0）外各包在注册表上都是新名字：PyPI 需先为 `rutis`、`rutis-host` 配置 pending publisher。发布成功后撤下 crates.io 和 npm 上的旧 `rutis-interop`。
