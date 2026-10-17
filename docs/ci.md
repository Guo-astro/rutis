# 持续集成（CI）

[English](ci.en.md)

本文说明 CI 现在怎么运行，以及修改 CI 时要做什么。规则依据是[质量标准](quality-standard.md) §12，耗时数据在[质量现状](quality-status.md) §8。

## 1. 规则

1. PR 只运行和这次改动有关的任务；每晚（main 有新提交时）和发布前运行全部任务。合并到 main 不运行检查。
2. PR 推了新提交，旧的那次运行自动取消；每晚的运行和手动运行不取消。
3. 合并只看一个检查：`ci-ok`。
4. macOS 机器少，一次运行尽量只占一个 macOS 任务。
5. 同一项检查只在一个任务里跑。
6. 改 CI 配置的 PR 运行全部任务。
7. Rust 编译缓存只在 main 上的运行（每晚的运行）里保存；PR 读取 main 的缓存，不保存自己的。
8. 时间目标：普通代码 PR 从推送到出结果不超过 10 分钟，排队也算在内。

## 2. 有哪些 workflow

| workflow | 什么时候运行 | 做什么 |
| --- | --- | --- |
| `ci.yml` | 每个 PR；每晚由 `nightly.yml` 调用（`all=true`）；发布门里由 `release.yml` 调用（`all=true`）；手动（任何分支，`all` 默认打开） | 测试和检查（下文） |
| `nightly.yml` | 每天 UTC 19:00，main 有新提交时；手动（`force`：没有新提交也跑） | 调用 `ci.yml`（全部任务）、`install.yml`（五个平台）、`dylib-windows.yml`、`stress.yml`；用最新已发布的 Cordis 插件跑一遍 Cordis 检查，只报告（`cordis-latest`，下文）；调用 `netem.yml`，只报告（`netem`，下文）；失败时开问题单（下文、§7） |
| `dylib-windows.yml` | PR 改了 dylib 相关文件（与 `dylib` 开关相同）；每晚由 `nightly.yml` 调用；发布门里由 `release.yml` 调用（打 `v*` tag 不再单独触发）；手动 | Windows 上的 dylib 测试 |
| `stress.yml` | 每晚由 `nightly.yml` 调用；手动 | 重复运行内核测试和多进程测试；网络长时间测试；大规模运行内核的 shuttle 模型，以及模糊测试（下文） |
| `netem.yml` | 每晚由 `nightly.yml` 调用（只报告）；PR 改了 `tests/e2e/tests/netem.rs` 或 `netem.yml`；手动 | S5 的容器版（风险 E12，#341）：两个 Docker 容器，非 loopback 地址 + TLS，netem 延迟、丢包、断网（下文） |
| `ci-stats.yml` | 每周；手动 | 统计 CI 的耗时和失败 |
| `deps.yml` | 每周；手动 | 依赖漏洞检查：`cargo deny check advisories`（不含示例项目）、`@arcships/rutis-runtime` 的 `npm audit`、Python 包的 `pip-audit`。不在 PR 上跑，不阻止合并；失败时 GitHub 通知维护者 |
| `install.yml` | 每晚由 `nightly.yml` 调用；PR 改了打包、构建配置或安装测试（`package.yml`、`install.yml`、`tests/e2e/**`、各包的 `package.json`、`pyproject.toml`、`node/rutis-host/scripts/**`、`Cargo.toml`、`Cargo.lock` 等）；发布时由 `release.yml` 调用；手动 | 用 `package.yml` 构建这次提交的包并安装运行：npm、PyPI、release 二进制。每晚和发布前五个平台（linux-x64、linux-arm64、darwin-x64、darwin-arm64、win32-x64），PR 上只有 linux-x64，手动运行默认三个平台（linux-x64、darwin-arm64、win32-x64） |
| `release.yml` | 打 `v*` tag；手动（`dry-run` 默认打开：只跑发布门，不发布） | 发布门（调用 `ci.yml`（`all=true`）、`package.yml`、`install.yml`（五个平台）、`dylib-windows.yml`）、发布、发布后验证（[发布](release.md)） |
| `release-cli.yml` | 打 `cli-v*` tag | 发布 rutis-cli |

### 每晚的 `nightly.yml`

合并到 main 不运行任何检查：PR 已经按改动跑过相关的任务。PR 上没测到的两类问题由每晚的运行补上：路径表漏了某个依赖，PR 上该跑的任务没跑（Q12.3.3）；两个 PR 各自通过，合在一起才出问题。

- 时间：每天 UTC 19:00（北京时间凌晨 3 点）。第一个任务 `plan` 比较 main 当前的提交和上一次成功的每晚运行的提交，相同就跳过，后面的任务都不跑。
- 内容：调用 `ci.yml`（`all=true`，全部任务）、`install.yml`（构建这次提交的包，五个平台安装运行；`darwin-arm64` 在自托管 macOS 上，其他格子用 GitHub 的干净机器）、`dylib-windows.yml`、`stress.yml`（默认轮数）、`netem.yml`（任务 `netem`），以及任务 `cordis-latest`。六个同时开始。
- 失败时 `report` 任务开问题单（§7）。
- `cordis-latest`（风险 F4，规范 Q6.14.4）：把 `node/baseline` 的 Cordis 和 dsh 插件换成最新已发布的版本（每个包取 `latest` 与 `next` 两个 dist-tag 中较新的一个，`node/baseline/latest.mjs`；锁文件不改、不提交），再跑用到它们的检查：库级挂载与原生 Cordis 对照（`rutis-bridge` 的 `dsh_baseline`）、生成的绑定（`examples/dsh-baseline`，去掉挂载的版本固定）、S6（`tests/e2e/tests/cordis.rs`）。最新版本之间 peer 依赖冲突、装不到一起时，先报告，再用 `--legacy-peer-deps` 安装，检查照常跑。**只报告**：上游发新版不应让 main 的每晚运行失败，所以这个任务不会失败（`continue-on-error`），`report` 不等它，也不开问题单；作业摘要列出每个包锁定的和最新的版本、每项检查的结果，失败的检查在运行页面上显示为警告。维护者看到警告后判断：是 rutis 的问题就登记 issue 修复，是上游的不兼容就在升级锁文件时处理。
- `netem`（风险 E12，#341）：调用 `netem.yml`，跑 `tests/e2e/tests/netem.rs`：两个 Docker 容器在自己的 bridge 网络上，B 是监听 `wss://0.0.0.0:7443` 的 Python 运行时（证书由测试时生成的 CA 签发），A 是 `rutis-host run`，拨号 B 并在 B 上运行一行；`tc qdisc … netem` 依次加上 200 ms 延迟 + 5% 丢包、双向断网，再恢复，最后 B 换 token 重启。断言与 S5 相同：进行中的调用显式失败且只执行一次、退避上限内重连、拒绝类别可读、两个容器内无残留。用 GitHub 的 `ubuntu-24.04`（要 Docker 和 `NET_ADMIN`；自托管 Linux 是没有 Docker 的容器，§11），release 构建 `rutis-host`，约 2 分钟。**只报告**，做法同 `cordis-latest`：`netem.yml` 的 `report-only` 输入让任务 `continue-on-error`，`report` 不等它、不开问题单；失败时运行页面显示警告，失败场景的目录（每个程序的输出）上传为 artifact `e2e-netem`。手动运行：`gh workflow run netem.yml --ref <分支>`（这时失败就是失败）；本机 Linux 有 Docker 时的命令写在 `netem.rs` 开头。
- 手动运行：Actions 页面选 `nightly`，点 “Run workflow”；没有新提交也要跑时勾选 `force`，或者 `gh workflow run nightly.yml -f force=true`。在别的分支上手动运行时，比较的是那个分支上一次成功的每晚运行。
- 只想跑一次全部 CI 任务（不要安装、dylib-windows、stress）：`gh workflow run ci.yml --ref <分支>`（`all` 默认打开）。

### 每晚的 `stress.yml`

只偶尔失败的测试，在 PR 上碰巧撞到时会挡住和它无关的人。`stress.yml` 每晚把这类测试重复跑很多轮，让偶发失败先在夜里出现。它不在 PR 上跑，不影响 PR 的时间。需要大量迭代或固定时长的检查也放在这里：内核的 shuttle 模型和模糊测试（#179）。五个任务同时运行：

| 任务 | 做什么 |
| --- | --- |
| `rutis` | 内核测试重复 `rounds` 轮（默认 20）。每轮换测试线程数（1、2、全部核数）、打乱顺序、换种子，每五轮用一次 release 构建。第一次失败就停 |
| `multiprocess` | bridge、loader 中会启动进程、打开本地通道、启动运行时进程的测试，也就是 PR 上 macOS、Windows 跑的那些测试文件：除 `ci.yml` 的 `PURE_TESTS_BRIDGE`、`PURE_TESTS_LOADER` 两个列表以外的全部测试文件（§4；新测试文件这里自动跟着跑）。运行时和 `rust` 任务相同（§9）。测试只构建一次，然后重复 `rounds` 轮，每轮换测试线程数、打乱顺序、换种子。某一轮失败也跑完所有轮，最后才让任务失败 |
| `soak` | bridge 的 WebSocket 和本地通道长时间测试，各 `soak_secs` 秒（默认 600） |
| `shuttle` | 内核的 shuttle 模型（`crates/rutis/src/fiber/shuttle_tests.rs`：TransitionTask 唤醒、`post_join` 与驱动退出、并发 dispose 只清理一次），每个模型每种调度器 `shuttle_iterations` 次迭代（默认 1,000,000；PR 上在 `rust` 里跑 1000 次）。失败时打印失败的调度和种子，日志作为 artifact `shuttle-log` 上传，作业摘要里写明怎样重放（`RUTIS_SHUTTLE_REPLAY`） |
| `fuzz` | `fuzz/` 下的三个模糊测试目标（cargo-fuzz，nightly Rust），矩阵里每个目标一项，各 `fuzz_secs` 秒（默认 1200）：`session_frame`（会话帧和其中的带标签值）、`local_lines`（本地通道的行分帧及其长度上限）、`websocket_upgrade`（监听方读取升级请求）。都从仓库里的语料（`fuzz/corpus/<目标>`）开始，运行中新增的语料不保留。崩溃、卡住（单个输入超过 10 秒）或性质检查失败，该项失败：输入和日志作为 artifact `fuzz-<目标>` 上传，作业摘要里有复现命令。跑在 GitHub 的 Linux runner 上（`LINUX_RUNNER`，§11） |

整个 workflow 的时间由最慢的任务决定：`soak` 约 21 分钟；`multiprocess` 估计 30–45 分钟（构建约 10 分钟，每轮约 1–1.5 分钟）；`fuzz` 每项约 25 分钟（安装 cargo-fuzz、构建，再跑 20 分钟）；`shuttle` 几分钟。

- 失败不重试（规范 Q7.3）。
- 每轮在日志里打印线程数和种子。作业摘要里有每一轮的结果表、失败的测试名和它失败的轮次，以及重放命令：用同样的种子、线程数和顺序重跑那个测试文件。
- `multiprocess` 有失败时，每一轮每个测试文件的输出都作为 artifact `multiprocess-logs` 上传，保留 14 天。
- 手动运行：Actions 页面选 `stress`，点 “Run workflow”，可以改 `rounds`、`soak_secs`、`shuttle_iterations` 和 `fuzz_secs`；或者 `gh workflow run stress.yml -f rounds=50 -f fuzz_secs=120`。五个任务都会运行。在本机运行 shuttle 模型和模糊测试目标的方法见 [CONTRIBUTING](../CONTRIBUTING.zh-CN.md)。
- 夜里出现的失败按 §7 处理：登记 issue，再想办法稳定复现。

## 3. PR 上怎么决定跑哪些任务

`ci.yml` 的第一个任务 `changes` 看这个 PR 改了哪些文件，输出下面几个开关。后面的任务根据开关决定跑不跑。完整运行（输入 `all` 为 true：每晚、手动运行）时所有开关都打开；不是 PR、`all` 又为 false 时所有开关都关闭，只跑 `changes` 和 `ci-ok`。

| 开关 | 改了这些文件时打开 |
| --- | --- |
| `all` | `Cargo.lock`、根目录的 `Cargo.toml`、`rust-toolchain.toml`、`.github/workflows/**`。打开时其他开关也全部打开 |
| `code` | 除 `docs/**` 和 `*.md` 以外的任何文件 |
| `dylib` | dylib 相关的 crate：`rutis-dylib*`、`rutis-sdk`、`rutis-dev`、`rutis-xtask`，以及链接 SDK、放着 dylib 宿主测试的 `rutis-cli`；`tools/*dylib*`、`tools/build-dylib-bundle.sh`、`tools/test-sdk-bundle.sh`、`tools/lib/**`、`tests/dylib-fixtures/**`。**内核 `crates/rutis` 不在内**：内核改动也会影响 SDK，但 dylib 任务在过去 88 次运行里没有失败过，内核 PR 不再为它等 20 分钟，由每晚的运行检查 |
| `packaging` | `scripts/train.mjs`、各个 `pyproject.toml` 和 `package.json`、`crates/*/Cargo.toml`、`node/rutis-host/scripts/**` |
| `docs` | 任何 `.md` 文件 |
| `bun` | `bun/**`、`node/rutis/**`、`crates/rutis-bridge`、`crates/rutis-loader`、`crates/rutis-host` |
| `go` | `go/**`、`crates/rutis-bridge`、`crates/rutis-loader`、`crates/rutis-host` |
| `repro` | `crates/rutis-sdk`、`crates/rutis-cli/build.rs`、`tools/test-dylib-repro.sh`、`tools/build-dylib-bundle.sh`、`tools/lib/**`。内核改动也会影响 SDK，但它的可复现检查放在完整运行里 |
| `specs` | `specs/**`（TLA+ 模型、配置、预期结果，以及记录结果的 README） |

## 4. 每个任务

“PR 上”一列是这个任务在 PR 上什么时候运行、运行哪些内容；完整运行中（每晚、手动运行，以及打开 `all` 的 PR 上）每个任务都运行，内容是全量的。

| 任务 | 机器 | PR 上 | 完整运行多做的 | 内容 |
| --- | --- | --- | --- | --- |
| `links` | macOS（自托管，§10） | `docs` | — | 检查 Markdown 里的相对链接 |
| `rust` | Linux | `code` | Linux 上 loopback 方式启动运行时的重跑 | 全部 Rust 测试，每项只跑一次：工作区（除 bridge、loader、`rutis-e2e`）、loader 全部行类型、bridge 全部 feature；内核的 shuttle 模型，1000 次迭代（用 `--cfg rutis_shuttle` 在 `target/shuttle` 里构建，远不到一分钟；`stress.yml` 大规模运行）。用最低支持版本（§9） |
| `e2e` | Linux | `code` | 跨版本相遇：`cargo test -p rutis-e2e --test cross_version -- --ignored`（#209），上一个发布版本的运行时包、Go SDK、`rutis-host` 与这次提交相遇；从 npm、PyPI、Go 模块代理下载，装在 `runner.temp`（不进 Rust 缓存），约 1–2 分钟。结果随新版本发布而变（§6 第 5 条），不在 PR 上跑；它只随本项目自己的发布而变，又要在发布门里跑，所以放完整运行（每晚、发布门），不放每周。放进已有的 `e2e`（Node、Python、Bun、Go 都已装好），不新开任务（§6「加一个任务」第 4 条）；完整运行中 `e2e` 不在关键路径上 | 端到端场景 `cargo test -p rutis-e2e`（`tests/e2e`：真实的 rutis-host 进程、项目目录、各语言的探针插件），与 `rust` 并行，用最低支持版本（§9）。失败时上传 `RUTIS_E2E_DIR`（失败场景的目录和日志）为 artifact `e2e-linux`。安装场景不在这里跑，由 `install.yml` 跑。S7（`embedding.rs`）反复创建关闭租户的次数由 `RUTIS_E2E_TENANT_CYCLES` 决定：PR 上 20 次，完整运行（每晚）3000 次，多约一分钟；其他任务用默认的 20 次。同样的场景在 macOS 上由 `network-macos` 跑，在 Windows 上由 `runtimes-windows` 跑（只跑不限于 Unix 的场景） |
| `js-py` | macOS（自托管，§10） | `code` | — | `scripts/train.mjs`；Node 包（`node/rutis`、`rutis-runtime`、`rutis-host`）和 rutis-dsh 的 dsh 测试；Python 单元测试 |
| `lint` | Linux | `code` | — | `cargo fmt --check`；`PURE_TESTS_*` 里的每个名字都有对应的测试文件（§4）；`cargo clippy --workspace --all-targets --all-features -- -D warnings`。与测试并行。没有基线：警告在引入它的 PR 里改掉，升级工具链带来的新 lint 在升级的 PR 里改掉 |
| `msrv` | macOS（自托管，§10） | 只在 `all` 时（改了 `Cargo.lock`、`Cargo.toml` 等） | 整个任务 | 用声明的 MSRV（`Cargo.toml` 的 `rust-version`）和仓库的 `Cargo.lock` 编译库和二进制（`cargo +<版本> check --workspace`，不含示例项目 `rutis-agent`、`rutis-cli`） |
| `checks` | macOS（自托管，§10） | 只在 `all` 时 | 整个任务 | bridge、loader 各个 feature 组合的编译检查和 `--no-run`（整个工作区的编译由 `lint` 的 clippy 覆盖）。这些检查发现的是少见的 feature 组合编译错误，不跑测试 |
| `network-macos` | macOS | `code`：只跑和平台有关的测试 | bridge、loader 全部测试；`node/rutis` 测试 | 最新版本（§9）下的进程启动、本地通道、运行时进程、WebSocket、rutis-host 测试；端到端场景（失败时上传 artifact `e2e-macos`）；Node 运行时和 Python 包的测试；整个工作区的编译检查 |
| `runtimes-windows` | Windows | `code`：只跑和平台有关的测试 | bridge、loader 全部测试 | 同上，在 Windows 上（进程用 loopback 方式启动）；Go SDK 测试；E2E harness 和不限于 Unix 的 E2E 场景（`rutis-e2e`：Job Object、Ctrl-Break、残余检查）；整个工作区的编译检查 |
| `runtimes-bun` | Linux | `bun`：Bun 1.4.0 | Linux 最新版；macOS 1.4.0 | Bun 运行时的全部测试，包括 loopback 方式启动的重跑 |
| `runtimes-go` | Linux | `go`：Go oldstable | Linux stable；macOS oldstable；Linux 上 loopback 方式启动的重跑 | Go SDK 与 Go 运行时的全部测试 |
| `semver-rutis` | macOS（自托管，§10） | `packaging` | 整个任务 | 公开 API 和上次发布相比有没有不兼容的改动（只提示） |
| `dylib-linux-launcher`、`dylib-linux-sdk-bundle` | Linux | `dylib` | — | dylib 测试，分两个任务同时跑 |
| `dylib-linux-repro` | Linux | `repro` | 整个任务 | SDK 可复现构建（在一台机器上用两个目录各构建一次） |
| `dylib-macos` | macOS（自托管，§10） | `dylib` | 可复现构建一步 | 同样的 dylib 测试，在一个任务里依次跑，共用一次构建的 bundle；可复现构建这一步只在 `repro` 打开时跑 |
| `release-dry-run` | macOS（自托管，§10） | `packaging` | 整个任务 | 发布要用的包能构建（crate、npm 包、Python 包）。各平台的二进制和 wheel 由 `install.yml` 构建并安装（§2）：PR 上只有 Linux x64，其他平台的打包在每晚和发布前检查 |
| `sdk-repro`、`sdk-repro-macos` 及比对 | Linux、macOS（自托管，§10） | 只在 `all` 时 | 整个任务 | 在两台机器上分别构建 SDK，比较结果是否相同（比较在自托管的 macOS 上） |
| `specs` | Linux | `specs` | — | TLA+ 设计模型（`specs/`，质量标准 §11.2）：每个模型一项（矩阵），装 Java 21 和固定版本、校验过 sha256 的 `tla2tools.jar`（v1.7.4），运行模型目录下的 `run.sh`，每个配置的结果（通过、死锁、某个不变式或活性被违反）与 `expected.txt` 比较。失败时上传 TLC 日志（含反例）为 artifact `tlc-<模型>`。`cross-runtime-sync` 约 1 分钟，`remote-lease` 约 2–3 分钟（16 核；GitHub 的 4 核机器上估计 5–8 分钟） |
| `ci-ok` | macOS（自托管，§10） | 总是 | — | 汇总（§5） |

### 和平台有关的测试

PR 上 macOS、Windows 默认跑 bridge、loader 的全部测试文件和两个 crate 的单元测试（`--lib`），只跳过 `ci.yml` 顶部 `PURE_TESTS_BRIDGE`、`PURE_TESTS_LOADER` 列出的测试文件；rutis-host 的测试全跑。列表里的测试只依赖 Rust 代码本身：内存通道上的协议、行的配置和生命周期等，不启动进程、不开 socket、除了读 fixture 不碰文件、不依赖操作系统的行为。它们在各平台上结果相同，PR 上只在 Linux 的 `rust` 任务里跑，完整运行中三个平台都跑。跳过它们是为了让 macOS、Windows 的时间有上限。

传给 `cargo test` 的 `--lib --test <名字> ...` 由 `tools/platform-tests.sh` 在任务运行时生成：`crates/<crate>/tests/` 下的每个测试文件，去掉列表里的名字。所以新加的测试文件默认在三个平台上都跑，不需要记得登记；只有确定不启动进程、不打开通道或文件的测试才加进 `PURE_TESTS_*`。列表里的名字找不到对应文件时，脚本报错，`lint` 任务失败，重命名或删除测试文件不会让列表悄悄失效。每晚 `stress.yml` 的 `multiprocess` 任务用同一个脚本重复运行同一组测试（§2）。

### 一致性矩阵

bridge 的一致性套件在哪些实现、哪些通道、哪些平台上运行（风险 C10，#176）。手写维护：增删格子、改跳过原因时同步改这张表。“跳过”的格子在测试里用 `ignore = "原因"` 标注，`cargo test` 的输出里能看到；整平台不编译的，原因写在 `cfg` 旁的注释里。

会话契约（`session::testing::session` 与畸形帧检查 `malformed`，`tests/runtime_conformance.rs`）：

| 实现 | 通道 | Linux | macOS | Windows |
| --- | --- | --- | --- | --- |
| Rust | memory | 运行 | 运行 | 运行 |
| Node、Python | Unix socket | 运行 | 运行 | 跳过：Windows 没有 Unix socket，运行时用 loopback 连接 |
| Node、Python | loopback TCP（带一次性令牌） | 运行 | 运行 | 运行 |
| Go | Unix socket | 运行 | 运行 | 跳过：同上 |
| Go | loopback TCP | 运行 | 运行 | 运行 |
| Node、Python | WebSocket（夹具监听，Rust 拨入） | 运行 | 运行 | 运行 |
| Go | WebSocket | 不跑：Go SDK 的 WebSocket 监听在运行时内部（`package rutis`），会话夹具是单独的程序用不到；Go 运行时的 WebSocket 列在运行时契约里 | 同 Linux | 同 Linux |
| Bun | Unix socket、loopback TCP | 运行（畸形帧检查只在 Unix socket 上） | 运行 | 不编译：Bun 在 Windows 上不是声明支持的平台 |
| Bun | WebSocket | 不跑：Bun 运行时的 WebSocket 监听在它的监督进程里（`src/remote.ts`），会话夹具是单独的程序用不到；Bun 运行时的 WebSocket 列在运行时契约里 | 同 Linux | 同 Linux |

运行时契约（`runtime::testing::runtime`，`tests/session_matrix.rs`）：

| 实现 | 通道 | Linux | macOS | Windows |
| --- | --- | --- | --- | --- |
| Node、Python、Go | 继承的 socket（`fd:3`） | 运行 | 运行 | 跳过：Windows 不交接描述符，只有 loopback |
| Node、Python、Go | 回拨 socket 路径 | 运行 | 运行 | 跳过：同上 |
| Node、Python、Go | loopback TCP（本地传输的 `Handover::Loopback`，与 `LocalRuntime` 启动运行时的方式相同，再 `Process::attach`） | 运行 | 运行 | 运行 |
| Node、Python、Go | WebSocket（运行时监听，Rust 拨入） | 运行 | 运行 | 运行 |
| Bun | `fd:3`、回拨 socket 路径 | 运行；`runtimes-bun` 用 `RUTIS_LOCAL_HANDOVER=loopback` 重跑 | 运行 | 不编译：同上 |
| Bun | loopback TCP | 不跑单独的列：由上一行的重跑覆盖 | 同 Linux | 不编译：同上 |
| Bun | WebSocket（运行时监听，Rust 拨入） | 运行 | 运行 | 不编译：同上 |
| 不声明 `fd` 的 Node 包回退到回拨 | — | 运行 | 运行 | 跳过：Windows 只有 loopback |

前两列和最后一行经 `Process` 门面启动进程，契约还检查崩溃后的退出状态；loopback、WebSocket 两列接到通道上，只检查崩溃结束会话。Windows 上 `Process` 门面的 loopback 启动由 `python_runtime.rs`、`go_runtime.rs` 等覆盖。

节点契约（`rutis_bridge::testing::node`）：

| 实现 | 通道 | Linux | macOS | Windows |
| --- | --- | --- | --- | --- |
| Rust 节点 | memory（`tests/node_conformance.rs`） | 运行 | 完整运行中运行 | 完整运行中运行 |
| Rust 节点 | WebSocket（`tests/websocket_conformance.rs`，两个 rutis 根经 loopback WebSocket 链接） | 运行 | 运行 | 运行 |
| Cordis 节点 | WebSocket（`tests/websocket_conformance.rs`） | 运行 | 运行 | 运行 |

拨出契约（`tests/dial_conformance.rs`，rutis 起接入点，运行时以 `dial:ws://…` 连进来）：

| 实现 | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Python | 运行 | 运行 | 运行；主动停止一项跳过：它发 SIGTERM，Windows 没有 |
| Go | 运行 | 运行 | 运行；主动停止一项跳过：它发 SIGTERM，Windows 没有 |
| Node | 运行 | 运行 | 运行；主动停止一项跳过：同上 |
| Bun | 运行 | 运行 | 不编译：Bun 在 Windows 上不是声明支持的平台 |

只在 Unix 上编译的其他 bridge 测试文件（原因写在各文件的 `cfg` 旁）：`bun_runtime.rs`、`bun_channel.rs`（Bun）；`local_spawn.rs`（`fd:3`、回拨两种交接，Windows 的 loopback 交接由 `local_loopback.rs` 覆盖）；`local_plugin.rs`、`value_roundtrip.rs`（Unix socket）；`local_stdio.rs`（`dup2`）；`concurrent_launch.rs`（跨 fork/exec 继承的描述符）；两个浸泡测试只在 Linux（读 `/proc`）。

### 每种改动在 PR 上跑什么

| 改动 | 运行的任务 | 预计时间（推送到结果） |
| --- | --- | --- |
| 只有文档 | `changes`、`links` | 约 1 分钟 |
| 内核 `crates/rutis` | `rust`、`e2e`、`js-py`、`lint`、`network-macos`、`runtimes-windows` | 约 5–8 分钟 |
| bridge、loader、host | 再加 `runtimes-bun`、`runtimes-go`（Linux 各一个版本） | 约 6–9 分钟 |
| 只有 Python 包 | `rust`、`e2e`、`js-py`、`lint`、`network-macos`、`runtimes-windows` | 约 5–8 分钟 |
| 改了 `package.json`、`pyproject.toml`、`crates/*/Cargo.toml` | 再加 `release-dry-run` 和 `semver-rutis`；改了包的 `package.json`、`pyproject.toml` 时还有 `install.yml`（Linux x64） | 不变，打包任务与测试并行 |
| dylib | 再加 dylib 任务；`dylib-windows.yml` | 约 20–25 分钟（dylib 的预算） |
| CI 配置、`Cargo.lock`、根 `Cargo.toml` | 全部，同完整运行 | 约 25 分钟 |

## 5. 合并条件

- `ci-ok` 是 main 分支唯一要求通过的检查。
- `ci-ok` 只看列在它 `needs` 里的任务：这些任务成功或被跳过，它就通过；有任务失败或被取消，它就失败。
- 没有列进 `ci-ok` 的任务，失败了也不会阻止合并。

## 6. 修改 CI 时要做的事

### 新增 CI 检查的流程

CI 慢下来，通常是一项一项加检查加出来的。新增检查（新任务、已有任务里的新步骤、新的版本或平台）前按下面的顺序回答：

1. **它防的是哪类问题，放在哪一级。** 级别是 PR、每晚（`nightly.yml`：`ci.yml` 的完整运行、`install.yml`、`dylib-windows.yml`、`stress.yml`）、每周。默认放每晚；只有必须在合并前拦住的问题才放 PR。放每晚的检查失败了，按 §7 优先处理，同样会被发现，只是晚一点。
2. **放 PR 的，写明它占多少时间。** 它不能让关键路径（最慢的那个任务）变长：放进一个并行的 Linux 任务，或者在已有任务里替换掉别的内容。不新增 macOS、Windows 任务；macOS、Windows 上只跑和平台有关的测试（§4）。
3. **能替换就不新增。** 例如支持矩阵的最低版本：把 Linux 任务里的版本换成最低版本，macOS 保持最新版，而不是再加一个任务跑最低版本。
4. **目标：普通代码 PR 从推送到出结果不超过 10 分钟，排队也算在内。** 用 `node tools/ci-stats.mjs [次数] [workflow] [--since 日期]` 测量（每周的 `ci-stats.yml` 也会跑），看"ordinary code"一行：第一次运行、跑了 `rust`、没跑 `checks` 和 `dylib-` 任务的 PR。
5. **结果会因外部数据变化的检查不放 PR。** 漏洞库、新版本发布会让代码没变的 PR 失败；这类检查放每周（`deps.yml`）。超出时，从关键路径上的任务开始调整：拆成并行任务，或者把成本高、很少失败的部分移到每晚。

把这几条的答案写进 PR 描述。

### 加一个任务

1. 先按上面的流程决定它放在哪一级。
2. 给它加 `needs: changes` 和 `if: needs.changes.outputs.<开关> == 'true'`（只在完整运行中跑的用 `all`）。没有合适的开关就在 `changes` 里加一个，并写进 §3。
3. 把它加进 `ci-ok` 的 `needs`。
4. 看已有任务里是不是已经在做同样的检查；能放进已有任务的，不新开任务。
5. 要用 macOS 时，先看能不能放进 `network-macos`。普通改动最多占 1 个 macOS 任务，改 dylib 时最多 2 个。
6. 写进 §4。

### 加一种语言运行时

1. `changes` 加一个这种语言的开关，包括它的运行时目录，以及 `crates/rutis-bridge`、`crates/rutis-loader`、`crates/rutis-host`。
2. 在 `rust`、`e2e`（最低支持版本）和 `network-macos`（最新版本）里安装这种语言（§9），在已有的 bridge、loader、host 测试步骤里加上它的 feature；它的测试文件默认在 macOS、Windows 上跑（§4），不需要登记。
3. 如果要测多个版本，加一个 `runtimes-<语言>` 任务：按第 1 步的开关运行，加进 `ci-ok`，PR 上只在 Linux 跑最低支持版本；其他版本和 macOS 只在完整运行中跑（做法见 `runtimes-bun` 和 `changes` 的 `bun-matrix`）。
4. 发布相关：`release-dry-run`、`scripts/train.mjs`、`release.yml` 都加上它的包。
5. 更新 §3、§4。

### 改开关对应的文件

- 改 `changes` 等于改了 `.github/workflows/**`，这个 PR 会运行全部任务。
- 如果某个任务在 PR 上被跳过，之后在每晚的运行中失败了，说明开关漏了某些文件：修好失败后，把漏掉的文件加进开关。

### 改 CI 配置

- 单独开 PR，提交类型用 `ci:`。
- 改 CI 配置的 PR 运行全部任务，所以只在 PR 上跑的那部分（例如 macOS、Windows 上的测试子集）在这个 PR 里不会跑到；合并后看之后第一个普通 PR 的运行。
- 合并后手动运行一次 `nightly.yml`（`gh workflow run nightly.yml -f force=true`），或者看第二天每晚运行的结果。

## 7. 失败怎么处理

- 每晚的运行失败时，`nightly.yml` 的 `report` 任务开一个问题单：标签 `ci`，指派给维护者，标题 `nightly: <日期> 失败`，写明失败的任务和提交范围（上一次成功的每晚运行的提交到这次的提交，附比较链接）。已经有未关闭的 `nightly:` 问题单时，在它下面追加评论，不另开。
- 每晚失败优先处理：在提交范围里找出是哪次合并引起的，修好之前不再合并可能受影响的改动。修好后关闭问题单。
- 每晚的运行比较的是上一次**成功**的运行：失败之后即使没有新提交，下一晚也会再跑一次同一段提交。想早点确认修复，手动运行 `nightly.yml`（`force`）。被手动取消的每晚运行也不算成功。
- 合并到 main 不运行检查。PR 上新的推送取消正在跑的运行。每晚的运行同一时间只有一个，不取消；手动运行和 `nightly.yml`、`release.yml` 调用的运行各自是独立的并发组，不会互相取消或替换。
- 偶尔失败的测试：先想办法稳定复现，再修。不用自动重试来掩盖。

## 8. 编译缓存

- 所有任务用 `Swatinem/rust-cache`，设置 `save-if: github.ref == 'refs/heads/main'`：只有 main 上的运行（每晚的运行，或在 main 上手动运行）保存缓存。
- PR 只能读取 main 和它自己的缓存。PR 不保存，是为了不占用仓库 10 GB 的缓存空间，把 main 的缓存挤掉。
- 自托管的 macOS runner 不用 `rust-cache`，保留自己的 target 目录（§10）。
- 加任务时照此设置（`ci.yml` 里 `rust` 任务的 `rust-cache` 处有说明）。
- 不能用缓存的检查（SDK 可复现构建）放在完整运行里，PR 只在 `repro` 打开时运行。

## 9. 版本

规则：每种语言，Linux 任务用最低支持版本，macOS 和 Windows 任务用最新版本。最低版本在每个 PR 上都验证，最新版本在 macOS、Windows 上验证，不为版本单独加任务。

| | 声明在哪 | Linux（`rust`、`e2e`、`js-py`、`checks`、`lint`、`runtimes-go`、`runtimes-bun`、`stress.yml` 的 `multiprocess`） | `network-macos`、`runtimes-windows` |
| --- | --- | --- | --- |
| Node | 各 npm 包 `package.json` 的 `engines.node` | 声明的最低主版本 | 最新的偶数主版本，手动升级 |
| Python | `python/rutis/pyproject.toml` 的 `requires-python` | 声明的最低版本 | `3.x`，自动跟随；新版本让 PR 失败时，临时写死上一版并另开 PR 修 |
| websockets | `python/rutis/pyproject.toml` 的 `network` extra | `==<下限>.*` | `>=<下限>` |
| Bun | `bun/rutis-bun/package.json` 的 `engines.bun` | 声明的最低版本 | `latest` |
| Go | `go/rutis/go.mod` | `oldstable` | `stable` |
| Rust 工具链 | `rust-toolchain.toml` | 所有 workflow 的 `dtolnay/rust-toolchain@<版本>` 与它相同 | 同左 |
| MSRV | 根 `Cargo.toml` 的 `rust-version` | `msrv` 任务（完整运行） | — |

端到端场景（`rutis-e2e`）三个平台都跑：Linux 在 `e2e`（最低版本），macOS 在 `network-macos`、Windows 在 `runtimes-windows`（最新版本；Windows 上只跑不限于 Unix 的场景）。

`runtimes-go`、`runtimes-bun` 在完整运行中的 macOS 矩阵项里，Node、Python 只是跨语言行的对端，和 Linux 一样用最低版本。发布用的 Node（`release.yml`、`package.yml` 和 `ci.yml` 的 `release-dry-run`）是发布工具的版本，不属于支持矩阵。

**改了声明，要在同一个 PR 里改 CI 的版本**：改 `engines`、`requires-python`、websockets 下限或 `rust-version` 的 PR，同时改上表里对应任务的版本；否则声明的最低版本没有经过验证（Q10.3）。

**包的依赖**：三个 npm 包各有自己的 lockfile，任务在用到它之前安装：启动 Node 运行时的任务先 `npm --prefix node/rutis-runtime ci`；加载 SDK 测试工具（`node/rutis/src/testing.mjs`，它引入 Cordis）的任务——SDK 自己的测试、`bun/rutis-bun/test/sdk.test.ts`、端到端场景里 Node 模板的测试——还要 `npm --prefix node/rutis ci`；启动 Bun 运行时的任务（它引入 Cordis；`rutis-bridge/tests/bun_runtime.rs` 检查 `node_modules` 存在）在 `bun/rutis-bun` 里 `bun install --frozen-lockfile`。本地同样（[CONTRIBUTING](../CONTRIBUTING.zh-CN.md)）。

## 10. 自托管的 macOS runner

`ci.yml` 的 `dylib-macos`、`sdk-repro-macos`，以及 `install.yml` 在 PR 和每晚的运行中的 `darwin-arm64`，跑在自托管的 macOS runner 上：一台专用、隔离的 Mac，可以装多个 runner 实例，标签都是 `self-hosted, macOS, ARM64`（第一个实例叫 `macos-1`）。结果和操作系统无关的任务也在那里跑，免得排队等 GitHub 的 Linux runner：`ci.yml` 的 `links`、`js-py`（Node 22、Python 3.10、websockets 15，Python 由 uv 装）、`msrv`、`checks`、`release-dry-run`、`semver-rutis`、两个 SDK 比对，`runtimes-go`/`runtimes-bun` 在完整运行时的 macOS 矩阵项（Go oldstable、Bun 1.4.0，Node 22，Python 由 uv 装），以及只做调度的 `changes`、`ci-ok`，`install.yml`、`package.yml` 的 `plan`（发布时，即 tag 上，`plan` 仍用 `ubuntu-24.04`）。只有一个实例时它们会排在正在跑的 macOS 任务后面，所以要多加实例（现在有 `macos-1` 到 `macos-4`）。`msrv`、`checks` 在 macOS 上编译，不编译只在 Linux 上的 cfg 代码；这部分由 Linux 上的 `lint`（clippy，全部 feature）编译。留在 Linux 上的：`rust`、`e2e`、`lint`、`runtimes-go`/`runtimes-bun` 的 ubuntu 矩阵项、`dylib-linux-*`、`sdk-repro`。`network-macos` 也在自托管的机器上跑（仓库变量 `NETWORK_MACOS_RUNNER` 为 `["self-hosted","macOS","ARM64"]`；删掉变量回到 GitHub 的 `macos-15`。曾因 #287 留在 GitHub 上，#344 修复后切过来）。其余任务仍用 GitHub 的 runner：`darwin-x64`（要 Intel 的 `macos-15-intel`）、`package.yml` 的 macOS 构建（发布用的文件）、`release.yml` 调用 `install.yml` 时和手动运行 `install.yml` 时的 `darwin-arm64`（发布前的检查用 `macos-15`）；Linux 和 Windows 的任务见 §11。

**切回 GitHub 的 runner，不用改代码**：仓库变量 `MACOS_RUNNER`（Settings → Secrets and variables → Actions → Variables）是 `runs-on` 的 JSON 值，没设置时为 `["self-hosted","macOS","ARM64"]`。设成 `"macos-15"`（带引号）就全部回到 GitHub 的 macOS；删掉变量就回到自托管。`gh variable set MACOS_RUNNER --body '"macos-15"'`。调度任务用另一个变量 `GATE_RUNNER`，切回时设成 `"ubuntu-24.04"`。

自托管的机器在任务之间保留状态，几个实例同时跑。这些任务开头和结尾（`if: always()`）运行 `tools/ci-self-hosted.sh`：

- 结束这个实例之前的任务留下的进程：命令行或工作目录在这个实例的工作区、target 目录或场景目录下的进程，不碰 runner 本身和别的实例的任务；
- 场景目录 `RUTIS_E2E_DIR` 是每个实例自己的 `/tmp/rutis-e2e-<runner 名>`（不用 `runner.temp`：Unix socket 路径在 macOS 上最长 104 字节，见 `tests/e2e/src/lib.rs`），开头和结尾清空；
- 编译不用 `rust-cache`（它会把缓存再上传到 GitHub），`CARGO_TARGET_DIR` 是每个实例保留的 `~/.cache/rutis-ci/<runner 名>/target`，超过 30 GB（`RUTIS_CI_TARGET_MAX_GB`）时清空；编译 dylib SDK 的任务（`dylib-macos`、`dylib-linux-*`、`dylib-windows`）例外，每次用任务自己的临时编译目录（`RUTIS_CI_FRESH_TARGET=1`）：dylib 的文件名不带哈希，不同特性、不同参数编出的 `rutis-sdk` 写同一个文件，保留的目录里出现过 cargo 认为不用重编、实际链接了另一个 `serde_json` 的 SDK（E0308，#376）；`release-dry-run` 打包前先 `cargo clean -p` 列车里的 crate（cargo 只按版本号判断 registry 来源的包要不要重编，保留的编译目录会把同版本的旧构建当成新的，见该步骤的注释）；机器上的 `cargo` 是 mbx（构建缓存）的 shim 时，不设 `CARGO_TARGET_DIR`，由 mbx 管理编译目录，并删掉这个目录（两者混用时，同一个编译目录里出现过依赖两个不同 `serde_json` 的 `rutis-sdk` 和 `rutis-cli`）；`setup-node`、`setup-go` 的缓存也只在 GitHub 的 runner 上用；
- Python 不用 `setup-python`（它的 macOS 版本只能装在 `/Users/runner` 下），用 uv 装任务要的版本，每个任务在 `runner.temp` 里建一个带 pip 的虚拟环境放进 `PATH`；
- `setup-bun` 把 Bun 装进 `~/.bun/bin`，同一台机器上的实例共用这个目录，而 `network-macos` 和 `runtimes-bun` 在完整运行里同时装不同的版本：`runtimes-bun` 在自托管 macOS 上给 `setup-bun` 一个 `runner.temp` 下的 `HOME`，不用缓存；
- `tools/*.sh` 留在 `/tmp/rutis-*` 的目录超过 12 小时后删除；
- 几个实例同时编译会把机器压满（10 核上 4 个实例各开 10 个 rustc，负载到过 79）：每个任务的 `CARGO_BUILD_JOBS` 和 `RUST_TEST_THREADS` 设成 CPU 数除以机器上的实例数（`~/actions-runner*/` 里有 `.runner` 的目录数），至少 2，开头会打印选的值和当时的负载。实例不装在 `~/actions-runner*` 下，或要别的值时，设仓库变量 `RUTIS_CI_JOBS`。`sdk-repro-macos` 也照此限制，不影响构建结果。
- Go 的编译缓存 `GOCACHE` 是每个实例自己的（`~/.cache/rutis-ci/<runner 名>/go-build`，Windows 上在 `_work/_rutis-ci/go-build`）；任务失败后，结尾一步删掉这个实例的 Go 缓存：被中途取消的任务可能留下写了一半的缓存文件，之后读到它的构建都会失败（"malformed elf file"，#374）。下一次任务重新编译 Go 的标准库，大约多一分钟。

**加一个实例前，机器上要有**：

1. runner 用 `./config.sh --labels self-hosted,macOS,ARM64 --name macos-<n>` 注册（名字在机器上唯一，目录和缓存按名字分开），`./svc.sh install && ./svc.sh start` 作为服务运行。
2. 文件描述符上限 10240：在 `~/Library/LaunchAgents/actions.runner.*.plist` 里加 `SoftResourceLimits` 和 `HardResourceLimits`，各含 `NumberOfFiles` = `10240`，然后 `./svc.sh stop && ./svc.sh start`。
3. rustup（`dtolnay/rust-toolchain` 用它装 `rust-toolchain.toml` 的版本）：`curl https://sh.rustup.rs -sSf | sh -s -- -y --default-toolchain none`。
4. Xcode 命令行工具（`xcode-select --install`）：`cc`、`codesign`、`otool`、`lipo`。`sdk-repro-macos` 打印 Xcode 版本，只有命令行工具时打印它们的版本。
5. 磁盘至少 100 GB 空闲（每个实例的 target 目录最多 30 GB，加上 `_work/_tool` 里的 Node、Python、Go、Bun）。
6. 不睡眠：`sudo pmset -a sleep 0 disksleep 0`（或在系统设置里关掉）。
7. 不需要 Homebrew：这些任务只用系统自带的 `bash`（3.2）、`perl`、`tar`、`shasum`、`lsof`、`jq`（macOS 15 起系统自带 `/usr/bin/jq`，`ci-ok` 要用），Node、Go、Bun、uv 由各自的 setup action 装进 `_work/_tool`，Python 由 uv 装进 `~/.local/share/uv`。

## 11. 自托管的 Linux runner

`ci.yml` 的 Linux 任务（`rust`、`e2e`、`lint`、`runtimes-go`/`runtimes-bun` 的 Linux 矩阵项、`dylib-linux-*`、`sdk-repro`）由仓库变量 `LINUX_RUNNER`（`runs-on` 的 JSON 值）决定跑在哪里：没设置时是 GitHub 的 `"ubuntu-24.04"`，设成 `["self-hosted","Linux"]` 就跑在组织的自托管 Linux runner 上；删掉变量就回到 GitHub。`package.yml`（要 Docker 容器）、`install.yml`、`stress.yml` 等其他工作流不受影响，只有 `stress.yml` 的 `shuttle`、`fuzz` 两个任务也按这个变量。

这些 runner 是同一台机器（32 核、60 GB 内存）上的 4 个 Docker 容器，每个容器一个 runner，用户 `runner`，有免密 sudo，自带 Rust、Node、Go、Python，没有 `lsof`、Docker 和 Bun（Bun 由 `setup-bun` 装）。任务开头和结尾同样运行 `tools/ci-self-hosted.sh`（见上一节），区别是：

- 容器里只有一个 runner，`/tmp` 是容器自己的，场景目录用最短的 `/tmp/rutis-e2e`；
- 进程的工作目录从 `/proc` 读（没有 `lsof`）；
- 容器看得到整台机器的 CPU，看不到别的容器：每个任务的并行数取仓库变量 `LINUX_CI_JOBS`（4 个容器时设 `8`），没设置时是全部 CPU 数；
- `setup-python` 在自托管的 Linux 上可以用，Python 照旧由它装。

`runtimes-windows` 和 `dylib-windows.yml` 同样由仓库变量 `WINDOWS_RUNNER` 决定：没设置时是 `"windows-2025"`，设成 `["self-hosted","Windows"]` 就跑在组织的自托管 Windows runner 上（一台 16 核的机器上 4 个实例，以 `NETWORK SERVICE` 运行；Rust 在 `C:\Rust`，有 VS Build Tools、Git、Node、Go、Python）。步骤用 Git 的 bash：机器的系统 `PATH` 里要有 `C:\Program Files\Git\bin`（改了以后重启 runner 服务）。自托管时，每个实例的编译目录和场景目录在 `_work/_rutis-ci/` 下（`checkout` 会清掉检出目录里的 `target`）；Python 不用 `setup-python`（它的 Windows 安装程序要管理员权限），用机器上的 Python，在 `runner.temp` 里建虚拟环境；并行数是 CPU 数除以 4，仓库变量 `WINDOWS_CI_JOBS` 可以改。`dylib-windows` 和 `runtimes-windows` 用同一个实例的编译目录；它的脚本把临时文件放在 `mktemp -d /tmp/rutis-*` 里，用完不删（可复现检查在里面放两份源码和两份 release 构建）；Git 的 bash 把 `/tmp` 映射到服务账户的临时目录，所有实例共用，设 `TMP`、`TEMP` 也不会改变它。所以和自托管的 Mac 一样，任务开头删掉其中超过 12 小时的 `rutis-*` 目录。`tools/test-dylib-launcher.sh` 的 Job Object 检查按进程名在整台机器上找宿主进程，所以宿主副本每次用不同的名字，别的实例同时跑这个检查不会互相干扰。机器要开长路径：注册表 `LongPathsEnabled` = 1，`git config --system core.longpaths true`。
