# 持续集成（CI）

[English](ci.en.md)

本文说明 CI 现在怎么运行，以及修改 CI 时要做什么。规则依据是[质量标准](quality-standard.md) §12，耗时数据在[质量现状](quality-status.md) §8。

## 1. 规则

1. PR 只运行和这次改动有关的任务；main 上每次运行全部任务。
2. PR 推了新提交，旧的那次运行自动取消；main 上的运行不取消。
3. 合并只看一个检查：`ci-ok`。
4. macOS 机器少，一次运行尽量只占一个 macOS 任务。
5. 同一项检查只在一个任务里跑。
6. 改 CI 配置的 PR 运行全部任务。
7. Rust 编译缓存只在 main 上保存；PR 读取 main 的缓存，不保存自己的。
8. 时间目标：普通代码 PR 从推送到出结果不超过 10 分钟，排队也算在内。

## 2. 有哪些 workflow

| workflow | 什么时候运行 | 做什么 |
| --- | --- | --- |
| `ci.yml` | 每个 PR；每次推送 main | 测试和检查（下文） |
| `dylib-windows.yml` | PR 改了 dylib 相关文件（与 `dylib` 开关相同）；每次推送 main；打 `v*` tag；手动 | Windows 上的 dylib 测试 |
| `stress.yml` | 每晚；手动 | 重复运行内核测试和多进程测试；网络长时间测试（下文） |
| `ci-stats.yml` | 每周；手动 | 统计 CI 的耗时和失败 |
| `release.yml`、`release-cli.yml` | 打 tag | 发布 |

### 每晚的 `stress.yml`

只偶尔失败的测试，在 PR 上碰巧撞到时会挡住和它无关的人。`stress.yml` 每晚把这类测试重复跑很多轮，让偶发失败先在夜里出现。它不在 PR 上跑，不影响 PR 的时间。三个任务同时运行：

| 任务 | 做什么 |
| --- | --- |
| `rutis` | 内核测试重复 `rounds` 轮（默认 20）。每轮换测试线程数（1、2、全部核数）、打乱顺序、换种子，每五轮用一次 release 构建。第一次失败就停 |
| `multiprocess` | bridge、loader 中会启动进程、打开本地通道、启动运行时进程的测试，也就是 `ci.yml` 的 `PLATFORM_TESTS_BRIDGE`、`PLATFORM_TESTS_LOADER` 两个列表（加进列表的新测试文件，这里自动跟着跑）。运行时和 `rust` 任务相同：Node 24、Bun 1.4.0、Python 3.12（含 websockets）、Go oldstable。测试只构建一次，然后重复 `rounds` 轮，每轮换测试线程数、打乱顺序、换种子。某一轮失败也跑完所有轮，最后才让任务失败 |
| `soak` | bridge 的 WebSocket 和本地通道长时间测试，各 `soak_secs` 秒（默认 600） |

整个 workflow 的时间由最慢的任务决定：`soak` 约 21 分钟；`multiprocess` 估计 30–45 分钟（构建约 10 分钟，每轮约 1–1.5 分钟）。

- 失败不重试（规范 Q7.3）。
- 每轮在日志里打印线程数和种子。作业摘要里有每一轮的结果表、失败的测试名和它失败的轮次，以及重放命令：用同样的种子、线程数和顺序重跑那个测试文件。
- `multiprocess` 有失败时，每一轮每个测试文件的输出都作为 artifact `multiprocess-logs` 上传，保留 14 天。
- 手动运行：Actions 页面选 `stress`，点 “Run workflow”，可以改 `rounds` 和 `soak_secs`；或者 `gh workflow run stress.yml -f rounds=50`。三个任务都会运行。
- 夜里出现的失败按 §7 处理：登记 issue，再想办法稳定复现。

## 3. PR 上怎么决定跑哪些任务

`ci.yml` 的第一个任务 `changes` 看这个 PR 改了哪些文件，输出下面几个开关。后面的任务根据开关决定跑不跑。推送 main 时所有开关都打开。

| 开关 | 改了这些文件时打开 |
| --- | --- |
| `all` | `Cargo.lock`、根目录的 `Cargo.toml`、`rust-toolchain.toml`、`.github/workflows/**`。打开时其他开关也全部打开 |
| `code` | 除 `docs/**` 和 `*.md` 以外的任何文件 |
| `dylib` | dylib 相关的 crate：`rutis-dylib*`、`rutis-sdk`、`rutis-dev`、`rutis-xtask`，以及链接 SDK、放着 dylib 宿主测试的 `rutis-cli`；`tools/*dylib*`、`tools/build-dylib-bundle.sh`、`tools/test-sdk-bundle.sh`、`tools/lib/**`、`tests/dylib-fixtures/**`。**内核 `crates/rutis` 不在内**：内核改动也会影响 SDK，但 dylib 任务在过去 88 次运行里没有失败过，内核 PR 不再为它等 20 分钟，由合并后 main 上的运行检查 |
| `packaging` | `scripts/train.mjs`、各个 `pyproject.toml` 和 `package.json`、`crates/*/Cargo.toml`、`node/rutis-host/scripts/**` |
| `docs` | 任何 `.md` 文件 |
| `bun` | `bun/**`、`node/rutis/**`、`crates/rutis-bridge`、`crates/rutis-loader`、`crates/rutis-host` |
| `go` | `go/**`、`crates/rutis-bridge`、`crates/rutis-loader`、`crates/rutis-host` |
| `repro` | `crates/rutis-sdk`、`crates/rutis-cli/build.rs`、`tools/test-dylib-repro.sh`、`tools/build-dylib-bundle.sh`、`tools/lib/**`。内核改动也会影响 SDK，但它的可复现检查放在 main 上 |

## 4. 每个任务

“PR 上”一列是这个任务在 PR 上什么时候运行、运行哪些内容；main 上（以及打开 `all` 的 PR 上）每个任务都运行，内容是全量的。

| 任务 | 机器 | PR 上 | main 上多做的 | 内容 |
| --- | --- | --- | --- | --- |
| `links` | Linux | `docs` | — | 检查 Markdown 里的相对链接 |
| `rust` | Linux | `code` | Linux 上 loopback 方式启动运行时的重跑 | 全部 Rust 测试，每项只跑一次：工作区（除 bridge、loader）、loader 全部行类型、bridge 全部 feature。用最低支持版本：Node 24（#219 合并后改 22）、Bun 1.4.0、Python 3.12、Go oldstable |
| `js-py` | Linux | `code` | — | `scripts/train.mjs`；Node 包（`node/rutis`、`rutis-runtime`、`rutis-host`）和 rutis-dsh 的 dsh 测试；Python 单元测试 |
| `checks` | Linux | 只在 `all` 时 | 整个任务 | `cargo check --workspace --all-targets`；bridge、loader 各个 feature 组合的编译检查和 `--no-run`。这些检查发现的是少见的 feature 组合编译错误，不跑测试 |
| `network-macos` | macOS | `code`：只跑和平台有关的测试 | bridge、loader 全部测试；`node/rutis` 测试 | Node 26、最新 Bun、Go stable 下的进程启动、本地通道、运行时进程、WebSocket、rutis-host 测试；Node 运行时和 Python 包的测试；整个工作区的编译检查 |
| `runtimes-windows` | Windows | `code`：只跑和平台有关的测试 | bridge、loader 全部测试 | 同上，在 Windows 上（进程用 loopback 方式启动）；Go SDK 测试；整个工作区的编译检查 |
| `runtimes-bun` | Linux | `bun`：Bun 1.4.0 | Linux 最新版；macOS 1.4.0 | Bun 运行时的全部测试，包括 loopback 方式启动的重跑 |
| `runtimes-go` | Linux | `go`：Go oldstable | Linux stable；macOS oldstable；Linux 上 loopback 方式启动的重跑 | Go SDK 与 Go 运行时的全部测试 |
| `semver-rutis` | Linux | `packaging` | 整个任务 | 公开 API 和上次发布相比有没有不兼容的改动（只提示） |
| `dylib-linux-launcher`、`dylib-linux-sdk-bundle` | Linux | `dylib` | — | dylib 测试，分两个任务同时跑 |
| `dylib-linux-repro` | Linux | `repro` | 整个任务 | SDK 可复现构建（在一台机器上用两个目录各构建一次） |
| `dylib-macos` | macOS | `dylib` | 可复现构建一步 | 同样的 dylib 测试，在一个任务里依次跑，共用一次构建的 bundle；可复现构建这一步只在 `repro` 打开时跑 |
| `release-dry-run`、`release-windows`、`release-wheel-aarch64` | Linux、Windows、Linux | `packaging` | 整个任务 | 发布要用的包能构建 |
| `sdk-repro`、`sdk-repro-macos` 及比对 | Linux、macOS | 只在 `all` 时 | 整个任务 | 在两台机器上分别构建 SDK，比较结果是否相同 |
| `ci-ok` | Linux | 总是 | — | 汇总（§5） |

### 和平台有关的测试

`ci.yml` 顶部的 `PLATFORM_TESTS_BRIDGE`、`PLATFORM_TESTS_LOADER` 列出 PR 上 macOS、Windows 跑哪些测试文件：会启动进程、打开本地通道（Unix socket、继承的 socket、loopback 交接）、启动运行时进程、使用 WebSocket 的测试，加上两个 crate 的单元测试（`--lib`）；rutis-host 的测试全跑。其余测试（内存通道上的协议、行的配置和生命周期等）只依赖 Rust 代码本身，PR 上在 Linux 的 `rust` 任务里跑，main 上三个平台都跑。

新加的测试文件如果会启动进程或打开通道，要加进这两个列表。每晚 `stress.yml` 的 `multiprocess` 任务也按这两个列表重复运行（§2）。

### 每种改动在 PR 上跑什么

| 改动 | 运行的任务 | 预计时间（推送到结果） |
| --- | --- | --- |
| 只有文档 | `changes`、`links` | 约 1 分钟 |
| 内核 `crates/rutis` | `rust`、`js-py`、`network-macos`、`runtimes-windows` | 约 5–8 分钟 |
| bridge、loader、host | 再加 `runtimes-bun`、`runtimes-go`（Linux 各一个版本） | 约 6–9 分钟 |
| 只有 Python 包 | `rust`、`js-py`、`network-macos`、`runtimes-windows` | 约 5–8 分钟 |
| 改了 `package.json`、`pyproject.toml`、`crates/*/Cargo.toml` | 再加三个打包任务和 `semver-rutis` | 不变，打包任务与测试并行 |
| dylib | 再加 dylib 任务；`dylib-windows.yml` | 约 20–25 分钟（dylib 的预算） |
| CI 配置、`Cargo.lock`、根 `Cargo.toml` | 全部，同 main | 约 25 分钟 |

## 5. 合并条件

- `ci-ok` 是 main 分支唯一要求通过的检查。
- `ci-ok` 只看列在它 `needs` 里的任务：这些任务成功或被跳过，它就通过；有任务失败或被取消，它就失败。
- 没有列进 `ci-ok` 的任务，失败了也不会阻止合并。

## 6. 修改 CI 时要做的事

### 新增 CI 检查的流程

CI 慢下来，通常是一项一项加检查加出来的。新增检查（新任务、已有任务里的新步骤、新的版本或平台）前按下面的顺序回答：

1. **它防的是哪类问题，放在哪一级。** 级别是 PR、main、每日（`stress.yml`）、每周。默认放 main 或每日；只有必须在合并前拦住的问题才放 PR。放 main 的检查失败了，按 §7 优先处理，同样会被发现，只是晚一点。
2. **放 PR 的，写明它占多少时间。** 它不能让关键路径（最慢的那个任务）变长：放进一个并行的 Linux 任务，或者在已有任务里替换掉别的内容。不新增 macOS、Windows 任务；macOS、Windows 上只跑和平台有关的测试（§4）。
3. **能替换就不新增。** 例如支持矩阵的最低版本：把 Linux 任务里的版本换成最低版本，macOS 保持最新版，而不是再加一个任务跑最低版本。
4. **目标：普通代码 PR 从推送到出结果不超过 10 分钟，排队也算在内。** 用 `node tools/ci-stats.mjs [次数]` 测量（每周的 `ci-stats.yml` 也会跑）。超出时，从关键路径上的任务开始调整：拆成并行任务，或者把成本高、很少失败的部分移到 main。

把这几条的答案写进 PR 描述。

### 加一个任务

1. 先按上面的流程决定它放在哪一级。
2. 给它加 `needs: changes` 和 `if: needs.changes.outputs.<开关> == 'true'`（只在 main 上跑的用 `all`）。没有合适的开关就在 `changes` 里加一个，并写进 §3。
3. 把它加进 `ci-ok` 的 `needs`。
4. 看已有任务里是不是已经在做同样的检查；能放进已有任务的，不新开任务。
5. 要用 macOS 时，先看能不能放进 `network-macos`。普通改动最多占 1 个 macOS 任务，改 dylib 时最多 2 个。
6. 写进 §4。

### 加一种语言运行时

1. `changes` 加一个这种语言的开关，包括它的运行时目录，以及 `crates/rutis-bridge`、`crates/rutis-loader`、`crates/rutis-host`。
2. 在 `rust`（最低支持版本）和 `network-macos`（最新版本）里安装这种语言，在已有的 bridge、loader、host 测试步骤里加上它的 feature；它的测试文件按 §4 加进 `PLATFORM_TESTS_*`。
3. 如果要测多个版本，加一个 `runtimes-<语言>` 任务：按第 1 步的开关运行，加进 `ci-ok`，PR 上只在 Linux 跑最低支持版本；其他版本和 macOS 只在 main 上跑（做法见 `runtimes-bun` 和 `changes` 的 `bun-matrix`）。
4. 发布相关：`release-dry-run`、`scripts/train.mjs`、`release.yml` 都加上它的包。
5. 更新 §3、§4。

### 改开关对应的文件

- 改 `changes` 等于改了 `.github/workflows/**`，这个 PR 会运行全部任务。
- 如果某个任务在 PR 上被跳过，合并后在 main 上失败了，说明开关漏了某些文件：修好失败后，把漏掉的文件加进开关。

### 改 CI 配置

- 单独开 PR，提交类型用 `ci:`。
- 改 CI 配置的 PR 运行全部任务，所以只在 PR 上跑的那部分（例如 macOS、Windows 上的测试子集）在这个 PR 里不会跑到；合并后看之后第一个普通 PR 的运行。
- 合并后看 main 上第一次运行的结果。

## 7. 失败怎么处理

- main 上失败优先处理：找出是哪次合并引起的，修好之前不再合并可能受影响的改动。
- main 上的运行被手动取消后，要重新运行，每次合并都要有完整结果。
- 偶尔失败的测试：先想办法稳定复现，再修。不用自动重试来掩盖。

## 8. 编译缓存

- 所有任务用 `Swatinem/rust-cache`，设置 `save-if: github.ref == 'refs/heads/main'`：只有 main 上的运行保存缓存。
- PR 只能读取 main 和它自己的缓存。PR 不保存，是为了不占用仓库 10 GB 的缓存空间，把 main 的缓存挤掉。
- 加任务时照此设置（`ci.yml` 里 `rust` 任务的 `rust-cache` 处有说明）。
- 不能用缓存的检查（SDK 可复现构建）放在 main 上，PR 只在 `repro` 打开时运行。
