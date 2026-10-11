# CI 静态检查与最低版本矩阵（设计稿）

[English](design-ci-checks-matrix-2026-10-11.en.md)

状态：设计稿，待评审（#204）。日期：2026-10-11。基准：`main` `0941ef4`。
依据：[质量规范](quality-standard.md) Q5.3.7、Q10.2、Q10.3、§12；[CI 说明](ci.md)（#237）；#204 的"范围调整（2026-10-10）"。属于 #183 第一步。

硬约束（来自 #204）：普通代码 PR 从推送到出结果 ≤ 10 分钟（含排队）；PR 上不新增 macOS、Windows 任务；新检查放进并行的 Linux 任务；能替换就不新增。

范围外：actionlint；Go 的 `govulncheck`；Python 代码风格检查；许可证检查。

## 一、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| D1 | 版本按机器分：Linux 任务用每种语言的最低支持版本，macOS 和 Windows 用最新版本 | 三 |
| D2 | websockets：Linux 装 `websockets==15.*`（声明的下限），其他地方装 `websockets>=15` | 三 |
| D3 | 新增一个并行的 Linux 任务 `lint`（`code` 开关）：`cargo fmt --check`、`cargo clippy -D warnings`；本 PR 把现有的 4 处格式差异和 22 个 clippy 警告全部改掉，不设基线 | 四 |
| D4 | MSRV 统一为 1.88；示例项目 `rutis-agent`、`rutis-cli` 不声明 MSRV；main 上检查一次 | 五 |
| D5 | 漏洞检查放进每周运行的新 workflow `deps.yml`，不阻止合并；失败时由 GitHub 的 workflow 失败通知告诉维护者 | 六 |
| D6 | `checks` 里的 `cargo check --workspace --all-targets` 删掉：`lint` 的 clippy 已经编译了同样的代码 | 四 |
| D7 | `tools/ci-stats.mjs` 能单独统计普通代码 PR；验收看这一类 | 八 |
| D8 | 其他 PR 需要的 CI 改动（#186 的 `e2e` 任务、#193 的打包与安装）由本 issue 在那些 PR 合入时加 | 七 |

## 二、现状（实测）

### 2.1 PR 时间

`node tools/ci-stats.mjs 40 ci.yml`（2026-10-11，#237 合并之后的 40 次运行）报告的 `pull_request` 中位数是 19.5 分钟，但它把打开 `all` 的 PR（约 20–40 分钟）、普通代码 PR 和重跑过的运行（重跑从第一次创建时算起，例如 run `38056277205` 记为 659 分钟）混在一起。只看普通代码 PR（7–9 个任务，第一次运行，成功）：

| run | 分支 | 推送到结果（分钟） |
| --- | --- | ---: |
| 38097374545 | fix/238-multilang-go-flaky | 5.2 |
| 38056775967 | fix/233-exit-status-flaky | 5.6 |
| 38055889193 | fix/239-handshake-flaky | 7.4 |
| 38098778901 | fix/173-local-line-limit | 8.7 |
| 38098889402 | fix/184-concurrent-launch | 8.7 |
| 38100944545 | fix/247-row-self-dispose | 8.8 |

中位数约 8.0 分钟（6 次，样本少）。关键路径是 `rust`（约 4.5 分钟）和 `runtimes-windows`（4.2–5.1 分钟）。Linux 也会排队（1.8–2.3 分钟），原因是每次推送 main 同时占 28 个任务。

### 2.2 fmt 与 clippy

- `cargo fmt --all --check`（1.98.1）：4 处差异，在 `crates/rutis-xtask/src/main.rs`（3 处）、`examples/native-mount/tests/cordis_mount.rs`（1 处）。
- `cargo clippy --workspace --all-targets --all-features`（1.98.1，冷构建 56 秒）：22 个警告，9 种 lint，没有错误。

| lint | 个数 | 位置 |
| --- | ---: | --- |
| `clippy::await_holding_lock` | 7 | `crates/rutis-agent/tests/session_persist.rs` |
| `dead_code` | 5 | `tests/dylib-fixtures/greeter-v2/src/lib.rs`（4，只在 `--all-features` 下：几个 feature 是互斥的版本）；`crates/rutis-loader/tests/migration_example.rs`（1） |
| `clippy::doc_lazy_continuation` | 3 | `crates/rutis-agent/src/driver.rs`、`session.rs` |
| `clippy::missing_safety_doc` | 2 | `crates/rutis-sdk/src/lib.rs`、`crates/rutis-dylib/src/loader.rs` |
| `clippy::type_complexity` | 1 | `crates/rutis-bridge/tests/memory_mux.rs` |
| `clippy::map_flatten`、`unnecessary_mut_passed`、`items_after_test_module` | 各 1 | `crates/rutis-agent/src/driver.rs`、`tui.rs` |
| `clippy::redundant_closure` | 1 | `crates/rutis-cli/src/main.rs` |

两个前提，CI 里也一样：`--all-features` 会打开 `tests/dylib-fixtures/greeter-*` 的 `export` feature，它在编译时读 `RUTIS_SDK_ARTIFACT_SHA256`，CI 照 `sdk-repro` 设成 64 个 `0`；`examples/native-mount`、`examples/interop-experiments` 的 build.rs 要先 `npm --prefix node/rutis-runtime ci`，`rutis-dsh` 要先 `npm --prefix crates/rutis-dsh/dsh ci`。

### 2.3 MSRV

声明：根 `Cargo.toml` `rust-version = "1.85"`，所有 crate 继承。用当前的 `Cargo.lock`（`--locked`）检查：

| 命令 | 结果 |
| --- | --- |
| `cargo +1.85 check --workspace` | 失败：依赖要求更高的 rustc（`icu_*` 2.3、`darling` 0.24 要 1.88，`idna_adapter` 1.2.2 要 1.86，`tree-sitter-language` 0.1.8 要 1.90） |
| `cargo +1.85 check -p rutis -p rutis-bridge --all-features` | 通过 |
| `cargo +1.88 check --workspace --exclude rutis-agent --exclude rutis-cli` | 通过 |
| `cargo +1.88 check -p rutis-agent` | 失败：`tree-sitter-language` 要 1.90 |

`rutis-loader`、`rutis-host` 到 1.88 是因为 `url` → `idna` → `icu_*`；`rutis-agent`、`rutis-cli` 到 1.90 是因为 rutui 用的 `tree-sitter-language`。没有验证：Linux、Windows 目标；用户自己解析依赖时会选到哪些版本。

### 2.4 依赖

- `cargo deny --all-features check advisories`：没有漏洞；4 个"不再维护"（async-std、bincode、paste、yaml-rust），都经 `rutis-agent` 的 rutui 引入。
- `npm audit`：`node/rutis-runtime`、`node/baseline` 没有问题；`crates/rutis-dsh/dsh`（不发布）有 13 个。
- `pip-audit`：`websockets==15.0.1` 没有已知漏洞。

### 2.5 版本写在哪里

| 位置 | Node | Python | websockets |
| --- | --- | --- | --- |
| `ci.yml` `rust`、`js-py`、`checks`、`runtimes-go`、`runtimes-bun` | 24 | 3.12 | `>=13` |
| `ci.yml` `network-macos` | 26 | 3.12 | `>=13` |
| `ci.yml` `runtimes-windows` | 24 | 3.12 | `>=13` |
| `release.yml`、`ci.yml` `release-*` | 24 | — | — |
| `release-cli.yml` | — | — | — （Rust 用 `@stable`，其他地方都是 1.98.1） |
| `stress.yml` `multiprocess`（#245） | 24 | 3.12 | `>=13` |
| 声明 | `engines.node` `>=22`（#219） | `requires-python >=3.12`（#196 前） | `websockets>=15`（#218） |

声明已经改了，CI 还停在旧值：Node 最低是 22，CI 跑 24；websockets 下限是 15，CI 装 `>=13`。

## 三、版本矩阵

规则：每种语言，Linux 任务用最低支持版本，macOS 和 Windows 用最新版本。只换版本号，不新增任务。

| 任务 | 机器 | Node | Python | websockets |
| --- | --- | --- | --- | --- |
| `rust`、`js-py`、`checks` | Linux | 24 → **22** | 3.12（#196 合并后 **3.10**，由 #196 的 PR 改） | `>=13` → **`==15.*`** |
| `runtimes-go`、`runtimes-bun`（Linux） | Linux | 24 → **22** | 同上 | `>=13` → **`==15.*`** |
| `network-macos` | macOS | 26 | 3.12 → **`3.x`** | `>=13` → **`>=15`** |
| `runtimes-windows` | Windows | 24 → **26** | 3.12 → **`3.x`** | `>=13` → **`>=15`** |
| `stress.yml` `multiprocess` | Linux | 跟 `rust` | 跟 `rust` | 跟 `rust` |
| `release.yml`、`release-*` | — | 24（发布工具的版本，不属于支持矩阵，不变） | — | — |

另外：`release-cli.yml` 的 `dtolnay/rust-toolchain@stable` 改成 `@1.98.1`（仓库根的 `rust-toolchain.toml` 决定 cargo 实际用的版本，`@stable` 只是多装一个用不到的工具链）。

对关键路径没有影响：`ubuntu-24.04` 镜像自带 Node 22，`pip install "websockets==15.*"` 几秒。

## 四、fmt 与 clippy：任务 `lint`

```yaml
lint:
  needs: changes
  if: needs.changes.outputs.code == 'true'
  runs-on: ubuntu-24.04
```

步骤：checkout；`dtolnay/rust-toolchain@1.98.1`（`rustfmt, clippy`）；`Swatinem/rust-cache@v2`（只在 main 上保存）；`cargo fmt --all --check`；Node 22 和两次 `npm ci`（§2.2 的前提）；`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`，`RUTIS_SDK_ARTIFACT_SHA256` 设成 64 个 `0`。`-D warnings` 写在 `--` 后面，不写进 `RUSTFLAGS`（改 `RUSTFLAGS` 会让依赖全部重新编译）。只用 clippy 默认的 lint 组。加进 `ci-ok`。

本 PR 把 §2.2 的 4 处格式差异和 22 个警告全部改掉（约 10 个文件，都是机械的改动）。greeter-v2 夹具的 `dead_code` 来自互斥的 feature，用普通的 `#[allow(dead_code, reason = "…")]`。以后升级工具链出现的新 lint，在升级的 PR 里改掉。

时间：有缓存时估计 3–4 分钟，与 `rust` 并行，不在关键路径上；没有缓存时 5–7 分钟。合并后如果 `lint` 的中位数超过 `rust`，先查缓存是否命中，再考虑把 clippy 拆成两个并行任务。

D6：`checks` 里的 `cargo check --workspace --all-targets` 删掉。`network-macos`、`runtimes-windows` 里的同一条命令保留，它们检查各自平台的编译。

## 五、MSRV

- 根 `Cargo.toml` 的 `rust-version` 改为 `"1.88"`。
- `crates/rutis-agent`、`crates/rutis-cli` 是示例项目，去掉 `rust-version.workspace = true`，不声明 MSRV。
- README、`docs/guide/README.md`（中英）里"Rust 1.85"改为 1.88。
- 检查：一个 Linux 任务 `msrv`，只在 `all` 时运行（main，以及改了 `Cargo.lock`、`Cargo.toml` 的 PR）：`dtolnay/rust-toolchain@1.88`，`cargo +1.88 check --workspace --exclude rutis-agent --exclude rutis-cli --locked`。只检查库和二进制（测试依赖 `rcgen`、`time` 要 1.88，同样能过，但不承诺）。约 3 分钟，普通 PR 不跑。

以后不通过时：把那个依赖锁在旧版本（`cargo update -p <依赖> --precise <版本>`），或者提高 `rust-version` 并同步改 README、guide。提高 `rust-version` 是用户能看到的变化，由维护者决定。

MSRV 只按我们的 `Cargo.lock` 验证；guide 里写一句：旧版 Rust 的用户可以用 Cargo 的 `incompatible-rust-versions = "fallback"` 解析依赖。

## 六、每周依赖检查：`deps.yml`

| 任务 | 命令 |
| --- | --- |
| `cargo-deny` | `cargo deny --all-features --exclude rutis-agent --exclude rutis-cli check advisories`（示例项目不检查，4 个"不再维护"都来自它们） |
| `npm-audit` | `npm --prefix node/rutis-runtime audit --omit=dev --audit-level=high` |
| `pip-audit` | 装 `./python/rutis[network]` 后 `pip-audit` |

- 触发：每周一次（`schedule`）、手动（`workflow_dispatch`）。
- 不进 `ci-ok`，不阻止合并：漏洞库每天更新，结果会在代码没变时变化，放在 PR 上会让无关的 PR 失败。
- 失败时：GitHub 把 workflow 失败通知发给维护者（@eric8810）。不另做开 issue 的自动化。
- 不检查 `crates/rutis-dsh/dsh`（不发布）和 `node/baseline`（测试用）。
- 不加 `deny.toml`：`advisories` 用默认配置就能跑。

## 七、其他 PR 需要的 CI 改动

`ci.yml` 只由 #203 / #204 修改的约定下，下面这些在对应 PR 合入时由本 issue 加（#196 例外，它自己改 Linux 的 Python 版本）：

| 来自 | 改动 |
| --- | --- |
| #186（#253） | 新任务 `e2e`（Linux，`code` 开关，加进 `ci-ok`）：与 `rust` 相同的最低版本，`cargo test -p rutis-e2e`；`rust` 的 `cargo test --workspace` 加 `--exclude rutis-e2e`；`network-macos` 加 `cargo test -p rutis-e2e` |
| #193（#254） | main 上调用 `package.yml` 和 `install.yml`；去掉 `release-windows`、`release-wheel-aarch64` |
| #196（#255） | Linux 的 Python 换成 3.10（由 #196 的 PR 自己改） |

## 八、测量与 `docs/ci.md`

`tools/ci-stats.mjs`：

- 加一类"普通代码 PR"：`rust` 跑了、`checks` 没跑、没有 `dylib-` 任务，且是第一次运行（`run_attempt == 1`）；
- 加参数 `--since <日期>`；
- 输出一行：`普通代码 PR：中位数 X 分钟，p90 Y，共 N 次；目标 ≤ 10`。

验收：第一阶段合并的日期记为 T；T + 7 天运行 `node tools/ci-stats.mjs 100 ci.yml --since T`，普通代码 PR 中位数 ≤ 10 分钟（样本少于 10 次时再延长一周），结果写进 #204。

`docs/ci.md`、`docs/ci.en.md`：任务表加 `lint`、`msrv`，workflow 表加 `deps.yml`；新增一节"版本"，写明规则（Linux 最低、macOS 和 Windows 最新）和声明的位置，以及"改了 `engines`、`requires-python`、websockets 下限或 `rust-version`，要在同一个 PR 里改 CI 的版本"；删掉"#219 合并后改 22"这类过时的说法。

## 九、分阶段与验收

在本 PR 里按提交分开：

1. 版本（§三，不含 Python 3.10）；`release-cli.yml`。
2. fmt、clippy 的修改；任务 `lint`；`checks` 删掉重复的命令。
3. MSRV（§五）。
4. `deps.yml`；用 `workflow_dispatch` 在本分支上跑一次。
5. `tools/ci-stats.mjs`；`docs/ci.md`、`docs/ci.en.md`。

验收（都可以用命令检查）：

1. `git grep -n 'websockets>=13' .github` 没有输出；Linux 任务的 `node-version` 是 22。
2. `ci.yml` 里 `runs-on` 为 macOS 或 Windows 的任务名单与 `0941ef4` 相同。
3. `lint` 在 `ci-ok` 的 `needs` 里，条件是 `code`；本地 `cargo fmt --all --check` 与 `cargo clippy … -- -D warnings` 通过。
4. 合并后 main 上 `msrv` 成功；`rutis-agent`、`rutis-cli` 的 `Cargo.toml` 没有 `rust-version`。
5. `deps.yml` 在本分支上手动运行一次成功。
6. T + 7 天：普通代码 PR 中位数 ≤ 10 分钟，样本 ≥ 10 次。

## 十、维护者的决定（2026-10-11）

- 规则统一：Linux 用最低版本，macOS 和 Windows 用最新版本；Windows 的 Node 改为 26、Python 改为最新。
- Python 的"最新"写作 `3.x`，自动跟随；新版本让 PR 失败时，临时写死上一版并另开 PR 修。
- MSRV 统一 1.88，示例项目 `rutis-agent`、`rutis-cli` 不声明。
- 22 个 clippy 警告一次改掉，不设基线。
- 每周依赖检查失败由 GitHub 通知 @eric8810。
- #196 自己改 Linux 的 Python 版本；#186、#193 需要的 CI 改动由本 issue 加。
