# Go 插件运行时（设计稿）

[English](design-go-runtime-2026-10-10.en.md)

状态：G1、G2 已实现（#227，出入见 §十七）。日期：2026-10-10。基准：`main` `fafc595`。
依据：[多语言总体稿](design-multilang-runtimes-2026-10-03.md)（下称"总体稿"，本文是其 M4，修订两点见附录 A.3）、[M1](design-multilang-m1-2026-10-04.md)、[M2](design-multilang-m2-2026-10-04.md)、[实例服务](design-instance-services-2026-10-08.md)、[插件 API](guide/plugin-api.md)、[Bun 运行时](design-bun-runtime-2026-10-09.md)（通道、`mount` 回复、远程监听、名字冲突、测试与之对齐）。

范围外：Go 侧 Cordis；Go `plugin` 包；宿主运行时编译 Go 代码。

## 一、决策

| # | 决策 | 附录 |
| --- | --- | --- |
| D1 | 一个二进制一个运行时进程；宿主同时运行多个 Go 运行时 | A |
| D2 | 一个二进制含一个或多个插件，由其 `main.go` 的 `rutis.Serve(...)` 决定 | A |
| D3 | 行名 `go:<插件名>`，`GoResolver` 按清单路由；限定写法 `<运行时名>:<插件名>` | — |
| D4 | 运行时名由文件名得出，是唯一的用户可见名 | — |
| D5 | 二进制自描述：`--rutis-manifest` 输出清单，解析不需要启动进程 | — |
| D6 | 进程运行期间，解析以启动清单为准；换文件在重启后生效，不自动重启 | E |
| D7 | 按需启动，空闲停止；`eager` 全部启动且不停止 | — |
| D8 | 同二进制内调用不走 IPC；跨二进制、跨语言经 rutis 按名字转发 | — |
| D9 | 每个进来的调用一个 goroutine：运行时可重入 | G |
| D10 | 调用链与取消放在 `context.Context` 中，插件必须向下传递 | D |
| D11 | 叶子插件；插件 API 版本 1；配置为结构体、Schema 反射生成 | — |
| D12 | 服务方法默认 `async`，`rutis.Sync(...)` 标同步 | B |
| D13 | 结构体按数据复制；按引用传对象须写 `rutis.Ref(v)` | C |
| D14 | 使用服务：绑定函数字段的结构体 | — |
| D15 | 宿主不编译；`run` 只运行现成二进制，`dev` 只重建项目自身 | — |
| D16 | 不同 SDK 版本的二进制可共存，宿主与二进制之间只约定线协议、清单格式、插件 API 版本 | — |
| D17 | SDK 只依赖标准库，位于 `go/rutis` | — |

## 二、运行时模型

```text
rutis 宿主
 ├─ LocalRuntime::node / ::python ...
 ├─ GoRuntimes（GoResolver + 生命周期）
 │    ├─ plugins/go/weather ── go-weather ── weather
 │    ├─ plugins/go/netkit  ── go-netkit  ── ping、dns、traceroute
 │    └─ plugins/go/k8s     ── 未启动（无行使用）
 └─ LoaderPlugin + 每个运行中的运行时一个 RuntimeRowsPlugin
```

- 行依赖 `RuntimeRows#<运行时名>` 与插件 `inject` 的全部名字（叶子运行时，rutis 门控全部名字）。
- 插件 `provides` 的服务从行的 fiber 发布到 `host_key(name)`。
- 进程退出：其全部行停止，使用其服务的行随之停止。
- `isolate`、实例标签、服务 id（名字 + NUL + 标签）、导出句柄：与 Python 运行时相同。

## 三、二进制与清单

### 3.1 二进制

- 插件作者分发：Go 包（导出 `Plugin`）+ `cmd/<名字>/main.go`（`Serve` 自己的插件）。
- 宿主接受任何响应 `--rutis-manifest` 的可执行文件（来源见附录 H）。

```go
// example.com/netkit/cmd/netkit/main.go
func main() { rutis.Serve(ping.Plugin, dns.Plugin) }
```

| `Serve` 规则 | |
| --- | --- |
| 参数 | `--rutis-manifest`：打印清单后退出；否则 `<channel> [--id <endpoint>] [--peer <endpoint>] <project>` |
| 插件同名 | 启动即退出，指明两个包 |
| 登记方式 | 只在 `Serve` 的参数里，不用 `init()` |
| 子命令 | `rutis.ServeArgs(args []string) error` |

### 3.2 清单

```json
{
  "manifest": 1,
  "sdk": "0.9.0",
  "pluginApi": 1,
  "plugins": {
    "weather": {
      "config": { "type": "object", "properties": { "city": { "type": "string" } } },
      "inject": ["llm"],
      "provides": { "weather": { "today": "async", "unit": "sync" } },
      "version": "v1.2.0"
    }
  }
}
```

| 项 | 规则 |
| --- | --- |
| 输出 | stdout，退出码 0；不连通道，不执行 `Apply` |
| 插件条目 | 与 `rows.schema` 回复相同，同一段代码生成 |
| `sdk` | 常量 `rutis.Version` |
| 插件 `version` | `debug.ReadBuildInfo()`：依赖模块取 `Deps` 版本；主模块取 `Main.Version`；`(devel)` 时取 `vcs.revision`（脏工作区加 `+dirty`）；都无则 `null` |
| 执行超时 | 30 秒（见 §十七） |
| 启动失败重试 | 文件正忙（ETXTBSY：刚写好的文件被别的线程正在启动的子进程继承了写句柄）时重试启动，至多 5 次，间隔 10 ms 起倍增 |
| 平台不符 | 报"不是为 <os>/<arch> 构建的" |
| `pluginApi` 过高 | 该二进制全部插件解析失败，提示升级对象 |
| 缓存键 | 路径、大小、mtime；Unix 加 ctime |
| 绕过缓存 | 启动运行时、`GoRuntimes::restart`、`rutis-host check`；另：行要的插件没有任何二进制有时，重读上次读取失败、且失败已记录满 5 秒的清单（失败可能已过去；不满 5 秒不重读，找不到的名字不会让持续失败或挂起的二进制每次都重跑） |
| 信任 | 只执行配置给出的来源（`binaries` 文件、`dir` 目录）；`dir` 整体视为受信任，应为专用目录 |

## 四、`GoResolver`

位于 rutis-loader（feature `go`），实现 `Resolver`。

| 项 | 规则 |
| --- | --- |
| 来源 | 文件列表 + 目录列表；遇到未知插件名时重扫一次目录 |
| 目录中的候选文件 | 可执行、不以 `.` 开头、内容含 SDK 标记字符串（如 `rutis-go-runtime:1`）；只执行候选文件 |
| 候选文件取清单失败 | 跳过并报诊断，不是配置错误；`check` 列出并以非零退出 |
| `binaries` 中文件取清单失败 | 配置不失败；该文件的插件全部解析失败并附原因 |
| 运行时名 | 去扩展名（`.exe`）→ 小写 → 非 `[a-z0-9-]` 换 `-` → 加前缀 `go-`（`net.kit_v1.exe` → `go-net-kit-v1`） |
| 运行时名的用途 | `Runtime#…`、`RuntimeRows#…` 的键；端点 id；限定行名、`restart`、`runtimes()`、`check`、歧义提示中的唯一名字。文件名只出现在 meta 的 `binary` |
| 重名 | 与其他运行时（`node`、`py`、`bun`、远程）或彼此重名：`binaries` 中为配置错误；目录中为跳过 + 诊断 |
| `go:<插件>` | 查所有清单。无：`NotFound`。多个：歧义错误，列出运行时，提示 `<运行时名>:<插件>` |
| `<运行时名>:<插件>` | 认领以其运行时名加 `:` 开头的行名，只查该清单 |
| 解析结果 | 与 `RuntimeResolver` 的叶子结果相同；"由声明构造 `Resolved`"的代码抽出共用 |
| meta | `{ source: "go", binary, runtime, version, sdk }` |
| 运行中换文件 | 进程运行期间以启动清单解析；新二进制才有的插件报"已更换，`restart(\"<运行时名>\")` 后生效"；`check` / `runtimes()` 显示"已更换，待重启"。进程未运行时用磁盘清单 |
| 第二段放行 | 保留 `RuntimeRows`；`RuntimeRowsPlugin` 改为接受 trait（运行时名 + 取需刷新的行），`GoResolver` 按运行时实现；需刷新 = 启动后清单变化的行 |

## 五、`GoRuntimes`

位于 rutis-loader，挂在 loader 之后。

```rust
let go = Arc::new(GoResolver::new(GoBinaries::new().dir("plugins/go").file("bin/weather")).with_catalog(&catalog));
chain = chain.with_shared(go.clone());
root.plugin(GoRuntimes::new(go, project).idle(Duration::from_secs(60)));
```

| 项 | 规则 |
| --- | --- |
| 结构 | 每个运行中的运行时 = 一个子 `Ctx`：`LocalRuntime::go(binary, project).named(名)` + `RuntimeRowsPlugin`；停止 = 释放子 `Ctx` |
| 启动 | `GoResolver` 解析出一行且其运行时未运行时启动；多个冷启动并行 |
| 空闲停止 | 仅按需启动的运行时：最后一行卸载后等 `idle`，若 `Loader::entries()` 中无未禁用条目解析到它则停止；期间有新解析则取消 |
| 崩溃 | 行停止，记录原因，不自动重启；解析也不触发重启。重启条件：`GoRuntimes::restart(名)`，或二进制文件已变化 |
| `eager()` | 挂载时启动全部；不做空闲停止 |
| `runtimes()` | 运行时名、状态（未启动 / 启动中 / 运行中 / 已停 + 原因 / 已更换待重启）、插件、版本 |

单个固定二进制的应用可不用 `GoResolver`：`LocalRuntime::go` + `RuntimeResolver::modules`（行名 `<运行时名>:<插件>`）。

## 六、Go SDK

### 6.1 插件

```go
type Config struct {
	City string `json:"city,omitempty" doc:"要查天气的城市"`
}

type LLM struct {
	Ask func(ctx context.Context, question string) (string, error)
}

type Weather struct{ llm LLM; city string }

func (w *Weather) Today(ctx context.Context) (string, error) {
	answer, err := w.llm.Ask(ctx, "weather in "+w.city)
	if err != nil {
		return "", err
	}
	return answer + " in " + w.city, nil
}

func (w *Weather) Unit() string { return "celsius" }

var Plugin = rutis.Define(rutis.Plugin[Config]{
	Name:     "weather",
	Inject:   []string{"llm"},
	Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Unit"))},
	Apply: func(ctx *rutis.Ctx, config Config) error {
		var llm LLM
		if err := ctx.Use("llm", &llm); err != nil {
			return err
		}
		ctx.Provide("weather", &Weather{llm: llm, city: config.City})
		return nil
	},
})
```

| API | 规则 |
| --- | --- |
| `rutis.Define` | 返回无类型参数的 `*rutis.Definition` |
| `Apply` 返回错误 | 装载失败；已登记的清理照常运行 |
| `*rutis.Ctx` | 实现 `context.Context`，行卸载时取消 |
| `ctx.Use(name, &target)` | `target`：函数字段结构体或 `*rutis.Service`；`name` 须在 `Inject` 中 |
| `ctx.Provide(name, value)` | 至卸载或调用返回函数为止；`value` 类型须与 `Provides` 一致 |
| `ctx.Effect(cleanup)` | `cleanup(context.Context) error`，卸载时逆序执行 |
| `ctx.Go(fn)` | goroutine 中运行，捕获 panic 写 stderr；卸载时取消并等待返回 |
| `*rutis.Service` | `svc.Call(ctx, "ask", &answer, args...)` |
| `rutis.NoConfig` | 无配置 |
| `Plugin.Schema` | 直接给 Schema，不再反射生成 |

### 6.2 函数字段结构体

```go
type LLM struct {
	Ask    func(ctx context.Context, question string) (string, error)
	Models func(ctx context.Context) ([]string, error) `rutis:"listModels"`
}
```

- 字段名按 §6.4 转为方法名；`rutis:"…"` 覆盖。
- 可只写用到的方法；每个字段须对应服务声明的方法，否则 `Use` 报错。
- 签名：可选首参 `context.Context`；末个返回值 `error`，之前至多一个结果。
- 提供者同进程：直接调用提供者方法，不复制；签名不完全相同时在进程内按 §6.5 转换一次。

### 6.3 配置 Schema

| Go | JSON Schema |
| --- | --- |
| `string` / `bool` / 整数 / 浮点 | `string` / `boolean` / `integer` / `number` |
| 切片、数组 | `array` + `items` |
| `map[string]T` | `object` + `additionalProperties` |
| 结构体 | `object` + `properties`；无 `omitempty` 的字段进 `required` |
| `json:"-"` | 省略 |
| `doc:"…"` | `description` |
| 实现 `rutis.Schemer` | 其自身 Schema |

配置在 `Apply` 前由 `encoding/json` 解码；失败即装载失败。

### 6.4 方法

| 项 | 规则 |
| --- | --- |
| 服务方法 | `Provides` 中类型的导出方法 |
| 线上名 | 首字母小写；开头连续大写整体小写，若其后紧跟小写字母，最后一个大写字母归下一个词（`Today`→`today`，`ID`→`id`，`URLFor`→`urlFor`，`HTTPServer`→`httpServer`，`GetURL`→`getURL`）；`rutis.Rename` 覆盖；重名则 `Define` 报错 |
| 签名 | `func (T) M([ctx context.Context,] 参数...) ([结果,] [error])` |
| 参数 | 按位置解码；多余报错；缺少取零值 |
| 错误 | 非 nil `error` 抛出；panic 捕获为 `Panic` 错误（带栈） |
| 形状 | `MethodsOf` 默认 `async`；`rutis.Sync(...)` 标 `sync` |
| `async` 执行 | goroutine 中执行，回复异步结果引用；调用方取消 → 方法 ctx 取消 |

### 6.5 值

| Go 值 | 跨进程 |
| --- | --- |
| `nil`、布尔、数字、字符串 | 复制；整数超出 ±2^53−1 报错 |
| 切片、数组、`map[string]T`、结构体 | 递归复制，遵循 `json` 标签；`json.Marshaler` 自行编码 |
| `error` | 复制名字与消息；名字 = 类型名（`*fs.PathError`→`PathError`）或 `Name() string` |
| 函数 | 引用 |
| `rutis.Ref(v)` | 引用，代理调用 `v` 的导出方法 |
| `ctx.Provide` 的值 | 总是引用 |
| `[]byte`、channel、`complex`、非字符串键 map、循环 | 不可跨进程 |

接收：函数类型 → 调用远端函数的 Go 函数；`*rutis.Future` → `Await(ctx)`；其余按 `encoding/json`。不接收对象引用（不声明 `objects`）。

## 七、并发、调用链、取消

| 项 | 规则 |
| --- | --- |
| 会话结构 | 一个读 goroutine；每个发出的调用一个回复 channel；每个进来的 `call` / `invoke` / `get` / `await` / 控制操作一个新 goroutine；写帧一把锁；各表各自加锁 |
| 可重入 | 任何时候执行任何进来的调用；声明 `reentrant-sync` |
| 等待环检测 | 同步调用的调用帧带 `sync: true`（只发给声明了 `sync-wait` 的对端），声明 `sync-wait`；不声明 `sync-stack`（#385） |
| 插件要求 | 服务对象并发安全；调用其他服务时不持锁 |
| 调用链 | 进来的调用：`path + [id]` 放入方法 ctx；发出的调用：从传入 ctx 取 `path`；`Apply` / 清理的 ctx 带 `rows.load` / `rows.unload` 的链 |
| ctx 检查 | `nil` ctx → panic；开发模式（`RUTIS_DEV=1`）下，无链 ctx 且进程内有进来的同步调用 → 警告一次并指出位置 |
| 进来的取消 | `cancel` 帧 → 方法 ctx 取消 |
| 发出的取消 | `async`：发 `cancel`，立即返回 `ctx.Err()`，丢弃迟到结果；`sync`：立即返回 `ctx.Err()`，丢弃迟到结果，对端照常执行 |
| 行卸载 | 取消该行 `*rutis.Ctx` |
| `SyncWaitCycle` | 自身不产生；收到时为 `*rutis.RemoteError`，`errors.Is(err, rutis.ErrSyncWaitCycle)` |

## 八、运行时进程

### 8.1 通道

| channel | 用途 | 实现 |
| --- | --- | --- |
| `fd:3` | Unix 本地继承 | `os.NewFile(3)` + `net.FileConn` |
| `unix:<path>` | 拨回 | `net.Dial("unix", …)` |
| `tcp:127.0.0.1:<port>` | Windows 本地 | 先发 `RUTIS_CHANNEL_TOKEN` + 换行，随即从环境删除 |
| `listen:ws://…` / `listen:wss://…` | 远程（G2） | §九 |
| `ws://…` | — | 启动报错：只监听 |

- 按行分帧；单条消息长度上限同 [#173](https://github.com/arcships/rutis/issues/173)，超限关闭通道。
- `project` 参数保留，经 `rutis.Project()` 提供。

### 8.2 契约

特性 `["rows.v2", "hosts", "leaf", "scopes"]`；能力 `signals`、`reentrant-sync`、`sync-wait`（compat 握手里为 `reentrant-sync`、`sync-wait`）。

| 控制操作 | 行为 |
| --- | --- |
| `mount` | `{ services: {}, features, implementation: { name, version }, engine: { name: "go", version: runtime.Version() } }` |
| `rows.schema(entry)` | `{ config, inject, provides, version }`；不存在时报错并列出已有插件 |
| `rows.load(key, entry, config, isolate, inject, exports)` | 建行与导出槽位，解码配置，执行 `Apply`；失败则卸载该行后报错 |
| `rows.update(key, config)` | 卸载后重新装载 |
| `rows.unload(key)` | 撤销服务 → 取消行 ctx → 逆序清理 |
| `hosts.provide(name, methods, label)` / `hosts.withdraw(id)` | 登记 / 撤销 rutis 服务代理（`scopes` 规则） |
| `release` / `get` | 释放导出对象 / 读导出字段 |
| `dispose` | 卸载全部行，等待进行中的调用 |

- `service(id, handle, version)` 通知在 `ctx.Provide` 返回前发出（先于 `rows.load` 回复）。
- `implementation` / `engine` 随 Bun B1（#200）落地；Python 同时补上（`engine`：`python` + `platform.python_version()`）。
- 实现名 = 发布包名：Python `rutis`，Bun `@arcships/rutis-bun`，Go `github.com/arcships/rutis/go/rutis`。`check` 统一格式打印。

### 8.3 退出与清理时限

| 情况 | 规则 |
| --- | --- |
| 调用、`Apply`、清理中的 panic | 捕获为错误 |
| 插件 goroutine 未捕获的 panic | 进程退出；行全部停止 |
| `rows.unload`、`dispose` | 不设时限（同 Python）；时限由 rutis 侧决定 |
| 会话已结束 | 卸载全部行，清理合计至多 5 秒；超时放弃余下清理，stderr 记录涉及的行，`os.Exit(0)` |
| 未观察 `ctx.Done()` 的 goroutine | 卸载后继续运行，SDK 不干预（写入指南） |

## 九、远程（G2）

`<二进制> listen:wss://0.0.0.0:7443/rutis --id office-go --peer main <project>`

| 项 | 规则 |
| --- | --- |
| 协议 | 端点格式（协议 3），子协议 `rutis.3` |
| 控制方 | 一次一个；新连接接管，旧租约清理完再问候新会话；旧连接以 4002 关闭 |
| 凭据 | `RUTIS_TOKEN`、`RUTIS_CERT`、`RUTIS_KEY`；token 恒定时间比较 |
| 无 TLS | 只允许回环地址 |
| 拒绝 | 404 路径、401 无凭据、403 token 错、400 子协议错 |
| 消息上限 | 16 MiB，超限以 1009 关闭 |
| 心跳 | 30 秒无响应断开；`RUTIS_HEARTBEAT` 可调 |
| 启动输出 | stderr `rutis: listening on …` |
| 实现 | `net/http` 自写服务端，仅文本帧 |
| 宿主侧 | 每个远程二进制一个 `RuntimePlugin::remote(名)`；行名 `<名>:<插件>`；声明经 `rows.schema` |

## 十、Rust 侧改动

| 改动 | 位置 |
| --- | --- |
| `Launcher::go(binary, project)`：程序即二进制，`cwd = project`，Unix 上 `inherit_fd` | `rutis-bridge/src/runtime/process.rs` |
| `LocalRuntime::go(binary, project)`，默认名由文件名得出 | `rutis-bridge/src/runtime/local.rs` |
| feature：rutis-bridge `go = []`；rutis-loader `go = ["runtimes", "rutis-bridge/go"]`；rutis-host 依赖开 `go` | 三个 `Cargo.toml` |
| `GoResolver`、`GoBinaries`、`GoRuntimes` | `rutis-loader/src/runtime/go.rs`（新） |
| `RuntimeRowsPlugin` 接受 trait | `rutis-loader/src/runtime/rows.rs` |
| `RowSchema.version` 写入 meta（与 Bun B1 共用，先落地者实现） | `rutis-bridge`、`rutis-loader` |
| 远程 `Naming::Npm` 拒绝 `go:` 及 `<Go 运行时名>:` 前缀 | `rutis-loader/src/runtime.rs` |

## 十一、rutis-host

```json
{
  "runtimes": { "go": { "dir": "plugins/go", "binaries": ["bin/weather"], "start": "on-demand", "idle": 60 } },
  "rows": [{ "id": "weather", "name": "go:weather", "config": { "city": "Oslo" } }]
}
```

| 字段 | 规则 |
| --- | --- |
| `dir` / `binaries` | 至少一个 |
| `start` | `on-demand`（默认）/ `eager` |
| `idle` | 秒；仅 `on-demand`；与 `eager` 同时出现为配置错误 |
| `remote[].language` | 增加 `go` |

| 命令 | 行为 | 阶段 |
| --- | --- | --- |
| `check` | 每个二进制：运行时名、平台、SDK / 插件 API 版本、插件、实现与引擎；歧义；每行的目标二进制；失败的候选文件 | G2 |
| `dev` | 无 `rutis.dev.json` 时据 `go.mod` 判定 Go 项目；重建与重启见 §十二 | G2 |
| `new <name> --lang go` | 插件包、`cmd/<name>/main.go`、测试、`rutis.dev.json`、发布工作流（tag → `go test`、`check`、按平台出二进制） | G2 |
| `go add <模块>@<版本>` | `go install` 到 `dir`；无工具链时报错 | G2 |
| `go add <URL>` | 下载预编译二进制到 `dir`，校验 SHA-256 | G3 |
| `run` | 不需要 Go 工具链 | — |

macOS 隔离属性：`check` 与启动时检查（复用 dylib 隔离检查），报原因与处理办法，不自动移除。

## 十二、换代码

| 场景 | 流程 |
| --- | --- |
| `dev` | 监视 `.go`、`go.mod`、`go.sum` → 构建到缓存目录新文件 `<name>-<序号>` → 成功则替换该运行时的文件并重启它，删旧文件；失败则打印错误，旧进程继续 |
| 部署 | 改名替换文件 → `GoRuntimes::restart("<运行时名>")`（rutis-host：重启宿主）；重启前按启动清单解析 |
| 重启效果 | 该运行时的行及其使用者停止；新进程起来后按新清单解析、按依赖启动；其他运行时不受影响 |

## 十三、仓库与发布

| 项 | 规则 |
| --- | --- |
| 位置 | `go/rutis`，模块 `github.com/arcships/rutis/go/rutis` |
| 包 | `rutis`（API、`Serve`、清单）、`rutis/rutistest`；会话层 `internal/peer` |
| 版本 | `go/rutis/version.go` 的 `const Version`；用于 `implementation.version` 与清单 `sdk`；`scripts/train.mjs` 以正则校验 |
| 发布 | tag `go/rutis/vX.Y.Z`，经 Go 模块代理 |
| 依赖 | 仅标准库；Go 版本：当前两个稳定版 |
| 兼容 | 同一协议版本内，任意 SDK 版本的二进制可在任意宿主版本运行，前提是插件 API 不高于宿主支持 |
| 文档 | `docs/guide/go-plugin.md`（中英）；插件 API 文档加 Go 列 |

## 十四、测试

| 测试 | 位置 | 内容 |
| --- | --- | --- |
| 会话层单元 | `go/rutis/internal/peer` | 编解码、引用计数、同步中回调、取消、ctx 取链、并发 |
| 会话一致性 | `rutis-bridge/tests/runtime_conformance.rs` + Go `conformance` 目标 | `session::testing::session` |
| 契约 × 通道 | `rutis-bridge/tests/session_matrix.rs` 加 `Go` | `runtime::testing::runtime`：`fd:3`、拨回；G2 加 WebSocket |
| Python 专项的 Go 版 | `python_runtime.rs` 的 Go 版；`cancellation.rs`、`error_shape.rs`、`rpc_callbacks.rs`、`process_exit.rs`、`live_objects.rs` 加 Go 列 | features；取消到 ctx；错误形状（类型名、`Panic`）；panic 退出撤回服务；引用与 release；夹具 `conformance-session/weather/greeter`，`crash()` 退出码 17 |
| 无宿主插件测试 | `rutistest.Load(t, plugin, config, services)` | 只用 `Inject` 的服务；提供声明的方法；清理运行；严格模式按 §6.5 编解码 |
| 清单 | `go/rutis` + `rutis-loader/tests/go_rows.rs` | 与 `rows.schema` 一致；缓存命中不执行；插件 API 过高、平台不符的错误 |
| 多二进制 | `go_rows.rs` | 按插件名路由；歧义与限定写法；无标记文件不执行、失败文件跳过；运行中换文件按启动清单、`restart` 后生效；原位替换保留大小 / mtime 时 `restart` 读到新清单；跨二进制同步 / 异步调用；单进程崩溃只影响自身行及使用者 |
| 按需启动 | `go_rows.rs` | 未使用不启动；空闲后退出；期间新行取消退出；崩溃不重启，换文件后可启动 |
| 运行时一致性 | `multilang.rs`（feature `go`） | Go 版叶子插件；三语言冷启动互调；提供者移除只停使用者；`inject` 门控 |
| 交叉同步调用 | `multilang.rs` | Node→Go→回调 Node（带 ctx）、Go↔Python、Go↔Go：不卡死 |
| 实例 | `instance_runtimes.rs` | 标签隔离、实例自有服务 |
| 远程租约（G2） | `leases.rs`、`remote_rows.rs` 加 Go 列 | 同 Bun 设计 §8 |
| Windows | `runtimes-windows` | loopback；`dev` 换文件重启 |

- 夹具：`crates/rutis-loader/tests/fixtures/go`，测试开始时构建成至少两个二进制。
- CI `runtimes-go`（Linux、macOS，`actions/setup-go` 两个稳定版）：`go test ./...`、`cargo test -p rutis-bridge --features go,…`、`-p rutis-loader --features go,…`、`-p rutis-host`；`cargo check --no-default-features --features go`。
- `test`、`network-macos`、`runtimes-windows` 加 `actions/setup-go`；`runtimes-windows` 加 `cargo test -p rutis-host`。
- E2E：S2（[#186](https://github.com/arcships/rutis/issues/186)）`new --lang go` 循环；S3（[#187](https://github.com/arcships/rutis/issues/187)）跨语言与崩溃恢复；S9（[#193](https://github.com/arcships/rutis/issues/193)）无工具链环境运行下载的二进制。

## 十五、分阶段

| 阶段 | 内容 | 验收 |
| --- | --- | --- |
| G1 | SDK（会话、叶子 API、`Serve` 的 `fd`/`unix`/`tcp`、清单、`rutistest`）；`Launcher::go`、`LocalRuntime::go`、feature `go`；`GoResolver`；`GoRuntimes`（仅 `eager`）；`RuntimeRowsPlugin` trait 化 | §十四 除按需启动、远程外全部通过；多二进制并行；Linux / macOS / Windows |
| G2 | 按需启动与空闲停止；rutis-host（`runtimes.go`、`dev`、`check`、`new --lang go`、`go add <模块>@<版本>`）；指南；远程 | 按需启动测试通过；`dev` 改代码只重启本运行时；远程 Go 运行时可控 |
| G3（按需） | 下载预编译二进制；接收对象引用；生成组合用 `main.go` | 有需求时定 |

## 十六、待定

| 项 | 当前取向 |
| --- | --- |
| 清单获取方式 | 执行二进制；备选为构建时写入文件、宿主直读（需 `go generate`） |
| `idle` 默认值 | 60 秒；备选默认不停止 |
| `run` 是否构建 | 否 |
| 默认方法形状 | `async` |
| 函数字段外的类型化绑定 | 暂不做（备选 `go generate`） |
| 接收对象引用 | G3 |
| 调用链丢失检测 | 仅开发模式警告 |

## 十七、实现说明

G1、G2 由 #227 实现。与本文的出入：

| 项 | 本文 | 实现 |
| --- | --- | --- |
| `async` 方法的回复 | 回复异步结果引用 | 方法返回时直接回复结果；`cancel` 帧取消方法的 ctx（Rust 侧对 async 方法的回复按值或 future 都能 settle） |
| 清单 | §3.2 的字段 | 另有 `runtime: "rutis-go-runtime:1"`（SDK 标记，保证它在二进制里）和 `platform`（`GOOS/GOARCH`，`check` 打印） |
| `ServeArgs` | `ServeArgs(args []string) error` | `ServeArgs(args []string, defs ...*Definition) error`；另有 `ServeConn(net.Conn, defs...)`、`Manifest(defs...)` |
| `*rutis.Service` | `Call`、`Methods` | 另有 `Local()`：服务是否由同进程的插件提供 |
| 测试工具 | `rutistest.Load` | `Load(t, plugin, config, services) *Loaded`；`Service(name)` 的 `Call` / `Bind`；插件在真正的运行时里、经内存会话运行 |
| §十四 的"Go 列" | `cancellation.rs` 等加 Go 列 | 这些文件只针对 Node；Go 的对应检查集中在 `rutis-bridge/tests/go_runtime.rs` |
| 启动触发 | 解析出行时启动 | 另由每 100 ms 的检查补足：loader 复用解析结果时不再问解析器 |
| 空闲停止后 | 已停 + 原因 | 回到"未启动" |
| `runtimes.go` | `dir`、`binaries`、`start`、`idle` | 另有 `project`（运行时的工作目录） |
| 开发构建 | — | `GoBinaries::file_named`、`GoResolver::replace`、`read_go_manifest` |
| 远程 npm 命名 | 拒绝 `go:` 与 `<Go 运行时名>:` | 拒绝任何 `<名字>:` 前缀 |
| macOS 隔离属性 | 复用 dylib 的检查 | `rutis-host check` 用 `xattr` 检查并提示 |
| 清单执行超时 | 5 秒 | 30 秒：macOS 上新二进制第一次运行要等系统评估，几个同时启动时超过 5 秒 |
| 同一运行时的启动、重启、停止 | §五：空闲停止期间又有行解析到它，就取消这次停止 | 同一时刻只有一个在进行（运行时标记为忙，离开时一定清除）；新进程在旧进程释放后才启动；`restart` 的调用方放弃等待（如外面套了超时）时，重启照样走完，下一次重启等它结束。空闲停止期间又有行解析到它：不取消这次停止，停止完成后立即重新启动 |
| `rows.load` 的 `inject` | 签名里有 | 忽略，与 Python 相同：行配置的 `inject` 是 Cordis 行在 Cordis 里等待用的，叶子运行时的门控由 rutis 按插件声明的 inject 做 |
| `check` | 打印实现与引擎 | 不打印：`implementation` / `engine` 在 mount 回复里，`check` 不启动 Go 运行时；随 Bun B1 统一 |
| 未做 | | Python `mount` 回复的 `implementation` / `engine` 与 `check` 打印它们（随 Bun B1）；G3 |

---

## 附录

### A. 一个二进制一个运行时

**A.1 Go 与 Node / Python 的差异**

| | Node / Python | Go |
| --- | --- | --- |
| 运行时加载代码 | 可以 | 不可靠：`plugin` 包要求同一工具链、同一依赖，且不能卸载 |
| 分发物 | 源码包（npm / PyPI），装进同一环境 | 编译好的二进制，按平台发布或 `go install` |
| 合并多个作者的插件 | 装进同一环境即可 | 需工具链、整体重编，依赖须合为一份（MVS 每模块一个版本），无法保证 |

**A.2 与 Python 运行时对比**

| | Python | Go |
| --- | --- | --- |
| 进程数 | 一个环境一个 | 一个二进制一个，用到才启动 |
| 可装载的插件 | 环境中可导入的 | 构建时固定，见清单 |
| 行名 → 进程 | 前缀即运行时名 | 按清单路由 |
| 启动前的声明 | 无，需第二段放行 | 有（清单） |
| 依赖冲突 | 同环境仅一份 | 各二进制独立 |
| 换代码 | 重新导入模块 | 换二进制，重启该运行时 |

**A.3 对总体稿的修订**：总体稿把 Go 定为"一组插件一个可执行文件，需要隔离时才多开"。修订为：(1) 分组由二进制的作者或组合者决定，多个 Go 运行时并存是常态；(2) 行名通常不带运行时名，由 `GoResolver` 路由。

**A.4 进程数的代价**：Go 进程启动为毫秒到数十毫秒级，空闲内存为数 MB 到十余 MB；按需启动使未用的二进制不占进程；需要单进程的部署可自行组合为一个二进制，模型不变。

### B. 默认 `async`

- 对 Go 调用方两者无区别（总是阻塞当前 goroutine）。
- `sync` 方法会阻塞 Node 调用方的事件循环；Go 方法常做网络 I/O。
- 不按签名推断：同步方法也可能回调其他服务，同样需要 ctx 带链。

### C. 结构体按数据、对象须 `rutis.Ref`

Go 结构体既可是数据也可是对象，无法像 Python 那样按"是否有方法"判定，故默认数据，引用须显式标记。

### D. 必须向下传 ctx

Node 同步调用 Go 方法，该方法以 `context.Background()` 同步回调 Node 传入的函数：回调不带链，Node 视为无关调用而延后，同时 Node 在等 Go 返回，双方互等。Go 侧不会因此卡死（总是可重入），对端会。

### E. 运行中以启动清单为准；缓存键

- 新解析的行必须与运行中的进程一致；按磁盘清单解析会把新声明的行装进旧进程，或报"无此插件"。
- 原位替换并保留大小与 mtime（`rsync -t`、解包、CI 缓存）会命中旧缓存：Unix 的 ctime 仍会变；Windows 无可用 ctime，故关键时刻绕过缓存。
- 正在运行的可执行文件：Windows 不可覆盖（故 `dev` 每次构建到新文件名），Unix 不可原地写入（故部署用改名替换）。

### F. 清理时限

`rows.unload` / `dispose` 有 rutis 在等待，时限由 rutis 侧（`dispose_with_timeout`、进程管理）决定；会话已结束时无人等待，需自行限时以免残留 goroutine 拖住进程退出。必须完成的工作不应只放在清理里。

### G. 可重入与总体稿 §九

- 每个调用一个 goroutine，同步等待只阻塞发起者，因此 Go↔Go、Go↔Node、Go↔Python 的交叉同步调用不会卡死；代价与 Python 相同：服务会被并发调用。
- 已接入或已设计的叶子运行时（Python、Bun、Go）均可重入。本文建议规则"叶子运行时都可重入、只有 Cordis 不可重入"，待 Swift（M3）确认，记录于总体稿 §九。两个 Cordis 运行时之间的交叉同步调用、Rust 侧等待环检测仍在总体稿 §九 待定。

### H. 二进制来源

1. 作者按平台发布的预编译二进制；
2. `GOBIN=plugins/go go install <模块>/cmd/<名>@<版本>`；
3. 部署方组合多个插件包编成的二进制（依赖须可合并，由部署方负责）；
4. `rutis-host dev` 构建的项目二进制。
