# 写一个 Go 插件

从创建项目到发布，再到被宿主使用。需要 Go 1.24 或更高。

Go 插件和 TypeScript、Python 插件的写法与规则相同（见 [插件 API](plugin-api.md)），不同的是分发方式：**Go 插件编译成二进制，一个二进制就是宿主里的一个运行时进程**。一个二进制可以带一个或几个插件；宿主可以同时运行很多个二进制，只在有行用到时才启动它。

## 1. 创建项目

下面的 `rutis-host`（建项目、在本地宿主里运行、检查）要先装好，任选 [rutis-host：安装](rutis-host.md#安装) 里的一种，例如 `npm install -g @arcships/rutis-host`，或把 GitHub Releases 上的二进制放进 `PATH`；不想装的话，每条命令写成 `npx @arcships/rutis-host …`。

```bash
rutis-host new weather --lang go
cd weather
go mod tidy
```

得到的项目：

| 文件 | 作用 |
| --- | --- |
| `plugin.go` | 插件本身（包 `weather`，导出 `Plugin`） |
| `plugin_test.go` | 单元测试，不需要宿主 |
| `cmd/weather/main.go` | 二进制：`rutis.Serve(weather.Plugin)` |
| `rutis.dev.json` | 本地运行时的配置和测试用的其他插件 |
| `go.mod` | 模块路径（改成你自己的）；依赖 `github.com/arcships/rutis/go/rutis` |
| `.github/workflows/release.yml` | 打 `v*` tag 时测试，并把各平台的二进制发布到 GitHub Release |

## 2. 写插件

生成的 `plugin.go` 是一个不用任何服务的 `greeter`，`plugin_test.go` 是它的测试。下面把它换成一个用到 `llm` 服务的 `weather`：`plugin.go` 换成这段，`plugin_test.go` 同时换成 §3 的那段（两个文件一起换）。

```go
package weather

import (
	"context"

	"github.com/arcships/rutis/go/rutis"
)

// 配置：JSON Schema 由 SDK 从这个类型生成。
type Config struct {
	City string `json:"city,omitempty" doc:"要查天气的城市"`
}

// 用到的服务：函数字段的结构体，字段名对应方法名（Ask → ask）。
type LLM struct {
	Ask func(ctx context.Context, question string) (string, error)
}

// 提供的服务：导出的方法就是服务的方法。
type Weather struct {
	llm  LLM
	city string
}

func (w *Weather) Today(ctx context.Context) (string, error) {
	answer, err := w.llm.Ask(ctx, "weather in "+w.city) // 把 ctx 传下去
	if err != nil {
		return "", err
	}
	return answer + " in " + w.city, nil
}

func (w *Weather) Unit() string { return "celsius" }

var Plugin = rutis.Define(rutis.Plugin[Config]{
	Name:     "weather",
	Inject:   []string{"llm"},                                                       // 都就绪才启动，任何一个撤销就停下
	Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Unit"))}, // 声明提供什么（check 运行前就知道）；Unit 标成 sync，其余 async
	Apply: func(ctx *rutis.Ctx, config Config) error {
		var llm LLM
		if err := ctx.Use("llm", &llm); err != nil {
			return err
		}
		city := config.City
		if city == "" {
			city = "Oslo" // 行里没写的字段是零值：默认值在这里给
		}
		ctx.Provide("weather", &Weather{llm: llm, city: city})
		ctx.Effect(func(context.Context) error { return nil }) // 清理，可以没有
		return nil
	},
})
```

要点：

| 事项 | 规则 |
| --- | --- |
| 方法名 | 导出方法的线上名字是首字母小写的形式（`Today` → `today`，`URLFor` → `urlFor`，`HTTPServer` → `httpServer`）；`rutis.Rename` 可以改 |
| 同步还是异步 | 默认 `async`；`rutis.Sync("Unit")` 标出同步方法。同步方法会让 Node 调用方阻塞事件循环，做 I/O 的方法保持 `async` |
| 方法签名 | `func (T) M([ctx context.Context,] 参数...) ([结果,] [error])` |
| 配置 | JSON Schema 由配置类型生成（字段名按 `json` 标签，`doc` 标签是说明）；行的 `config` 里没写的字段是零值，默认值在 `Apply` 里给（上面的 `city`）；要分清「没写」和「写了空值」，用指针字段（`*string`）。`Schema` 字段可以换成手写的 JSON Schema，`rutis-host check` 打印它，但不会替你填默认值 |
| 使用服务 | 函数字段的结构体可以只写用到的方法；或用 `*rutis.Service` 按名字调用：`svc.Call(ctx, "ask", &answer, "q")` |
| `ctx` | `*rutis.Ctx` 是一个 `context.Context`，插件卸载时取消；插件自己起的 goroutine 用 `ctx.Go(...)` 或看 `ctx.Done()` |
| 并发 | 每个进来的调用在自己的 goroutine 上执行：提供的对象要能被并发调用，调用别的服务时不要持有锁 |
| 值 | 数据（数字、字符串、切片、map、结构体）复制，结构体字段与切片、map 的元素逐个检查（字段名按 `json` 标签）；`time.Time`（时间点，UTC）、`[]byte`、`*big.Int`、键不是字符串的 map、`map[K]struct{}`（Set）与 `rutis.DateTime`、`rutis.Date`、`rutis.TimeOfDay`、`rutis.Decimal`、`rutis.UUID` 按各自的值传，见[值怎样传递](plugin-api.md#值怎样传递)；超出 ±(2^53−1) 的整数（`int64`、`uint64` 等）自动按大整数传，收到的大整数解码进声明的整数类型，到 `any` 时是 `*big.Int`；函数按引用；对象要按引用传时写 `rutis.Ref(v)` |
| 错误 | 返回的 `error` 带着类型名跨进程（`*QuotaError` → `QuotaError`，实现 `Name() string` 可以自定）；实现 `Code() string` 和 `Data() any` 时带上 code 和 data，调用方从错误的 `code`、`data` 读到（`Data` 按 JSON 编码）；panic 变成名为 `Panic` 的错误。收到的错误是 `*rutis.RemoteError`，有 `Name`、`Message`、`Code`、`Data` |

**一定把 ctx 传下去。** 调用链（`path`）放在 `context.Context` 里：Node 同步调用你的方法、你的方法又同步回调 Node 时，只有带着收到的 ctx，Node 才认得出这是它正在等的调用链；换成 `context.Background()` 两边会互相等。`rutis-host dev` 下，丢了调用链的调用会在 stderr 警告一次。

**插件本身也是一个值。** `rutis.Define(...)` 的结果（`*rutis.Definition`）可以照常作为参数交给一个服务（比如应用的部件登记处），函数字段的参数类型直接写 `*rutis.Definition`；收的一方就能在别处挂它，每挂一次是一份，每一份在这个进程里运行，用的是 `Apply` 里准备好的东西。只用来交出的定义可以不写 `Name`：

```go
var parts struct {
	Register func(ctx context.Context, scope string, def *rutis.Definition) error
}
if err := ctx.Use("parts", &parts); err != nil {
	return err
}
return parts.Register(ctx, "session", rutis.Define(rutis.Plugin[SessionConfig]{
	Inject: []string{"history"},
	Apply:  func(ctx *rutis.Ctx, config SessionConfig) error { /* 直接用 pool */ return nil },
}))
```

这一行卸载、重载时，从它交出的定义挂出的每一份先结束。见 [插件 API](plugin-api.md#插件定义)。

### 事件

一个宿主上所有语言、所有进程的监听器都在一条事件总线上（[设计](../design-events-2026-10-18.md)）：事件在哪里发出、监听器在哪里，结果都一样。

```go
off := ctx.On("session/created", func(ctx context.Context, args ...any) (any, error) {
	return nil, nil // nil 和 false 是没有结果，其余都是结果
}, rutis.Prepend()) // 选项：rutis.Prepend()、rutis.Global()

err := ctx.Emit(callerCtx, "session/created", id)          // 立即返回；err 只报拒绝
err = ctx.Parallel(callerCtx, "session/created", id)        // 等全部；*rutis.AggregateError，按登记顺序
value, err := ctx.Serial(callerCtx, "config/resolve", key)   // 第一个结果
value, err = ctx.Bail(callerCtx, "config/resolve", key)      // 同步调用的 Serial
value, err = ctx.Waterfall(callerCtx, "chat/reply", terminal, msg) // terminal 可为 nil；terminal 是 rutis.Terminal，即 func(ctx context.Context, args ...any) (any, error)，以事件参数调用
value, err = ctx.Filter(func(l rutis.Listener) (bool, error) { return l.Row == "chat", nil }).Serial(callerCtx, "config/resolve", key)
```

一个完整的例子：插件回答 `greeting/pick`、在 `greeting/format` 的 waterfall 里加一环，它提供的服务用 `Serial` 取第一个结果、用 `Waterfall` 让别的插件（任何语言）改写问候语、用 `Emit` 通知：

```go
package greeter

import (
	"context"
	"fmt"

	"github.com/arcships/rutis/go/rutis"
)

type Greeter struct{ ctx *rutis.Ctx }

func (g *Greeter) Hello(ctx context.Context, name, lang string) (string, error) {
	word, err := g.ctx.Serial(ctx, "greeting/pick", lang) // 第一个结果；nil 表示没有
	if err != nil {
		return "", err
	}
	if word == nil {
		word = "Hello"
	}
	text, err := g.ctx.Waterfall(ctx, "greeting/format", func(_ context.Context, args ...any) (any, error) {
		return args[0], nil // 终态：链的最里面
	}, fmt.Sprintf("%v, %s", word, name))
	if err != nil {
		return "", err
	}
	if err := g.ctx.Emit(ctx, "greeting/said", name); err != nil { // 立即返回
		return "", err
	}
	return text.(string), nil
}

var Plugin = rutis.Define(rutis.Plugin[struct{}]{
	Name: "greeter",
	Apply: func(ctx *rutis.Ctx, _ struct{}) error {
		ctx.On("greeting/pick", func(_ context.Context, args ...any) (any, error) {
			if args[0] == "sv" {
				return "Hej", nil
			}
			return nil, nil // 没有结果：下一个监听器接着来
		})
		ctx.On("greeting/format", func(ctx context.Context, args ...any) (any, error) {
			next := args[len(args)-1].(rutis.Next) // waterfall：最后一个参数是 next
			inner, err := next(ctx)
			if err != nil {
				return nil, err
			}
			return fmt.Sprintf("%v!", inner), nil
		})
		ctx.Provide("greeter", &Greeter{ctx: ctx})
		return nil
	},
})
```

Python、TypeScript 插件或宿主里的 Rust 代码监听同样的名字，就加入同一条链；写法见 [插件 API：事件](plugin-api.md#事件)。

| 事项 | 规则 |
| --- | --- |
| Waterfall | 监听器的最后一个参数是 `next`（`rutis.Next`，即 `func(context.Context) (any, error)`）；调用它执行链上其余部分，不调用就否决其余；第二次调用以 `WaterfallNext` 失败。返回最外层的值，原样 |
| 失败 | `Emit` 的监听器失败不影响发出方和其他监听器，在监听器所在处报告（stderr：`rutis: listener of <name> in row <row> failed: …`）。`Parallel` 全部跑完，返回 `*rutis.AggregateError`（只有一个失败也是）；`Serial`、`Bail`、`Waterfall` 遇到第一个错误停止并返回它 |
| 作用域 | 事件按名字在一条总线上送达；`ctx.Scope()` 是过滤用的描述，不是送达规则 |
| 过滤 | `Filter(keep)` 在任何监听器执行之前，按登记顺序对每个监听器调用一次 `keep`，传入它的 `rutis.Listener`（`Name`、`Prefix`、`Plugin`、`Row`、`Runtime`、`Node`、`Scope`、`Global`）；`keep` 返回 `(bool, error)`：它返回错误（或 panic）时，不执行任何监听器，这次分发以这个错误失败（`Emit` 不返回它，而是报告一行 `rutis: filter of <name> from row <row> failed: …`）；用 `rutis.Global()` 登记的监听器不经过滤 |
| Context | 监听器的 `ctx` 来自发出方（带调用链），这一行卸载时取消；`Parallel`、`Serial`、`Waterfall`、`Bail` 的发出方放弃时也取消，`Emit` 的监听器不因发出方放弃而取消。卸载等正在运行的监听器结束，最多 `RUTIS_SHUTDOWN_TIMEOUT` |
| 名字 | `internal/*` 是运行时自己的；`rutis/*`（`rutis/service`、`rutis/row`、`rutis/runtime`）可以监听，不能发出（`ReservedEvent`） |

和调用服务一样，把调用方给你的 context 传下去。`errors.Is(err, rutis.ErrWaterfallNext)`（以及 `ErrReservedEvent`、`ErrAsyncListener`、`ErrBinding`）匹配任何进程里的同名错误。

## 3. 测试

`rutistest` 不需要宿主，但插件在它真正的运行时里跑、值经过真正的会话传递：

```go
package weather

import (
	"context"
	"testing"

	"github.com/arcships/rutis/go/rutis/rutistest"
)

// 假的 llm：一个 Go 值，导出的方法就是服务的方法（Ask 是 ask）。
type fakeLLM struct{}

func (fakeLLM) Ask(ctx context.Context, question string) (string, error) { return "sunny", nil }

func TestToday(t *testing.T) {
	loaded := rutistest.Load(t, Plugin, Config{City: "Oslo"}, map[string]any{"llm": fakeLLM{}})
	var today string
	if err := loaded.Service("weather").Call(context.Background(), "today", &today); err != nil {
		t.Fatal(err)
	}
	if today != "sunny in Oslo" {
		t.Fatalf("got %q", today)
	}
}
```

```bash
go test ./...
```

`Provides` 写出插件提供什么，`rutis-host check` 不运行插件就知道 `weather` 由这一行提供：宿主在 CI 里用 `check --strict` 检查配置时，用到 `weather` 的行才能通过（模板默认就写了）。写了就按声明（`Sync` 标出同步方法、`Rename` 改名）。不写也能运行：`ctx.Provide` 提供的服务照样自动对外，方法取值的导出方法，都按 `async` 调用，只是 `check` 要等插件运行才知道，`--strict` 会报错。

测试里的事件：测试一侧就是总线的宿主。`loaded.Emit`、`Parallel`、`Serial`、`Waterfall`、`Bail` 和 `Filter` 从这一行的作用域发出；`loaded.On(name, fn)` 像宿主一样监听（插件发出这个名字的事件随后经测试一侧，就像经 rutis）；`loaded.Load(config, rutistest.InScope("t1/s1"))` 在一个作用域里再加载一行；`loaded.Update(config)` 像 rutis 一样修改一行的配置，也就是重启这一行：先卸载，再用新配置装载，监听器重新登记，排在其他监听器之后（rutis 不给 Go 的行发 `rows.update`，配置变了就重启；清理运行时这一行已经不在，清理里发出的事件会失败）；`loaded.Reported()` 是运行时报告的 `Emit` 监听器失败。上面的 `greeter` 的测试：

```go
func TestEvents(t *testing.T) {
	loaded := rutistest.Load(t, Plugin, struct{}{}, nil)
	said := make(chan any, 1)
	loaded.On("greeting/said", func(_ context.Context, args ...any) (any, error) {
		said <- args[0] // 测试一侧像宿主一样监听
		return nil, nil
	})
	var text string
	if err := loaded.Service("greeter").Call(context.Background(), "hello", &text, "Ada", "sv"); err != nil {
		t.Fatal(err)
	}
	if text != "Hej, Ada!" || <-said != "Ada" {
		t.Fatalf("got %q", text)
	}
	word, err := loaded.Serial(context.Background(), "greeting/pick", "en") // 测试自己发出
	if err != nil || word != nil {
		t.Fatalf("got %v, %v", word, err)
	}
}
```

`Load` 会检查宿主会检查的事：插件只用 `Inject` 里声明的服务；提供的服务是 `Provides` 声明的类型（没有声明的服务，像 rutis 一样按导出方法、都按 async 调用）；卸载时清理都成功。只在同一个进程里才成立的写法（例如把结构体当对象传、靠共享内存）在这里就会失败。

插件交给测试替身的定义（替身方法的参数是 `*rutis.Definition`）可以直接交给 `rutistest.Load`：它像 rutis 挂一份那样，在定义它的那个运行时里再加载一行，用的是插件 `Apply` 里准备好的东西；这次传的 `services` 是这一份自己的，像它所在实例的服务；它在 `Provides` 里声明的服务也只属于这一份。所以只提供声明过的服务时，同一个定义的几份可以同时加载；没声明的服务名提前不知道，不隔离。描述和报告里，副本的行用来源行的 id。定义属于交出它（作为参数、作为它提供的服务的返回值，或在它发出的事件里）的那一行：这一行卸载或被 `Update`（重启）时，这些副本先卸载，之后再 `Load` 这个定义会失败。副本的配置挂上后不变：在副本上调用 `Update` 或 `Load` 会失败。没有 `Name` 的定义也可以直接 `rutistest.Load`。

## 4. 在本地宿主里运行

```bash
rutis-host dev
```

`dev` 构建 `cmd/weather`，作为运行时 `go-weather` 运行它的每个插件；改了 `.go`、`go.mod`、`go.sum` 就重新构建并只重启这个运行时，构建失败时旧的继续运行。

`rutis.dev.json` 里，和插件 id 相同的那一行（这里是 `weather`）不写 `name`，它给插件本身加配置；其他行是一起运行的插件。插件需要的其他服务就写成这样的行，可以是 Python 或 TypeScript 插件，同时在 `runtimes` 里写上它们的语言。例如用 Python 写一个假的 `llm`，放在项目的 `dev/` 里：

```json
{
  "format": 1,
  "runtimes": { "py": { "project": "dev" } },
  "rows": [
    { "id": "weather", "config": { "city": "Oslo" } },
    { "id": "llm", "name": "py:fake_llm" }
  ]
}
```

```python
# dev/fake_llm.py
from rutis import define_plugin


class FakeLlm:
    async def ask(self, question):
        return "sunny"


def apply(ctx, config):
    ctx.provide("llm", FakeLlm())


plugin = define_plugin(apply, provides={"llm": FakeLlm})
```

```bash
uv venv dev/.venv                                  # Python 运行时的环境：<project>/.venv
uv pip install --python dev/.venv/bin/python "rutis==0.17.2"   # 和你的 rutis-host 同版本
```

Python 运行时在 `runtimes.py.project`（这里是 `dev`）的 `.venv` 里找解释器，那个环境里要装 `rutis`；`dev/` 下没有 Go 代码，`go build ./...` 不受影响。用 TypeScript 写也一样：`"runtimes": { "node": { "project": "dev" } }`，在 `dev/` 里 `npm install @arcships/rutis`，行名写 `./dev/fake-llm.ts`。

这些行的文件变化时只重新加载这些行，不重新构建，Go 运行时也不为此重启；`rutis.dev.json` 改了也在运行中生效（见 [用 dev 开发插件](rutis-host.md#用-dev-开发插件)）。`rutis-host check` 列出每一行和每个 Go 二进制的插件、版本和插件 API。

## 5. 发布

推一个 `v0.1.0` 这样的 tag：模板里的工作流运行测试和 `rutis-host check`，为 Linux、macOS（x64 / arm64）和 Windows x64 构建二进制，连同 `SHA256SUMS` 发布到 GitHub Release。

想把你的插件编进自己二进制的人，直接导入你的包：`rutis.Serve(weather.Plugin, other.Plugin)`。

## 6. 被宿主使用

宿主在 `rutis.json` 里给出 Go 插件目录，把二进制放进去：

```json
{
  "runtimes": { "go": { "dir": "plugins/go" } },
  "rows": [{ "id": "weather", "name": "go:weather", "config": { "city": "Oslo" } }]
}
```

```bash
rutis-host go add example.com/weather/cmd/weather@v0.1.0   # 用本机的 Go 工具链 go install
```

也可以下载发布里对应平台的二进制放进目录（macOS 上下载的二进制带隔离属性，`rutis-host check` 会提示）。行名 `go:<插件名>` 由宿主在所有二进制里查找；两个二进制里有同名插件时，写成 `go-<二进制名>:<插件名>`。目录里的二进制在第一次有行用到时才启动，空闲 60 秒后停下（`"start": "eager"` 全部启动并一直运行）；换了二进制要重启宿主才生效。见 [rutis-host 与 rutis.json](rutis-host.md)；Rust 宿主见 [在 Rust 应用里嵌入](rust-host.md)（`GoResolver`、`GoRuntimes`）。

## 7. 在另一台机器上运行

插件二进制也能在别的机器上作为远程运行时运行，`language` 写 `go`。两种方式，参数与 Python 运行时相同，见 [连接节点](nodes.md#别的机器上的语言运行时)：

```bash
# 监听，由 rutis 来连（证书与私钥在 RUTIS_CERT、RUTIS_KEY）
RUTIS_TOKEN=secret RUTIS_CERT=server.pem RUTIS_KEY=server.key \
  ./weather listen:wss://0.0.0.0:7443/rutis --id gpu --peer main .

# 拨出，连到 rutis 的接入点，断开后按固定规则重连
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  ./weather dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

拨出时状态写在 stderr（`rutis: connected to …` 等），SIGTERM 或 Ctrl-C 卸载全部行后退出；接入点协议不兼容时以状态 1 退出。规则见 [连进来的运行时](nodes.md#连进来的运行时)。
