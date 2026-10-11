# S2 端到端：插件作者的开发循环（设计稿）

[English](design-e2e-dev-loop-2026-10-11.en.md)

状态：设计，待评审。日期：2026-10-11。基准：`main` `0941ef4`。
关联：[#186](https://github.com/arcships/rutis/issues/186)（[#183](https://github.com/arcships/rutis/issues/183) 第一步）。依据：[质量规范](quality-standard.md)（下称"规范"）Q6.6、Q7；[质量执行现状](quality-status.md)（下称"现状"）场景 A，§9.1；E2E 框架 [#221](https://github.com/arcships/rutis/pull/221)（`tests/e2e/`）；[rutis-host 指南](guide/rutis-host.md)；[CI](ci.md)。

范围外：从 npm / PyPI 安装（S9 [#193](https://github.com/arcships/rutis/issues/193)）；`run` 的信号处理（`crates/rutis-host/tests/signals.rs` 已覆盖）；`check` 对手写配置的输出（`crates/rutis-host/tests/check.rs`、#205 已覆盖）；kill 后的残余（`tests/e2e/tests/cross_language.rs` 已覆盖）；反复重新加载、长时间运行（S8 #192）；`rutis-host` 本身的行为修改。

## 一、结论

| # | 结论 | 节 |
| --- | --- | --- |
| C1 | 一个测试文件 `tests/e2e/tests/dev_loop.rs`，每种语言一个测试：`node`、`python`、`bun`、`go`；每个测试走同一串步骤：`new → 模板测试 → check → dev → 改成 v2 → 改坏 → 修好 → Ctrl-C` | 三 |
| C2 | v2、v3 和改坏版本由场景整份写入，v1 就是 `new` 生成的模板；v2、v3 在 apply 和清理时各打印一行，用来断言"旧实例卸载了且恰好一次" | 3.1 |
| C3 | 第 2 步用仓库里的包（链接 `node/rutis`、`bun/rutis-bun`、tsx；`RUTIS_PYTHON_PATH`；Go 的 `replace`），不连注册表；从注册表安装由 #193 负责 | 3.3 |
| C4 | 只断言退出码和 rutis 自己写的几行输出，期望写在测试代码里（与 `check.rs` 相同）；第三方工具的错误文字只断言"含文件名和行号" | 五 |
| C5 | 同步点全部是输出行或进程退出；改文件用"同目录点文件 + rename"并保证 mtime 变化；不用 sleep | 六 |
| C6 | 本 PR 做 Linux、macOS；Windows 在 #232 之后另开 PR | 七 |
| C7 | CI：新开一个并行的 Linux `e2e` 任务，macOS 放进 `network-macos`，由 #204（#256）加 | 八 |

## 二、现状（核对过的代码）

| 已有测试 | 测了什么 | 没测什么 |
| --- | --- | --- |
| `crates/rutis-host/src/new.rs` `projects_are_created_with_their_names_filled_in` | 生成的文件内容 | 生成的项目能不能测试、运行 |
| `crates/rutis-host/tests/project.rs` | `project::dev_config` 对各种项目的解析（库调用） | 二进制、重新加载 |
| `crates/rutis-host/tests/check.rs` | `check` 的 stdout / stderr / 退出码（手写的配置，含缺包） | 模板生成的项目 |
| `crates/rutis-host/tests/signals.rs`（Unix） | `run` / `dev` 在各种信号下的退出码与清理次数 | 改文件后的重新加载 |
| `tests/e2e/tests/cross_language.rs` | `run` 下跨语言调用；kill 后残余 | `new` / `dev` / `check` |

**`dev` 的行为**（`crates/rutis-host/src/main.rs` `dev`，`project.rs`）

- 启动后先取一次文件快照（`project::sources`），再打印 `rutis-host dev: running <id>; changes reload it (Ctrl-C ends)`。这一行之后的修改一定会被看到。
- 每 400 ms 重新取快照，比较 `(路径, mtime)`。跳过 `node_modules`、`.git`、`.venv`、`venv`、`target`、`dist`、`build`、`__pycache__`、`.pytest_cache` 和 `.` 开头的名字（`.env` 除外）。
- 有变化时（非 Go 项目）：`host.invalidate()`，再对所有非 peer 行逐个 `loader.reload`，每行打印 `<id>: reloaded` 或 `<id>: cannot reload: <错误>`。`rutis.dev.json` 里加的行（包括场景的 probe）也会重新加载。`Loader::reload` 全有或全无：新模块解析失败时旧模块继续运行。
- Go 项目：重新 `go build` 到 `.rutis/go/<名字>-<n>`；成功时换二进制、重启运行时，打印 `<运行时>: rebuilt and restarted`；失败时打印 `<名字>: the build failed; the last build keeps running` 和编译器输出。Go 项目的其他行不重新加载。
- 状态行（`status::follow`，每 200 ms 比较一次）只在变化时打印，中间状态可能被跳过；场景只等稳定状态。

**`check`**：在没有 `rutis.json` 的插件项目里检查项目本身（`dev_config`）。不检查 `inject` 的服务有没有行提供，另见 #258。

## 三、场景

### 3.1 公共做法

- 场景目录 `<root>/<场景>-<pid>-<n>/project/`；`rutis-host new demo --lang <语言>` 在这里执行，生成 `project/demo/`，之后的命令在 `demo/` 里执行。
- 插件的四个版本：

| 版本 | 来源 | `greeter.hello("Ada")` | 打印 |
| --- | --- | --- | --- |
| v1 | `new` 生成的模板，不改 | `Hello, Ada!` | 无 |
| v2 | 场景整份写入 | `Hello, Ada! (v2)` | apply 时 `demo v2 applied <解释器>`，清理时 `demo v2 cleanup` |
| 改坏 | v2 在固定的第 5 行加一处语法错误（Go：编译错误） | — | — |
| v3 | 场景整份写入 | `Hello, Ada! (v3)` | `demo v3 applied …`、`demo v3 cleanup` |

  整份写入而不是在模板上替换字符串：模板改了措辞不会让场景失效，模板本身由 v1 那几步测试。

- probe（`tests/e2e/probes/`）作为 `rutis.dev.json` 的额外行，在第 3 步之后写入，所以第 3 步检查的是原样的模板项目。

| 项目 | probe | 放在 | `rutis.dev.json` 加的 runtimes |
| --- | --- | --- | --- |
| node | TS | `demo/probe.ts` | 无 |
| python | Python | `demo/src/probe.py`（Python 运行时的项目是 `src/`） | 无 |
| bun | Python | `demo/dev/probe.py` | `"py": { "project": "dev" }` |
| go | Python | `demo/dev/probe.py` | `"py": { "project": "dev" }` |

### 3.2 步骤

"等"一律指等到某一行输出或进程退出（`Host::expect` / `wait_for` / `wait_exit`），只有防挂死超时。

| # | 动作 | 断言 |
| --- | --- | --- |
| 1 | `rutis-host new demo --lang <语言>` | 退出码 0；stdout 含 `created demo/`；生成的文件列表等于测试里写的列表 |
| 2 | 模板自带的测试（3.3） | 退出码 0 |
| 3 | `rutis-host check`（无参数） | 退出码 0；stdout 列出行 `demo`、它的 `provides`；stderr 为空 |
| 4 | 写 probe 和 `rutis.dev.json`，`rutis-host dev`；等 `rutis-host dev: running demo; …` 和 probe `started` | `hello("Ada")` = `Hello, Ada!` |
| 5 | 改成 v2；等 `demo: reloaded`、probe `started`、`demo v2 applied` | 结果 = `… (v2)`；Python：解释器在 `demo/.venv` 下（A8） |
| 6 | 改坏；等以 `demo: cannot reload: ` 开头的行、probe 重新 `started` | 错误含文件名和行号 5（A7）；结果仍 = `… (v2)`；`demo v2 cleanup` 出现 0 次（旧实例没被卸载，A4） |
| 7 | 修好（v3）；等 `demo: reloaded`、`demo v3 applied`、probe `started` | 结果 = `… (v3)`；`demo v2 cleanup` 恰好 1 次（A3） |
| 8 | Ctrl-C（`killpg(SIGINT)`）；`wait_exit` | 退出码 0；`demo v3 cleanup` 恰好 1 次；probe `stopped`（B2） |
| — | `Scenario::finish()` | 残余检查通过 |

### 3.3 各语言的差异

| | node | python | bun | go |
| --- | --- | --- | --- | --- |
| 第 2 步准备 | 链接 `node_modules/@arcships/rutis` → `node/rutis`，`node_modules/tsx` → `node/rutis-runtime/node_modules/tsx` | `python -m venv --without-pip .venv`；`rutis` 来自 `RUTIS_PYTHON_PATH` | 链接 `@arcships/rutis`、`@arcships/rutis-bun` → `bun/rutis-bun` | `go.mod` 追加 `replace github.com/arcships/rutis/go/rutis => <repo>/go/rutis` |
| 第 2 步命令 | `npm test` | `.venv` 的 python `-m unittest discover -s tests`，`PYTHONPATH=src` + `python/rutis` | `bun test` | `go test ./...`，`GOTOOLCHAIN=local`、`GOPROXY=off` |
| 第 5–7 步的输出 | `demo: reloaded` / `demo: cannot reload: …` | 同左 | 同左 | `go-demo: rebuilt and restarted` / `demo: the build failed; the last build keeps running` |
| 第 7 步额外断言 | — | — | — | 旧运行时进程已退出；`.rutis/go/` 只剩最新的二进制 |
| probe 在第 5–7 步 | 随每次重新加载重启 | 同左 | 同左 | 不重新加载；随 Go 运行时重启停下再启动 |
| 不做的 | — | `uv sync`（需要注册表，#193） | `bun run check`、`bunx --bun`（需要注册表，#193） | — |

Bun 运行时自己也比较入口文件的 mtime 和大小，所以 `replace` 保证 mtime 变化对 Bun 同样必要。

## 四、harness 要补的部分

都在 `tests/e2e/src/`，不依赖任何 rutis crate（Q7.6）。

| 新增 | 作用 |
| --- | --- |
| `Scenario::host_in(dir, args)` | 在项目子目录里启动宿主；环境与 `host_with` 相同 |
| `Scenario::program(dir, program, args, env) -> Host` | 运行 npm / python / bun / go：同样的临时目录、输出捕获、进程组、残余登记 |
| `Scenario::replace(relative, contents)` | 写到同目录的 `.<名字>.part`，再 `rename`；新 mtime 与旧的相同时 `set_modified(旧 + 1 s)` |
| `Scenario::link(project, package, target)` | 把仓库里的包链接进项目（`link_node_sdk` 的通用版） |
| `Host::count(text)` | 到目前为止含 `text` 的行数；"恰好一次"在宿主退出后再数 |
| `Host::ctrl_c()` | `killpg(SIGINT)`（终端的做法） |

## 五、断言哪些输出

只锁 rutis 自己写的、写进文档的输出，期望直接写在测试代码里：`new` 的 `created demo/`、生成的文件列表；`check` 的行；`rutis-host dev: running demo; …`；`demo: reloaded`、`demo: cannot reload: ` 前缀、Go 的两行；每一步的退出码。

不锁：tsx、Bun、Python、go 编译器的错误文字（只断言含文件名和行号）；状态行的顺序；`npm test` 等工具的输出。路径比较前把场景目录替换成 `<dir>`。

## 六、确定性（Q7.1）

| 可能不确定的地方 | 做法 |
| --- | --- |
| 改文件太早，`dev` 还没取第一次快照 | 等 `rutis-host dev: running …` 再改 |
| 两次修改的 mtime 相同 | `replace` 检查并把 mtime 推后 1 s |
| 写到一半被轮询看到 | 点文件 + `rename`，替换是原子的 |
| 两次修改落在同一次轮询里 | 每次修改后等到这次重新加载的结果行和 probe `started` 再做下一次 |
| "旧实例没卸载" | 改写为"`cannot reload` 和 probe 重新 `started` 已发生，而 `cleanup` 仍未出现"（Q7.1.1） |
| "恰好一次" | 宿主退出、输出关闭之后再数 |
| 超时 | 只有 harness 的防挂死超时（默认 30 s，`RUTIS_E2E_TIMEOUT`）；Go 的第 2 步 `go test` 先把构建缓存热起来 |

不用 sleep。

## 七、平台

Linux、macOS：四种语言都跑，Ctrl-C 用 `killpg(SIGINT)`。Windows 在 #232（Job Object 残余检查）之后另开 PR：没有进程检查，B2 在 Windows 上验证不到；那时再决定怎样发 Ctrl-C。Bun 在 Windows 上尚未声明支持（Bun 设计 §10），届时也跳过并写明原因（Q7.8）。

## 八、CI 与时间

估计（实现时在 CI 上实测并写进 PR）：每种语言约 10 s，Go 约 15–20 s（含构建）；四个测试并行，Linux 上约 20–30 s。

需要的 CI 改动（由 #204 的 PR #256 加，本 PR 不改 `ci.yml`）：

| 改动 | 时间 |
| --- | --- |
| 新任务 `e2e`（Linux，`code` 开关）：与 `rust` 相同的最低版本 Node、Bun、Python、Go；`cargo test -p rutis-e2e`；失败时上传 `RUTIS_E2E_DIR`；加进 `ci-ok` | 与 `rust` 并行，约 3–5 分钟 |
| `rust` 任务的 `cargo test --workspace` 加 `--exclude rutis-e2e` | 一项检查只在一处跑（Q12.7） |
| `network-macos`：加 `cargo test -p rutis-e2e` | 约 +1 分钟，不新增 macOS 任务 |

## 九、覆盖的风险

| 风险 | 级 | 怎么覆盖 |
| --- | --- | --- |
| A2 模板项目不能测试、运行 | P1 | 第 1–4 步（用仓库里的包；注册表安装是 #193） |
| A3 重新加载后旧代码仍在运行 / 旧实例没卸载 | P0 | 第 5、7 步：结果变化，旧实例清理恰好一次；Go：旧进程退出 |
| A4 改坏后 `dev` 崩溃或旧版本停止服务 | P1 | 第 6 步：旧版本继续服务且未被卸载，`dev` 继续运行 |
| A6 `check` 误判 | P1 | 第 3 步（模板项目能通过）；缺包由 `check.rs` 覆盖；`inject` 无人提供见 #258 |
| A7 错误信息看不出文件、行 | P1 | 第 6 步 |
| A8 Python venv 路径 | P1 | 第 5 步的解释器断言；Windows 部分在 #232 之后 |
| B2 Ctrl-C 后不清理、退出码不符 | P0 | 第 8 步（`dev` 下）；`run` 下由 `signals.rs` 覆盖 |

明确不覆盖：A1（#206）；A5 反复重新加载（#192）；B3、B10（`cross_language.rs`、#192）；B1、注册表安装（#193）；B4（`run` 不监视配置文件）。

## 十、分阶段与验收

| 阶段 | 内容 |
| --- | --- |
| E1 | harness 补充；`dev_loop.rs` 的 node、python 两个测试 |
| E2 | bun、go 两个测试 |
| 后续 PR | Windows（#232 之后） |

验收（都能自动检查）：

1. `cargo test -p rutis-e2e --test dev_loop` 在 Linux、macOS 上通过，四个测试都运行，都以 `finish()` 结束且没有残余。
2. 每一步的退出码与 3.2 一致。
3. 在 `RUTIS_LOCAL_HANDOVER=loopback` 下（Unix）同样通过。
4. 本机连续 5 次全部通过（记录在 PR 描述里）。
5. 场景代码里没有 `sleep`。

另在实现 PR 里手动做一次反向验证并记录：临时让 `dev` 在重新加载失败时卸载旧实例、或在 Ctrl-C 时不调用清理，确认场景失败。

## 十一、维护者的决定（2026-10-11）

- 第 2 步链接仓库里的包，不从注册表安装；注册表安装由 #193 负责。
- "改坏期间调用不失败"按"失败的重新加载之后，旧版本继续服务且没被卸载"来测；Go 项目的 probe 不被重载，顺带覆盖重载期间的调用。
- CI 改动由 #256 加；`check` 的 `inject` 检查另见 #258。
