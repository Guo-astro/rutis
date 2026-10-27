# 插件安装：`rutis-host add` / `remove` / `upgrade` 与嵌入 API

[English](design-plugin-install-2026-10-27.en.md)

状态：草案，未实现。日期：2026-10-27。基准：`main` `103c0d22`（0.17.2 之后）。排期：0.19。
Issue：[#691](https://github.com/arcships/rutis/issues/691)（属于 [#663](https://github.com/arcships/rutis/issues/663) 的 H2、O4）。
与 0.18 设计的关系：`add` 打印的插件介绍就是 `check` 已有的描述（版本、`inject`、`provides`、配置 Schema），不另起一份；新写的行不带 `config`，默认值按 [0.18 配置 Schema](design-config-schema-2026-10-27.md) C1 由 SDK 补，缺必填项时 `add` 用 `check` 的同一段检查说出来；出错文字遵守 [0.18 诊断](design-diagnostics-2026-10-26.md) §4.5。
兼容：全部兼容，不进《迁移到 0.19》。旧的行名写法（`py:`、`bun:`、`go:`、`swift:` 前缀，npm 包名）和手动安装照常可用，不转换。

范围外：沙箱或审核安装脚本（`add` 运行包管理器，和直接运行 npm / uv / go 一样，见 §十）；安装语言工具链本身（Node、Bun、Python、Go）；版本范围（`^1.2`、`>=0.3`）；pnpm、yarn、私有源的配置（用包管理器自己的配置文件与环境变量）；从部署机器上按记录重装全部插件；运行中热升级（§七）；远程运行时与应用里的插件（Kotlin）；从网址下载 Swift / Go 二进制；`add` 时指定行 id 或配置的参数；JSON 格式的输出（嵌入 API 返回结构，§九）。

## 一、场景：现在卡在哪

deskmate 的宿主持有者要用两个别人发布的插件：npm 上的 TypeScript 插件 `@ana/wordcount`，PyPI 上的 Python 插件 `bo-llm`（入口点 `llm`）。现在要做（[rutis-host 指南](guide/rutis-host.md)「快速上手」）：

```bash
npm init -y && npm install @ana/wordcount                          # 先知道它是 TypeScript
uv venv && uv pip install --python .venv/bin/python "rutis==0.19.0" bo-llm   # 先知道它是 Python，还要自己建环境、钉运行时版本
# rutis.json：runtimes 写 node、py；行名一个写包名、一个写 py: 加入口点名（不是包名）
```

#663 的评估里，宿主作者花时间最多的就是"每种语言的插件装在哪、行名怎么写"；deskmate 这样嵌入 rutis 的宿主，发现、安装、依赖处理只能自己写（`deskmate install <dir>`、`deskmate setup`）。

要做到：

```bash
rutis-host add npm:@ana/wordcount
rutis-host add pypi:bo-llm
rutis-host run
```

宿主持有者只说插件从哪里来（他在 npm 还是 PyPI 上找到它），不需要知道语言、包管理器、环境和行名前缀。嵌入的宿主用同一套代码做自己的「装插件」。

## 二、已有的东西

| 已有 | 位置 | 本稿怎样用 |
| --- | --- | --- |
| 每种语言一个 `Language`：运行时的键（`node`、`bun`、`py`、`go`、`swift`）、默认设置（`enable_default`）、按项目文件认语言（`of_project`） | `crates/rutis-host/src/languages/` | 本地目录的语言由 `of_project` 判断；记录里的语言就是运行时的键 |
| 隐式运行时：行用到、`runtimes` 没写的运行时按默认启用（#666） | `HostConfig::enable_used_runtimes` | 记录里的语言同样按默认启用 |
| Bun 项目的判断：依赖 `@arcships/rutis-bun` | `languages/bun.rs` `runs_in_bun` | npm 包是 Bun 还是 Node 插件，用同一条规则 |
| `rutis-host go add`：`GOBIN=<dir> go install` | `crates/rutis-host/src/main.rs` | 移进安装代码，`go add` 保留 |
| 运行时描述一行：版本、`inject`、`provides`、`announces`、配置 Schema（`RowSchema`） | `RuntimeResolver` → `describe_row`（`crates/rutis-loader/src/runtime.rs`） | 插件介绍，`check` 已经打印它 |
| Go、Swift 二进制的清单（插件名、版本、SDK 标记） | `crates/rutis-loader/src/runtime/binary.rs` | 找出二进制里的插件；认出本地二进制 |
| Python 插件的入口点组 `rutis.plugins`（模板已写） | `templates/python/pyproject.toml` | 找出一个 Python 包里的插件 |
| Python 运行时缺环境、缺 `rutis` 时给出的命令 | `languages/python.rs` | `add` 直接执行这些命令 |

Cordis 4 本身没有安装命令；Koishi 的插件市场用 npm 装进项目、再写配置，和本稿是同一个做法，本稿只是把它推到其他语言。Cordis 行为不变。

## 三、决策

| # | 决策 | 节 |
| --- | --- | --- |
| I1 | `rutis-host add <引用> [rutis.json]`、`remove <插件或引用>`、`upgrade [<插件或引用>[@版本]]`；`add --dry-run` 装前看介绍；`go add` 保留 | 五、七、八 |
| I2 | 插件引用写来源，不写语言：`npm:`、`pypi:` 前缀；Go 模块路径与本地路径本身说明来源；版本统一写 `@版本`；不带来源的名字报错 | 四 |
| I3 | 语言由包自己决定：npm 包依赖 `@arcships/rutis-bun` 是 Bun，否则 Node；PyPI 是 Python；Go 模块是 Go；本地目录按项目文件；本地可执行文件按 SDK 标记（Go、Swift） | 四 |
| I4 | 装到现在就用的位置：`runtimes.<语言>` 写了就用它指的，没写就用它的默认；Go、Swift 默认 `plugins/go`、`plugins/swift` | 五 |
| I5 | 运行时缺了就准备好：Node 项目与 `@arcships/rutis-runtime`、Bun 项目与 `@arcships/rutis-bun`、Python 环境与 `rutis`、Go / Swift 目录；不装 Node、Bun、Python、Go 本身 | 五 |
| I6 | 安装记录写在 `rutis.json` 顶层的 `installed`，以插件名为键；行里只写插件名 | 六 |
| I7 | 记录里的语言按默认启用它的运行时，不告警；Go、Swift 用记录里的二进制 | 六 |
| I8 | 插件介绍用插件已有的声明（运行时对它的描述，即 `check` 打印的），不新增清单格式；`provides` 仍可不写 | 八 |
| I9 | 嵌入 API 在 `rutis-loader` 的 `install` feature（默认关）：`Installer`；它返回的每条记录与 `rutis.json` 里 `installed` 的一条是同一个可序列化类型，rutis-host 原样写入、不转换；记录由调用方保存；`rutis` 内核不变 | 九 |
| I10 | 全有或全无：任何一步失败，`rutis.json` 不变，刚装上的包卸掉 | 五 |
| I11 | 运行中的宿主按现有的改配置规则接上新行（`run --watch-config`、`dev`）；`upgrade` 不热换代码 | 七 |

## 四、插件引用

| 写法 | 来源 | 语言 |
| --- | --- | --- |
| `npm:@ana/wordcount`、`npm:@ana/wordcount@1.2.0` | npm | 包的 `dependencies` 里有 `@arcships/rutis-bun` 是 Bun，否则 Node（装之前用 `npm view <包>@<版本> dependencies --json` 读） |
| `pypi:bo-llm`、`pypi:bo-llm@0.3.0` | PyPI | Python |
| `github.com/bo/netkit/cmd/netkit`、`…@v0.4.1` | Go 模块（经 Go 模块代理） | Go |
| `./plugins/notes`（目录） | 本地 | 按项目文件：`package.json`（同上分 Bun / Node）、`pyproject.toml`、`go.mod`（已有的 `of_project`） |
| `./wordcount-1.2.0.tgz`、`./bo_llm-0.3.0-py3-none-any.whl` | 本地包 | `.tgz` 读包里的 `package.json`；`.whl` 是 Python |
| `./netkit`（可执行文件） | 本地二进制 | 含 Go SDK 标记是 Go，含 Swift SDK 标记是 Swift（已有的清单读取） |

- **版本**：最后一个不在开头的 `@` 之后是版本（`@ana/wordcount@1.2.0`）；不写是最新版。版本是一个确切的版本或标签（npm 的 `latest`、Go 的 `latest`），交给包管理器时换成它的写法（PyPI 写成 `bo-llm==0.3.0`）。
- **本地路径**以 `./`、`../`、`/` 开头（Windows 上另有盘符），与行里写插件文件的规则相同；相对 `rutis.json` 所在目录。
- **Go 模块路径**按 Go 自己的规则认：第一段含 `.`（`github.com`、`example.org`）。`go:` 不用作来源前缀，它是旧的行名前缀，意思是语言。
- **不带来源的名字**（`wordcount`）报错，写明可选的写法。同一个名字在 npm 和 PyPI 上可能都有，猜错就会装上别人的包。

为什么写来源、不写语言：来源是宿主持有者知道的事（他在哪个页面找到这个插件），语言是包的事。同一个来源可能是两种语言（npm 上的包可以是 Node 或 Bun 插件），所以语言要从包里读，不让人写。

## 五、`add` 做什么

1. 解析引用，得出来源（§四）。
2. 定语言（§四）。
3. 准备运行时，装包：

| 语言 | 装到哪里 | 运行时缺了时 | 装包 |
| --- | --- | --- | --- |
| Node | `runtimes.node.project`（默认 `rutis.json` 所在目录） | 没有 `package.json` 时写一个 `{ "private": true }`；项目里没有 `@arcships/rutis-runtime`、宿主也没有自带一份（npm 版 rutis-host 自带）时一起装 `@arcships/rutis-runtime@<宿主版本>` | `npm install <包>[@版本]`（本地目录 npm 装成链接） |
| Bun | `runtimes.bun.project`（默认同上） | 同上，装 `@arcships/rutis-bun@<宿主版本>` | `bun add <包>[@版本]` |
| Python | 宿主运行时会用的解释器：`runtimes.py.python`，否则 `$VIRTUAL_ENV`，否则 `<runtimes.py.project>/.venv` | 都没有时建 `<project>/.venv`（`uv venv`，没有 uv 时 `python3 -m venv`），不往系统的 Python 里装 | `uv pip install --python <解释器> <包>`（没有 uv 时 `<解释器> -m pip install`），同一条命令里带上 `rutis==<版本>`：环境里没有 `rutis` 时是宿主的版本，有时是它现在的版本，装插件不改变运行时的版本。本地目录用 `-e` 安装，改了代码不用重装 |
| Go | `runtimes.go.dir`，没写 `runtimes.go` 时 `plugins/go` | 建目录 | `GOBIN=<目录> go install <模块>@<版本>`（不写版本时 `@latest`）；本地目录在它里面 `go install ./...`；本地二进制复制进目录 |
| Swift | `runtimes.swift.dir`，没写时 `plugins/swift` | 建目录 | 只有本地二进制：复制进目录 |

   `runtimes.go`（`swift`）写了但没有 `dir` 时报错：`add` 不知道装到哪里。包管理器的输出原样显示。
4. 找出包里的插件：Node、Bun 是包名（一个包一个插件）；Python 是这个发行包在 `rutis.plugins` 组里的入口点（一个包可以有几个）；Go、Swift 是二进制清单里的插件。
5. 描述每个插件：启动它的运行时，和 `check` 一样描述这一行（§八），打印出来。
6. 写文件：`installed` 加上每个插件的记录，`rows` 末尾每个插件加一行 `{ "id": <id>, "name": <插件名> }`。id 由插件名得出（已有的 `id_of`：`@ana/wordcount` → `wordcount`），已被别的行占用时加 `-2`、`-3`。同名的行已经有了时不再加。按原有键的顺序写回，缩进两格，先写临时文件再改名（`--watch-config` 不会读到写了一半的文件）。
7. 用 `check` 的同一段检查看新加的行，打印它的结果：缺它要用的服务、配置缺必填项（0.18）时这里就看得到。`add` 仍以 0 退出：行已经写好，接下来补上依赖或配置就行。

写哪个文件：第二个参数；没有时当前目录的 `rutis.json`；没有 `rutis.json`、有 `rutis.dev.json`（插件项目）时写它；都没有时新建 `rutis.json`（`{ "format": 1, "installed": …, "rows": […] }`）。

第 2 至 6 步任何一步失败：文件不变；第 3 步刚装上的包卸掉（装之前就已经在的包不动），以 1 退出。

## 六、安装记录与行

```json
{
  "format": 1,
  "installed": {
    "@ana/wordcount": { "from": "npm:@ana/wordcount", "version": "1.2.0", "language": "node" },
    "llm":            { "from": "pypi:bo-llm", "version": "0.3.0", "language": "py" },
    "dns":            { "from": "github.com/bo/netkit/cmd/netkit", "version": "v0.4.1", "language": "go", "binary": "plugins/go/netkit" }
  },
  "rows": [
    { "id": "wordcount", "name": "@ana/wordcount" },
    { "id": "llm", "name": "llm" },
    { "id": "dns", "name": "dns" }
  ]
}
```

- 键是插件名，也就是行的 `name`。`from` 是不带版本的引用，`version` 是装上的确切版本，`language` 是运行时的键，Go、Swift 另有 `binary`（相对文件所在目录）。同一个包的几个插件，`from` 相同。
- 为什么放在 `rutis.json` 里而不是旁边另一个文件：少一个文件、少一个概念；宿主持有者在同一处看到装了什么、运行什么；部署时本来就要带着 `rutis.json`。它由 `add` 写，人也能读能改。
- 行的名字是记录里的键时，rutis-host 按记录把它交给那门语言的解析：Node 原样，Bun `bun:<包>`，Python `py:<入口点>`，Go `go-<二进制>:<插件>`，Swift `swift-<二进制>:<插件>`。输出里仍写行里写的名字。不是记录里的键的名字，和现在一样解析。
- **运行时推出**（I7）：按记录的键写名字（不带前缀）的行，用到的语言在 `runtimes` 没写时按默认启用，和 #666 一样，但不告警（记录已经说明了）；`runtimes` 写了的照它。Go、Swift 没写时，运行时就是记录里的那些二进制（`binaries`），不把整个目录当作受信任。
- **名字冲突**：插件名已经是另一个包的记录时，`add` 拒绝，写明是哪个包。旧写法的行（`py:llm`）与记录无关，不算冲突。
- 旧版 rutis-host 读到带 `installed` 的文件，按现有规则报"不认识的字段"并列出可用字段。`format` 仍是 1：`installed` 不改变其他字段的含义。

## 七、`remove` 与 `upgrade`

- `rutis-host remove <插件名或引用> [rutis.json]`：卸掉这个插件所在的包（`npm uninstall`、`bun remove`、`uv pip uninstall`、删掉二进制），删掉这个包的全部记录和用这些插件名的行（包括分组里的），逐个打印删掉的行。一个包有几个插件时，按其中一个插件名删也删整个包，并说明。名字不在记录里时拒绝，写明它不是 `add` 装的、该用哪个包管理器自己卸。
- `rutis-host upgrade [<插件名或引用>[@版本]] [rutis.json]`：不写名字时升级全部记录。按记录的来源装最新版（或给出的版本），改记录里的 `version`；本地引用从同一路径重装。行不改。新版本多出的插件只打印一句如何加行（`add` 同一个引用即可）；少了的插件，它的记录删掉，用它的行打印一句"新版本里没有这个插件"。
- **运行中的宿主**（I11）：`upgrade` 只换磁盘上的包。正在运行的宿主继续运行旧代码，直到重启；`upgrade` 结束时说明这一点。不做热换的原因：重新加载一行只重新导入它自己的入口模块（`dev` 的已有规则），升级换掉的是整个包，半新半旧比不换更难查。

## 八、装之前看介绍

插件介绍就是插件已经写下的声明：TypeScript `definePlugin({ inject, provides, config })`、Python `define_plugin(...)`、Go 的清单与 `rutis.Plugin`。运行时导入插件、描述这一行（`RowSchema`），`check` 已经把它打印成 `version`、`inject`、`provides`、`config` 几行。

`rutis-host add --dry-run <引用>` 把包装进一个临时目录（临时的 Node 项目、Python 环境、Go 目录，和第 3 步相同的命令），描述包里的每个插件，打印介绍，删掉临时目录；不改 `rutis.json`，也不碰宿主的项目和环境。

- 不新增清单格式：现有的声明四种语言都有、形式相同，`check` 已在用；另起一份清单，插件作者就要多写一份 rutis 特有的东西，还会和代码里的声明不一致。
- 不读注册表的元数据（`npm view`、PyPI 的 JSON）：TypeScript 叶子插件、Python、Go 的声明都在代码里，包元数据里没有；只有 Cordis 插件的 `package.json` 有 `rutis.provides`。
- `provides` 仍可不写（2026-10-18 的决定：提供的服务自动暴露）。没写时介绍里是 `provides: none declared (it may provide more once it runs)`。
- 代价：`--dry-run` 要下载并导入插件，和 `add` 一样会运行包里的代码（§十）。

## 九、嵌入 API

`rutis-loader` 加 `install` feature（默认关），和语言 feature（`node`、`python`、`bun`、`go`、`swift`）一起决定能装哪些语言：

```rust
use rutis_loader::install::{Installer, Places, Reference};

let installer = Installer::new(Places {
    node: Some(home.join("plugins/node")),   // Node 项目
    python: Some(home.join("plugins/.venv")), // Python 环境（没有就建）
    go: Some(home.join("plugins/go")),       // 二进制目录
    ..Places::default()
});
let reference: Reference = "npm:@ana/wordcount".parse()?;
let about = installer.describe(&reference).await?;   // 装之前看：临时目录里装、描述、删掉
let installed = installer.add(&reference).await?;    // Vec<InstalledPlugin>：每个插件一条
for plugin in &installed {
    rows.push(json!({ "id": plugin.id, "name": plugin.row_name }));   // loader 的行名（py:llm 等）
    records.insert(plugin.name.clone(), plugin.record.clone());      // Record：就是 installed 里的一条
}
installer.upgrade(&records["llm"], None).await?;     // 按记录升级，返回新的记录
installer.remove(&records["llm"]).await?;            // 按记录卸掉整个包
```

- **放在 rutis-loader**：嵌入的宿主本来就依赖它和它的语言 feature；描述插件要用的运行时也在这里。`rutis` 内核不变，不知道任何包管理器；`rutis-loader` 也只在调用时把 npm、uv、go 当作外部命令运行，不在构建时依赖它们。rutis-host 的库不是公开 API（`lib.rs` 写明无 semver 承诺），不放那里；也不为它另开一个 crate。
- **记录是同一个形状**：`Record`（`from`、`version`、`language`，Go、Swift 另有 `binary`）实现 serde 的序列化与反序列化，就是 §六 `installed` 里键（插件名）后面的那个对象。rutis-host 把 `plugin.name → plugin.record` 原样写进 `installed`，读回来原样交给 `remove` / `upgrade`，中间不转换；rutis-host 读 `rutis.json` 用的也是这个类型。
- **记录归调用方**：调用方存在自己想存的地方（rutis-host 存进 `installed`，deskmate 存进自己的插件列表）。API 不读写任何配置文件，也不碰 loader 与运行时。
- **行名**：返回 loader 认得的行名（`py:llm`、`bun:@ana/x`、`go-netkit:dns`）。嵌入的宿主用程序写行，不需要无前缀的名字；那是 `rutis.json` 给人写的便利。
- `describe` 返回已有的 `RowSchema`，不另定类型。
- rutis-host 的 `add` / `remove` / `upgrade` 与 `go add` 都调用它，安装逻辑只有一份。

## 十、与其他设计的关系、安全

- **#185**：#185 的 `loader` 服务不安装包；从包仓库安装归本设计（命令行与嵌入 `Installer`）。
- **与 `dev`（#670）**：`add` 在插件项目里写 `rutis.dev.json`，`dev` 照现有规则重新读它：新行 `added` 并启动，别的行不动，共享运行时不重启。新行要的运行时还没在运行时（TypeScript 项目第一次加 Python 插件），和现在改配置加运行时一样被拒绝、提示重启，文字改为说出是哪个运行时：`reload rejected: rows now use the py runtime, which is not running: restart the host; nothing changed`。`run --watch-config` 相同。
- **#683（管住插件）**：各插件在哪个进程运行、能用哪些服务，是 #683 的行选项，与怎样装无关。
- **#698（远程运行时）**：只装到本机运行时；远程运行时上的插件由那一端自己装。
- **安全**：`add`、`upgrade`、`--dry-run` 运行包管理器，可能执行包里的安装脚本，并导入插件代码来描述它；这与宿主持有者直接运行 `npm install`、`uv pip install`、`go install` 相同。rutis 不另做沙箱或审核，指南写明：只装信任的插件。

## 十一、开发者看到什么

```text
$ rutis-host add npm:@ana/wordcount
(npm 的输出)
installed @ana/wordcount 1.2.0 (node) into /app
wordcount (@ana/wordcount): cannot run: nothing provides notes
  version: "1.2.0"
  inject: ["notes"]
  provides: {"tools":{"register":"async"}}
  config: {"type":"object","properties":{"limit":{"type":"integer","default":100}}}
rutis.json: added row wordcount

$ rutis-host add pypi:bo-llm
created the Python environment /app/.venv
(uv 的输出)
installed bo-llm 0.3.0 (py) into /app/.venv: plugin llm
llm (llm): ok
  version: "0.3.0"
  provides: none declared (it may provide more once it runs)
rutis.json: added row llm

$ rutis-host add --dry-run github.com/bo/netkit/cmd/netkit
(go 的输出)
github.com/bo/netkit/cmd/netkit v0.4.1 (go): plugin dns
dns
  version: "0.4.1"
  provides: {"dns":{"lookup":"async"}}
nothing was installed (--dry-run)

$ rutis-host remove llm
uninstalled bo-llm from /app/.venv
rutis.json: removed row llm

$ rutis-host upgrade
@ana/wordcount: 1.2.0 → 1.3.0
bo-llm: 0.3.0 is the latest
a running host keeps the old versions until it restarts
```

出错时（stderr，以 1 退出，文件不变）：

| 情况 | 输出 |
| --- | --- |
| 没写来源 | `rutis-host: wordcount: say where it comes from: npm:wordcount, pypi:wordcount, a Go module path or a local path` |
| 包管理器不在 | `rutis-host: cannot run npm (No such file or directory): install Node.js 22 or later`；Python 没有 uv 也没有 `python3`：`cannot create the Python environment /app/.venv: install uv, or Python 3.10 or later`；Go 沿用现有文字 |
| 包管理器失败 | 它自己的输出，然后 `rutis-host: npm install @ana/wordcount failed (exit status 1); rutis.json is unchanged` |
| 不是插件 | `rutis-host: bo-utils has no rutis.plugins entry point: it is not a plugin, or its author did not register one; uninstalled it again`；Node 包导入后不是插件时用 0.18 的文字（默认导出不是 `definePlugin` 的结果），后面同样加 `; uninstalled it again` |
| 本地目录认不出 | `rutis-host: ./plugins/notes has no package.json, pyproject.toml or go.mod` |
| 名字已被别的包占用 | `rutis-host: llm is already installed from pypi:other-llm: remove it first; uninstalled bo-llm again` |
| Go 目录不明 | `rutis-host: runtimes.go has no dir to install into: add one, or remove runtimes.go to use plugins/go` |
| `remove` 的名字不在记录里 | `rutis-host: llm was not installed by add: uninstall it with the package manager that installed it, and remove its row` |
| 运行中加了没在运行的运行时 | `rutis-host: reload rejected: rows now use the py runtime, which is not running: restart the host; nothing changed` |

## 十二、兼容与迁移

- 不破坏任何现有写法：没有 `installed` 的 `rutis.json` 行为完全不变；`py:`、`bun:`、`go:`、`swift:` 行与 npm 包名行照常解析；手动装的插件照常运行；`rutis-host go add` 保留。不转换旧文件，不告警。
- 唯一的文字变化是 §十 那一条拒绝原因（只在新增运行时被拒绝时）。
- 不进《迁移到 0.19》。指南里「宿主怎样装插件」改为先写 `add`，手动安装表保留为"不用 `add` 时"。

## 十三、实施

| 步骤 | 内容 | 完成条件 |
| --- | --- | --- |
| P1 | `rutis-loader` 的 `install` feature：`Reference` 解析、`Places`、`Installer` 的 `add` / `remove` / `upgrade` / `describe`、失败回滚；`go add` 改为调用它 | 单元测试：§四 每种写法解析出的来源、语言与包管理器命令（`PATH` 上放记录参数的假命令）；失败时卸掉刚装的包、不动原来就在的包 |
| P2 | rutis-host：`installed` 字段、按记录解析行名、记录推出运行时、`add`（含 `--dry-run`）/ `remove` / `upgrade`、写回文件、§十 的拒绝文字 | `crates/rutis-host/tests/install.rs`：记录 → 运行时与行名；旧写法与记录并存；写回保持键顺序；§十一 的每条出错输出逐字比较 |
| P3 | 端到端：`tests/e2e/tests/plugin_install.rs`（与 `dev_loop.rs` 同在 ci.yml 的 e2e 任务里） | Node、Bun、Python、Go 各一个场景：用 `rutis-host new` 的模板打出本地包（`npm pack`、`uv build` 的 wheel、Go 模块目录）→ `add --dry-run` → `add` → `check` → `run`，probe 调到它的服务 → 改版本后 `upgrade` → `remove`，残余检查通过；Python 场景从没有环境开始（`add` 建环境）；`pypi:` 与 Go 模块路径的写法用 uv 的本地 `--find-links` 与 `GOPROXY=file://` 跑一遍；一个 `dev` 场景：`add` 写进 `rutis.dev.json` 后新行 `added`，项目自己的行不重启。安装冒烟（`tests/e2e/tests/install.rs`）的二进制渠道加一步：没有 `.venv` 的目录里 `add` 一个 wheel，环境里的 `rutis` 来自本次构建 |
| P4 | 文档 | [rutis-host 指南](guide/rutis-host.md)（命令表、「快速上手」、「宿主怎样装插件」、`installed` 字段）、宿主持有者指南（#671，原 #685）、[在 Rust 应用里嵌入](guide/rust-host.md) 的 `Installer` 一节，中英两份同步；写明安装脚本与信任（§十） |

P1 → P2 → P3；P4 随 P2 一起。对应 issue 的「完成」：命令与 API 及四种语言的端到端测试是 P1–P3，两份指南是 P4。
