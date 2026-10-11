# S9 安装即用：安装冒烟与文档示例（设计稿）

[English](design-e2e-install-2026-10-11.en.md)

状态：设计稿，待评审。日期：2026-10-11。基准：`main` `0941ef4`。
Issue：#193（[质量跟踪 #183](https://github.com/arcships/rutis/issues/183) 第一步；#195、#196 的验收）。吸收已关闭的 #180 中的安装冒烟、文档示例与 E7 项。
依据：[质量规范](quality-standard.md) Q6.15、Q2.13、Q10.2、Q12.1、Q13.1.6、Q13.2；[质量现状](quality-status.md) 风险 B1，控制手段 IN、DOC；[CI](ci.md)；黑盒框架 `tests/e2e/`（#221）。

范围外：模板项目的 `new`、`check`、测试与 `dev` 循环（S2 #186）；CI 测试任务的最低版本矩阵（#204）；`cargo xtask dev`（#260）；rutis-dsh 的分发（#89）；跨版本相遇（#209）。

## 一、结论

| # | 决定 | 节 |
| --- | --- | --- |
| D1 | 安装冒烟是 `tests/e2e` 里的一组场景，以"已安装"模式运行：宿主和运行时都来自装好的包，不注入仓库里的运行时 | 三 |
| D2 | 直接从构建出的文件安装：npm 装 tgz，PyPI 用 `--no-index --find-links`，二进制解压归档；不搭本地注册表 | 三 |
| D3 | 每个格子：安装 → 核对版本和位置 → 运行一个最小项目 → 停止 → 残余检查 | 四 |
| D4 | 构建步骤从 release.yml 抽成可复用 workflow，main 和发布共用一份 | 六 |
| D5 | 时机：main 上 3 个平台；发布前 5 个平台都通过才发布；发布后在 Linux 上用注册表上的包每种渠道再跑一次，加 crates.io 与 Go 模块 | 七 |
| D6 | 最低版本放在 Linux 格子：Node 22、Python 3.10（#196 合入前 3.12）、websockets 15.0 | 五 |
| D7 | 文档示例：只给能运行的示例加标记，main 上用装好的包运行；没加标记的代码块不检查 | 八 |
| D8 | E7 不再允许跳过 | 九 |

## 二、现状

| 已有 | 位置 | 做到 | 缺 |
| --- | --- | --- | --- |
| `release-dry-run` | `.github/workflows/ci.yml` | 每个 crate `cargo package`；npm 包 `npm pack --dry-run`；`uv build`；`maturin build` | 不安装、不运行；只在 Linux |
| `release-windows` | `ci.yml` | 构建 Windows 二进制并运行 `help`；打 zip；构建 wheel | 不安装 |
| `release-wheel-aarch64` | `ci.yml` | 交叉编译 aarch64 wheel | 不安装 |
| 平台包测试 | `node/rutis-host/test/platform.test.mjs` | 平台包的名字、`os`、`cpu`、版本 | — |
| 版本一致 | `scripts/train.mjs` | 源码里各包版本一致 | — |
| 发布前冒烟 | `docs/release.md` "冒烟" | 人工"用发布的包从零走一遍" | 人工，没有记录 |
| 黑盒框架 | `tests/e2e/`（#221） | `Scenario`、`Host`、probe、残余检查 | 总是注入仓库的运行时（`tests/e2e/src/lib.rs` 设 `RUTIS_NODE_RUNTIME` / `RUTIS_PYTHON_PATH`）；宿主来自 `cargo build` |
| E7 | `tools/test-sdk-bundle.sh` | toolchain pin 与 bundle 不一致时 `pack-plugin` 拒绝 | 机器上没有第二套 toolchain 时打印 "skipped" 并继续 |

读代码时发现、与本设计有关的问题：

1. websockets 下限：声明是 `websockets>=15`（`python/rutis/pyproject.toml`），CI 装 `websockets>=13`，实际装到最新版，15.0 从未运行过。CI 测试任务由 #204 改；本设计的 Linux PyPI 格子装 `==15.0`。
2. Python 指南 §4 的 `src/fake_llm.py` 没有内容：#259。
3. "PyPI 分发自带 Python 运行时"需要核实：宿主找解释器的顺序是 `runtimes.py.python` → `$VIRTUAL_ENV` → `<project>/.venv` → `python3`（`crates/rutis-host/src/host.rs`）。`uvx rutis-host run` 能否找到随包安装的 `rutis`，由 S9 的 uvx 步骤给出答案；不行时修代码或修文档。
4. 文档里在项目目录执行 `rutis-host run`，依赖 #226；#226 合入前这一步标为已知失败，指向 #226（Q7.3）。
5. 发布后验证不能用 `release` 事件触发：release.yml 用 `GITHUB_TOKEN` 创建 Release，不会触发其他 workflow。放在 release.yml 的最后一个任务里。

## 三、"已安装"模式与安装来源

`Scenario::installed(name, channel)`（`tests/e2e/src/install.rs`）：

- 场景目录是新的临时目录；设新的 `HOME`（Windows 另设 `USERPROFILE`、`APPDATA`、`LOCALAPPDATA`），`npm_config_cache`、`UV_CACHE_DIR` 指向场景目录。
- 不设 `RUTIS_NODE_RUNTIME`、`RUTIS_PYTHON_PATH`；去掉继承来的 `RUTIS_*`、`NODE_PATH`、`PYTHONPATH`、`VIRTUAL_ENV`。
- `PATH` 去掉 `~/.cargo/bin` 和仓库的 `target/`。
- 宿主命令由渠道给出：`npx rutis-host`、`uv run rutis-host`、`.venv/bin/rutis-host`、解压目录里的 `rutis-host`。

| 渠道 | 怎样装本次构建 | 怎样确认运行的是它 |
| --- | --- | --- |
| npm | `npm install --omit=optional <rutis-host tgz> <本平台的 rutis-host-<平台> tgz> <rutis-runtime tgz>`，三个 tgz 来自构建产物 | `rutis-host --version` 等于列车版本；解析出的二进制路径在场景目录的 `node_modules` 下 |
| PyPI | `uv venv`，`uv pip install --no-index --find-links <wheel 目录> rutis-host==<版本>`；Linux 另用 `python -m venv` + pip 跑一遍 | 同上；`rutis.__file__` 在 venv 内 |
| 二进制 | 解压构建出的归档（Windows `.zip`）；Node 行的 `@arcships/rutis-runtime` 从 tgz 装进项目，Python 行的 `rutis` 从 wheel 装进 venv | `--version`；二进制路径在解压目录下 |
| crates.io、Go 模块 | 只在发布后（七）。发布前由已有的 `cargo package` 覆盖 | `--version` / `go list -m` |

npm 从 tgz 安装时，`rutis-host` 的 `optionalDependencies` 里其他平台的包在注册表上还不存在：用 `--omit=optional` 加上显式给出本平台的 tgz。实现时先核实 npm 用传进来的 tgz 满足 `rutis-host` 对 `@arcships/rutis-runtime` 的依赖、不去注册表取；核实不通过再考虑本地注册表。

## 四、每个格子的步骤

| 步骤 | 做什么 | 断言 |
| --- | --- | --- |
| 1 安装 | 按三 | 退出码 0 |
| 2 身份 | `宿主命令 --version`；宿主二进制的实际路径 | 列车版本；路径在场景目录下 |
| 3 最小项目 | 场景里内置的固定项目：一个 `greeter` 行加一个 probe 行。npm：TS 行；PyPI：`py:` 行；二进制：TS 和 Python 两行，Linux 上另加一个 Go 行 | `run`（在项目目录里，不带参数，即文档的写法）后 probe 调 `greeter.hello("Ada")` 得到结果；二进制格子里 TS 调 Python 得到结果 |
| 4 停止 | 只给主进程发 SIGTERM：npm 是 Node 启动器，uv 是 `uv` 进程，二进制是 `rutis-host` | 退出码 0；probe `stopped`；npm 启动器、`uv` 进程也已退出；残余检查通过（`tests/e2e/src/residue.rs`） |

另加几个只在某个渠道上的步骤：

| 渠道 | 步骤 | 断言 |
| --- | --- | --- |
| npm | 不装 `@arcships/rutis-runtime`，运行 Node 行 | 行照常运行：运行时来自 `@arcships/rutis-host` 的依赖 |
| npm | 不装本平台的 tgz | 退出码 1，输出 `no binary for <平台>` |
| npm（Linux） | 装 `@arcships/rutis-bun` 的 tgz，加一个 `bun:` 行，Bun 1.4.0 | probe 调用得到结果 |
| PyPI | `uvx rutis-host@<版本> --version`（`--find-links`）；一个没有 `.venv` 的 `py:` 项目里 `uvx … run` | 第一条输出列车版本；第二条见二-3 |
| PyPI（Linux） | `websockets==15.0`；`python -m rutis listen:ws://127.0.0.1:<端口>/rutis …`，宿主用节点行连上 | probe 调用得到结果 |
| 二进制 | 项目里没有 `@arcships/rutis-runtime`，运行 Node 行 | 退出码 1，输出 `npm install @arcships/rutis-runtime` |

Windows 上第 4 步暂用 kill，残余检查的进程一项按 #232 报告为跳过（框架已有此行为）。

## 五、平台与版本

| 平台 | main | 发布前 | 版本 |
| --- | --- | --- | --- |
| linux-x64（`ubuntu-24.04`） | 是 | 是 | 最低：Node 22、Python 3.10（#196 合入前 3.12）、websockets 15.0、Bun 1.4.0、Go 1.24 |
| darwin-arm64（`macos-15`） | 是 | 是 | 最新：Node 26、Python 最新 |
| win32-x64（`windows-2025`） | 是 | 是 | Node 24、Python 3.12 |
| linux-arm64 | — | 是 | 同 linux-x64 |
| darwin-x64 | — | 是 | 同 darwin-arm64 |

三种渠道在同一台机器上依次运行，每种渠道一个新的场景目录：每个平台一个任务。

不进：musl Linux（Alpine）× PyPI，没有 musllinux wheel，文档写明 PyPI 分发只支持 glibc Linux；pnpm、yarn、全局安装、升级、代理与离线安装，文档没有写这些方式。

#195 的安装验收由 linux-x64 的 npm 格子给出；#196 的由 linux-x64 的 PyPI 格子在 #196 合入后给出。

## 六、产物从哪里来

```text
package.yml（可复用，release.yml 与 main 共用）
  ├─ build-<目标>：release.yml 现有的 binaries / wheels 步骤 → 归档、wheel、平台包目录
  └─ pack：npm pack 各包和平台包；uv build python/rutis
         │  upload-artifact
         ▼
install.yml（可复用；输入：来源 local | registry，版本）
  └─ install-<平台>：checkout（编译场景程序），下载产物，
       RUTIS_E2E_ARTIFACTS=<目录> cargo test -p rutis-e2e --test install -- --ignored
       失败时上传场景目录
```

- 构建步骤只有一份：release.yml 的 `binaries`、`wheels` 移进 `package.yml`，release.yml 调用它。main 上 `package.yml` 取代 ci.yml 的 `release-windows`、`release-wheel-aarch64`（这两项只构建、不验证）。`release-dry-run` 留在 PR 的 `packaging` 开关下。
- 场景在 `tests/e2e/tests/install.rs`，标 `#[ignore = "needs this build's packages: run by install.yml with RUTIS_E2E_ARTIFACTS"]`，`cargo test --workspace` 不运行它们（Q7.8）。
- 场景在仓库外的临时目录运行，"已安装"模式不注入仓库的运行时；二进制路径的断言保证用的不是仓库里的宿主。

## 七、运行时机

| 时机 | 内容 | 时间 |
| --- | --- | --- |
| PR | 不增加任务 | 不变 |
| main | `package`（3 个平台）+ `install`（3 个平台）+ 文档示例（Linux） | 约 20 分钟，在 main 的 60 分钟预算内 |
| 发布前 | release.yml：`verify` → `package`（5 个目标）→ `install`（来源 local，5 个平台）→ 各 publish 任务 | 发布前增加约 10 分钟；5 个平台都通过才发布 |
| 发布后 | release.yml 最后一个任务：Linux 上用注册表上的包跑 npm、PyPI、二进制三种渠道，加 `cargo install rutis-host@<版本>` 和 `go list -m`；先轮询注册表直到取得该版本（每 15 秒一次，最多 15 分钟，防挂死）；失败时自动开 issue，标题含版本 | 约 10 分钟 |
| 手动 | `install.yml` 支持 `workflow_dispatch`，输入版本与来源 | — |

发布后验证失败时，撤回或标记（`npm deprecate`、PyPI yank）由维护者决定，步骤写进 `docs/release.md`（Q13.2）。

## 八、文档示例

只给能照着运行的示例加标记（GitHub 上不显示）：

| 标记 | 含义 |
| --- | --- |
| `<!-- example: <名字> file=<路径> -->` | 把下面的代码块写到示例 `<名字>` 的 `<路径>` |
| `<!-- example: <名字> run -->` | 按顺序执行下面 shell 代码块的每一行 |
| `<!-- example: <名字> run until="<文本>" -->` | 长时间运行的命令（`dev`、`run`）：等输出出现 `<文本>`，发 SIGINT，断言退出码 0 |

`tools/doc-examples.mjs --out <目录>` 抽取出示例项目和步骤；`tests/e2e/tests/doc_examples.rs` 在 main 的 Linux install 任务里，用装好的包执行它们。没有标记的代码块不检查。

第一批标记：`rutis-host.md` 的快速开始、`typescript-plugin.md`、`python-plugin.md`（#259 修好后）各一个完整示例，中英文都标。以后新写的可运行示例顺手加标记。

## 九、E7

`pack-plugin` 在编译前比较插件工作区与 bundle 的 `rust-toolchain.toml` channel，是纯字符串比较，不需要第二套 toolchain。改法：测试里把工作区 pin 写成不存在的 `0.0.0-e7`，设 `RUSTUP_AUTO_INSTALL=0`，断言完整的拒绝信息（含两个 pin），且没有出现 rustup 安装；去掉 "skipped" 分支。不下载，结果确定。

## 十、覆盖的风险

| 风险 / 条款 | 怎样覆盖 | 时机 |
| --- | --- | --- |
| **B1**（P0） | 三种渠道 × 3 个平台，本次构建的产物；其余 2 个平台在发布前；发布后用注册表上的包 | main、发布前、发布后 |
| Q6.15.1 | 安装 → 最小项目 → 停止，新目录与新的用户目录 | 同上 |
| Q6.15.3、Q13.2 | 发布后验证，失败开 issue | 发布后 |
| Q13.1.6 | 发布前 5 个平台 | 发布前 |
| B6，部分 | 缺平台包、缺 Node 运行时包的错误信息 | main |
| B2，部分 | npm 启动器、`uv run` 收到 SIGTERM 后清理并退出 | main（Unix） |
| MX，部分 | 安装场景下的 Node 22、Python 3.10、websockets 15.0、Bun 1.4.0 | main |
| Q2.13 | 标记过的文档示例运行 | main |
| I5，部分 | E7 不再跳过 | 改 dylib 的 PR |

不覆盖：macOS 浏览器下载的隔离属性；musl Linux；pnpm、yarn、全局安装；升级与多版本并存（#209）；公司代理、离线安装；Windows 的进程残余与 Ctrl-C（#232）；最小项目之外的功能正确性。

## 十一、新增与修改的文件

| 文件 | 内容 |
| --- | --- |
| `tests/e2e/src/install.rs` | 渠道、"已安装"模式的环境、身份核对、一次性命令 |
| `tests/e2e/tests/install.rs`、`tests/e2e/fixtures/install/` | 场景与最小项目 |
| `tests/e2e/tests/doc_examples.rs`、`tools/doc-examples.mjs` | 文档示例 |
| `docs/guide/*.md`（中英） | 第一批标记 |
| `tools/test-sdk-bundle.sh` | E7 去掉跳过 |
| `.github/workflows/package.yml`、`install.yml`、`release.yml` | 可复用的构建与安装；发布前与发布后 |
| `docs/release.md`（中英） | 发布前、发布后验证失败时的处理 |

`ci.yml` 里去掉 `release-windows`、`release-wheel-aarch64`、main 上调用 `package.yml` 和 `install.yml`，写在 PR 描述里，由 #204（#256）一起改。

## 十二、分阶段（都在本 PR 内，按提交分开）

1. "已安装"模式；三种渠道的场景；`package.yml`、`install.yml`；main 上 3 个平台；E7。
2. 发布前与发布后；`docs/release.md`。
3. 文档示例与第一批标记。

## 十三、验收（都可以自动检查）

1. main 上 `install` 的每个平台通过；日志里有语言环境版本、`rutis-host <列车版本>` 和宿主二进制的路径。
2. 检查能发现问题：去掉 `rutis` 包的 wheel → PyPI 格子失败；不装本平台 tgz → npm 格子在"缺平台包"一步得到预期错误。
3. 每个格子的残余报告为空（Windows 的进程一项按 #232 报告为跳过）。
4. release.yml 中每个 publish 任务都 `needs` 发布前的 install 任务。
5. 发布后验证自动运行；用 `workflow_dispatch` 指定不存在的版本时，自动开出 issue。
6. 标记过的文档示例在 main 上运行通过。
7. `tools/test-sdk-bundle.sh` 中 E7 没有跳过分支。
8. 普通 PR 的关键路径不变；main 上 `package` + `install` ≤ 25 分钟。

## 十四、维护者的决定（2026-10-11）

- Linux 最低版本格子的 Node 用 22 的最新补丁（`setup-node` 写 `22`）。
- PyPI 在 musl Linux：只在文档里写明 PyPI 分发支持 glibc Linux，不加 musllinux wheel。
- workflow（`package.yml`、`install.yml`、`release.yml`）在本 PR 里改，`ci.yml` 的改动由 #256 做；发布后验证失败时自动开 issue，撤回由人决定；`xtask dev` 的 SDK_ID 测试移到 #260。
