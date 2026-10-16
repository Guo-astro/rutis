# 写一个 Go 插件

从创建项目到发布，再到被宿主使用。需要 Go 1.24 或更高。

Go 插件和 TypeScript、Python 插件的写法与规则相同（见 [插件 API](plugin-api.md)），不同的是分发方式：**Go 插件编译成二进制，一个二进制就是宿主里的一个运行时进程**。一个二进制可以带一个或几个插件；宿主可以同时运行很多个二进制，只在有行用到时才启动它。

## 1. 创建项目

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
	Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Unit"))}, // 方法默认 async
	Apply: func(ctx *rutis.Ctx, config Config) error {
		var llm LLM
		if err := ctx.Use("llm", &llm); err != nil {
			return err
		}
		ctx.Provide("weather", &Weather{llm: llm, city: config.City})
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
| 使用服务 | 函数字段的结构体可以只写用到的方法；或用 `*rutis.Service` 按名字调用：`svc.Call(ctx, "ask", &answer, "q")` |
| `ctx` | `*rutis.Ctx` 是一个 `context.Context`，插件卸载时取消；插件自己起的 goroutine 用 `ctx.Go(...)` 或看 `ctx.Done()` |
| 并发 | 每个进来的调用在自己的 goroutine 上执行：提供的对象要能被并发调用，调用别的服务时不要持有锁 |
| 值 | 数据（数字、字符串、切片、map、结构体）复制，结构体字段与切片、map 的元素逐个检查（字段名按 `json` 标签）；`time.Time`（时间点，UTC）、`[]byte`、`*big.Int`、键不是字符串的 map、`map[K]struct{}`（Set）与 `rutis.DateTime`、`rutis.Date`、`rutis.TimeOfDay`、`rutis.Decimal`、`rutis.UUID` 按各自的值传，见[值怎样传递](plugin-api.md#值怎样传递)；超出 ±(2^53−1) 的整数（`int64`、`uint64` 等）自动按大整数传，收到的大整数解码进声明的整数类型，到 `any` 时是 `*big.Int`；函数按引用；对象要按引用传时写 `rutis.Ref(v)` |
| 错误 | 返回的 `error` 带着类型名跨进程（`*QuotaError` → `QuotaError`，实现 `Name() string` 可以自定）；panic 变成名为 `Panic` 的错误 |

**一定把 ctx 传下去。** 调用链（`path`）放在 `context.Context` 里：Node 同步调用你的方法、你的方法又同步回调 Node 时，只有带着收到的 ctx，Node 才认得出这是它正在等的调用链；换成 `context.Background()` 两边会互相等。`rutis-host dev` 下，丢了调用链的调用会在 stderr 警告一次。

## 3. 测试

`rutistest` 不需要宿主，但插件在它真正的运行时里跑、值经过真正的会话传递：

```go
func TestToday(t *testing.T) {
	loaded := rutistest.Load(t, Plugin, Config{City: "Oslo"}, map[string]any{"llm": fakeLLM{}})
	var today string
	if err := loaded.Service("weather").Call(context.Background(), "today", &today); err != nil {
		t.Fatal(err)
	}
}
```

```bash
go test ./...
```

`Load` 会检查宿主会检查的事：插件只用 `Inject` 里声明的服务；提供的服务是 `Provides` 声明的类型；卸载时清理都成功。只在同一个进程里才成立的写法（例如把结构体当对象传、靠共享内存）在这里就会失败。

## 4. 在本地宿主里运行

```bash
rutis-host dev
```

`dev` 构建 `cmd/weather`，作为运行时 `go-weather` 运行它的每个插件；改了 `.go`、`go.mod`、`go.sum` 就重新构建并只重启这个运行时，构建失败时旧的继续运行。插件需要的其他服务写在 `rutis.dev.json` 里，可以是 Python 或 TypeScript 插件（同时写上对应的 `runtimes`）。`rutis-host check` 列出每一行和每个 Go 二进制的插件、版本和插件 API。

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
