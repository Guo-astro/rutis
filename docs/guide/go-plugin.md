# 写一个 Go 插件

从建项目到发布：写一个 `greeter` 插件，它提供服务 `greeter`，用到别人提供的服务 `translate`。需要 Go 1.24 或更高。

下面的 `rutis-host`（建项目、在本地宿主里运行、调用服务）用 `npx @arcships/rutis-host …` 运行，要有 Node；也可以先装好，见 [rutis-host：安装](rutis-host.md#安装)，之后命令写成 `rutis-host …`。测试（`go test`）需要装好的 `rutis-host`：在 `PATH` 上，或由 `RUTIS_HOST` 给出路径；第一段命令用 npm 装上它。

## 1. 起项目

<!-- example: go run -->
```bash
npm install --global @arcships/rutis-host
npx @arcships/rutis-host new greeter --lang go
cd greeter
go mod tidy
go test ./...
```

Go 插件编译成二进制，一个二进制就是宿主里的一个运行时进程。

得到的项目：

| 文件 | 作用 |
| --- | --- |
| `plugin.go` | 插件本身（包 `greeter`，导出 `Plugin`） |
| `plugin_test.go` | 测试（`go test` 需要 `rutis-host`） |
| `cmd/greeter/main.go` | 二进制：`rutis.Serve(greeter.Plugin)` |
| `rutis.dev.json` | 在本地运行时有哪些行（第 7 步） |
| `go.mod` | 模块路径（改成你自己的）；依赖 `github.com/arcships/rutis/go/rutis` |
| `.github/workflows/release.yml` | 打 `v*` tag 时测试，并把各平台的二进制发布到 GitHub Release（第 8 步） |

## 2. 提供服务

生成的 `plugin.go`：

<!-- example: go file=greeter/plugin.go -->
```go
// Package greeter is a rutis plugin. The Go plugin guide
// (docs/guide/go-plugin.en.md) takes it on from here: step 2 explains this
// file, step 3 adds a service it uses.
package greeter

import (
	"github.com/arcships/rutis/go/rutis"
)

// Config is the plugin's configuration; its JSON Schema is derived from it
// (step 4 of the guide).
type Config struct {
	Greeting string `json:"greeting,omitempty" doc:"how to greet"`
}

// Greeter is the service the plugin provides: its exported methods are the
// service's methods (Hello is `hello` to other languages).
type Greeter struct {
	greeting string
}

func (g *Greeter) Hello(name string) string { return g.greeting + ", " + name + "!" }

var Plugin = rutis.Define(rutis.Plugin[Config]{
	Name: "greeter",
	// Services this plugin uses: Inject: []string{"translate"}, then
	// ctx.Use("translate", &translate) (step 3 of the guide).
	Inject: []string{},
	// The services it provides, and their methods.
	Provides: rutis.Provides{"greeter": rutis.MethodsOf[*Greeter]()},
	Apply: func(ctx *rutis.Ctx, config Config) error {
		greeting := config.Greeting
		if greeting == "" {
			greeting = "Hello" // a field the row leaves out is the zero value
		}
		ctx.Provide("greeter", &Greeter{greeting: greeting})
		return nil
	},
})
```

- `rutis.Define` 定义插件；插件启动时调用 `Apply`。
- `ctx.Provide("greeter", …)` 提供服务 `greeter`：值的导出方法就是服务的方法，别的语言里写成首字母小写（`Hello` 是 `hello`）。
- `Provides` 写出插件提供哪些服务、有哪些方法，宿主不运行插件就知道。

## 3. 使用服务

`greeter` 用 `translate` 服务把问候语翻译过来。`plugin.go` 换成：

<!-- example: go file=greeter/plugin.go -->
```go
package greeter

import (
	"context"

	"github.com/arcships/rutis/go/rutis"
)

type Config struct {
	Greeting string `json:"greeting,omitempty" doc:"how to greet"`
}

// 用到的服务：一个方法一个函数字段（Text 是 text）。
type Translate struct {
	Text func(ctx context.Context, text string) (string, error)
}

type Greeter struct {
	translate Translate
	greeting  string
}

func (g *Greeter) Hello(ctx context.Context, name string) (string, error) {
	word, err := g.translate.Text(ctx, g.greeting) // 把 ctx 传下去
	if err != nil {
		return "", err
	}
	return word + ", " + name + "!", nil
}

var Plugin = rutis.Define(rutis.Plugin[Config]{
	Name:     "greeter",
	Inject:   []string{"translate"},
	Provides: rutis.Provides{"greeter": rutis.MethodsOf[*Greeter]()},
	Apply: func(ctx *rutis.Ctx, config Config) error {
		var translate Translate
		if err := ctx.Use("translate", &translate); err != nil {
			return err
		}
		greeting := config.Greeting
		if greeting == "" {
			greeting = "Hello"
		}
		ctx.Provide("greeter", &Greeter{translate: translate, greeting: greeting})
		return nil
	},
})
```

- `Inject` 写出用到的服务。这些服务都就绪了插件才启动，其中一个撤销时插件停下。
- `ctx.Use("translate", &translate)` 把服务填进函数字段的结构体：字段名是方法名，首字母小写后就是线上的名字。
- 方法收到的 `ctx` 带着调用链，调用别的服务时把它传下去，不要换成 `context.Background()`。

模板的测试这时不再通过（它没有给 `translate`），第 6 步换掉它。

## 4. 配置

`Config` 就是配置：它的 JSON Schema 由这个类型生成，字段名按 `json` 标签，`doc` 标签是说明。

- 行里没写的字段是零值，默认值在 `Apply` 里给：上面 `greeting` 为空时是 `Hello`。
- `Apply` 之前，配置先按 Schema 校验：不合格的行以 `config: …` 失败，不调用 `Apply`。
- 配置从哪里来：测试里是 `rutistest.Load` 的第三个参数（第 6 步）；运行时来自宿主里这一行的 `config`（第 7 步的 `rutis.dev.json`）。

手写 Schema、用 `default` 给默认值、分清「没写」和「写了空值」，见 [插件 API：配置](plugin-api.md#配置)。

## 5. 发出和监听事件

每次打招呼，`greeter` 发出事件 `greeter/greeted`，带上名字；它自己也监听这个事件，记下打过几次招呼，由新方法 `count` 返回。`plugin.go` 换成：

<!-- example: go file=greeter/plugin.go -->
```go
package greeter

import (
	"context"
	"sync/atomic"

	"github.com/arcships/rutis/go/rutis"
)

type Config struct {
	Greeting string `json:"greeting,omitempty" doc:"how to greet"`
}

// 用到的服务：一个方法一个函数字段（Text 是 text）。
type Translate struct {
	Text func(ctx context.Context, text string) (string, error)
}

type Greeter struct {
	plugin    *rutis.Ctx
	translate Translate
	greeting  string
	greeted   atomic.Int64
}

func (g *Greeter) Hello(ctx context.Context, name string) (string, error) {
	word, err := g.translate.Text(ctx, g.greeting) // 把 ctx 传下去
	if err != nil {
		return "", err
	}
	if err := g.plugin.Emit(ctx, "greeter/greeted", name); err != nil {
		return "", err
	}
	return word + ", " + name + "!", nil
}

func (g *Greeter) Count() int64 { return g.greeted.Load() }

var Plugin = rutis.Define(rutis.Plugin[Config]{
	Name:     "greeter",
	Inject:   []string{"translate"},
	Provides: rutis.Provides{"greeter": rutis.MethodsOf[*Greeter]()},
	Apply: func(ctx *rutis.Ctx, config Config) error {
		var translate Translate
		if err := ctx.Use("translate", &translate); err != nil {
			return err
		}
		greeting := config.Greeting
		if greeting == "" {
			greeting = "Hello"
		}
		greeter := &Greeter{plugin: ctx, translate: translate, greeting: greeting}
		ctx.On("greeter/greeted", func(context.Context, ...any) (any, error) {
			greeter.greeted.Add(1)
			return nil, nil
		})
		ctx.Provide("greeter", greeter)
		return nil
	},
})
```

- `ctx.Emit(ctx, 名字, 参数...)` 发出事件，立即返回，不等监听器；返回的 `error` 只表示事件被拒绝。第一个参数是调用方给的 `ctx`。
- `ctx.On(名字, 监听器)` 监听事件。一个宿主里所有语言、所有进程的监听器都在同一条事件总线上：别的插件、别的语言写的插件也收得到 `greeter/greeted`。监听器在插件卸载时自动撤下。

其余发出方式（等监听器的结果等）见 [插件 API：事件](plugin-api.md#事件)。

## 6. 测试

`plugin_test.go` 换成：

<!-- example: go file=greeter/plugin_test.go -->
```go
package greeter

import (
	"context"
	"testing"

	"github.com/arcships/rutis/go/rutis/rutistest"
)

// 假的 translate：一个 Go 值，导出的方法就是服务的方法。
type fakeTranslate struct{}

func (fakeTranslate) Text(text string) string {
	words := map[string]string{"Hello": "Bonjour", "Hi": "Salut"}
	if word, ok := words[text]; ok {
		return word
	}
	return text
}

func TestHello(t *testing.T) {
	loaded := rutistest.Load(t, Plugin, Config{Greeting: "Hi"}, map[string]any{"translate": fakeTranslate{}})
	greeted := make(chan []any, 1)
	loaded.On("greeter/greeted", func(_ context.Context, args ...any) (any, error) {
		greeted <- args
		return nil, nil
	})

	greeter := loaded.Service("greeter")
	var text string
	if err := greeter.Call(context.Background(), "hello", &text, "Ada"); err != nil {
		t.Fatal(err)
	}
	if text != "Salut, Ada!" {
		t.Fatalf("hello: got %q", text)
	}
	if args := <-greeted; len(args) != 1 || args[0] != "Ada" {
		t.Fatalf("greeter/greeted: got %v", args)
	}
	var count int64
	if err := greeter.Call(context.Background(), "count", &count); err != nil {
		t.Fatal(err)
	}
	if count != 1 {
		t.Fatalf("count: got %d", count)
	}
	loaded.Unload()
}
```

<!-- example: go run -->
```bash
go test ./...
```

- `go test` 需要 `rutis-host`：在 `PATH` 上，或由 `RUTIS_HOST` 给出路径。测试进程跑 Go 运行时，并启动一次 `rutis-host test`；每次 `Load` 都是这个宿主的一行。
- `rutistest.Load(t, 插件, 配置, 服务)` 把插件作为一行加载：插件在它真正的运行时里、真正的 rutis 内核下运行。最后一个参数给出插件用到的服务，这里是假的 `translate`：由测试自己的一个插件提供。
- `loaded.Service("greeter")` 取插件提供的服务，`Call(ctx, 方法, &结果, 参数...)` 调用它。
- `loaded.On` 监听插件发出的事件；监听的是测试自己的插件，在宿主的总线上。
- `loaded.Unload()` 卸载插件；测试结束时这一行也会卸掉。
- 宿主对这一行做它对每一行都做的检查：配置合 Schema（`TryLoad` 交回它的那一行，`weather: config: city is required`），`Inject` 里的服务都在。

宿主给的服务类型（`rutis-host types`）只有 TypeScript 与 Python 两种；Go 插件照宿主文档里的方法写函数字段的结构体。测试工具的其余接口见 [插件 API：Go 的测试工具](plugin-api.md#go-的测试工具)。

## 7. 本地运行并试一下

`rutis-host dev` 把项目放进一个本地宿主里运行。宿主里每个插件占一**行**：`rutis.dev.json` 写出有哪些行。`greeter` 要用 `translate`，所以这里有两行：

<!-- example: go file=greeter/rutis.dev.json -->
```json
{
  "format": 1,
  "runtimes": { "py": { "project": "dev" } },
  "rows": [
    { "id": "greeter", "config": { "greeting": "Hello" } },
    { "id": "translate", "name": "py:fake_translate" }
  ]
}
```

- 和插件同 id 的那一行（`greeter`）就是插件自己，不写 `name`；`config` 是它的配置（第 4 步）。
- 另一行是假的 `translate`：`name` 写这一行运行哪个插件，`py:fake_translate` 是 Python 运行时里的模块 `fake_translate`。`runtimes.py.project` 说 Python 项目在 `dev/`。

假的服务用 `dev` 能运行的另一种语言写（Python、TypeScript、Bun）。这里用 Python，放在项目的 `dev/` 里：

<!-- example: go file=greeter/dev/fake_translate.py -->
```python
from rutis import define_plugin

WORDS = {"Hello": "Bonjour", "Hi": "Salut"}


class FakeTranslate:
    async def text(self, text):
        return WORDS.get(text, text)


def apply(ctx, config):
    ctx.provide("translate", FakeTranslate())


plugin = define_plugin(apply, provides={"translate": FakeTranslate})
```

Python 运行时在 `dev/.venv` 里找解释器，那里要装 `rutis`（要有 [uv](https://docs.astral.sh/uv/)）。`dev/` 里没有 Go 代码，`go build ./...` 不受影响：

<!-- example: go run -->
```bash
uv venv dev/.venv
uv pip install --python dev/.venv/bin/python rutis
```

用 TypeScript 写也可以：`"runtimes": { "node": { "project": "dev" } }`，在 `dev/` 里 `npm install @arcships/rutis`，行的 `name` 写 `./dev/fake-translate.ts`。

运行：

<!-- example: go run until="greeter: running" -->
```bash
npx @arcships/rutis-host dev
```

```text
greeter: building
greeter: built in 0.3s
rutis-host dev: running greeter; changes reload it (Ctrl-C ends)
```

`dev` 构建 `cmd/greeter`，作为运行时 `go-greeter` 运行它。之后每一行起来时打印 `greeter: running`、`translate: running`（两行谁先谁后不一定）；`rutis-host dev: services:` 是现在的服务、各由哪一行提供（`loader` 是宿主自己的），这一行在所有行都已运行、等待或失败后打印一次，之后服务变化时再打印，和各行状态行的先后不固定：

```text
rutis-host dev: services: greeter (greeter), loader (host), sessions (host), translate (translate)
```

另开一个终端，在项目目录里看宿主现在的状态：

```console
$ npx @arcships/rutis-host status
rows:
  greeter: running
  translate: running
services:
  greeter: greeter
  loader: host
  sessions: host
  translate: translate
runtimes:
  go-greeter: running (pid 63269)
  py: running (pid 63219)
calls:
  none
```

调用一次服务：每个参数是一个 JSON 值（字符串带引号），结果打印成 JSON；出错时写到标准错误并以 1 退出（[rutis-host：调用一个服务](rutis-host.md#调用一个服务)）：

```console
$ npx @arcships/rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada!"
$ npx @arcships/rutis-host call greeter.count
1
```

改代码：把 `plugin.go` 里的 `"!"` 改成 `"!!"` 并保存。`dev` 重新构建，只重启 `go-greeter` 这个运行时；构建失败时旧的继续运行：

```text
greeter: building
greeter: built in 0.3s
go-greeter: rebuilt and restarted
rutis-host dev: services: loader (host), sessions (host), translate (translate)
rutis-host dev: services: greeter (greeter), loader (host), sessions (host), translate (translate)
```

（重启的那一会儿，服务列表里没有 `greeter`。）

```console
$ npx @arcships/rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada!!"
```

改 `dev/fake_translate.py` 只重新加载 `translate` 这一行，不重新构建；改 `rutis.dev.json` 也在运行中生效。`fmt.Println`、`log` 的内容出现在 `dev` 的输出里。其余规则见 [rutis-host：用 dev 开发插件](rutis-host.md#用-dev-开发插件)。

## 8. 发布

- 包名：`go.mod` 的模块路径，改成你的仓库，例如 `github.com/you/greeter`；`cmd/greeter/main.go` 的导入随之改。
- 推一个 `v0.1.0` 这样的 tag：模板里的工作流运行测试和 `rutis-host check`，为 Linux（x64 / arm64）、macOS（x64 / arm64）和 Windows x64 构建二进制，连同 `SHA256SUMS` 发布到 GitHub Release。
- 依赖范围：`go.mod` 的 `require github.com/arcships/rutis/go/rutis` 是最低版本；二进制里编进的是构建时选定的那个版本。

宿主怎样装你的插件见 [rutis-host：宿主怎样装插件](rutis-host.md#宿主怎样装插件)。在另一台机器上运行见 [rutis-host.md 的远程一节](rutis-host.md#远程运行时与设备)。

## 调试

`--debug` 不优化地构建（`-gcflags=all=-N -l`），在 [Delve](https://github.com/go-delve/delve) 下启动运行时：Delve 监听 `127.0.0.1:2345`，程序照常运行，连上后再下断点。先装 Delve：

```bash
go install github.com/go-delve/delve/cmd/dlv@latest
```

```bash
npx @arcships/rutis-host dev --debug
```

在另一个终端连上，在 `Hello` 上下断点，继续运行；有调用到 `Hello` 时停住：

```bash
dlv connect 127.0.0.1:2345
```

```
(dlv) break (*Greeter).Hello
(dlv) continue
```

停住后 `stack` 看调用栈、`locals` 和 `print <变量>` 看变量。

- 断开而让运行时继续：`exit -c`。直接 `exit` 会问是否结束 Delve，选 `n`。
- VS Code：`launch.json` 里写 `{ "type": "go", "request": "attach", "mode": "remote", "host": "127.0.0.1", "port": 2345 }`。
- 每次重新构建，运行时连同 Delve 重新启动：重新连接，重新下断点。
- Delve 的地址固定，`--debug` 下只能运行一个 Go 二进制。
- `rutis.dev.json` 加的 Python 运行时这时也读这个终端（见 [Python 指南](python-plugin.md#调试)），Ctrl-C 也送到它。
- 停在断点时整个运行时停着：这个二进制里其他插件的调用也等它继续。
- macOS 上第一次调试时，系统会请求允许开发者工具。
- 测试：`dlv test .`（`rutis-host` 在 `PATH` 上，或设 `RUTIS_HOST`）。

## Go 的规则

| 事项 | 规则 |
| --- | --- |
| 方法名 | 导出方法的线上名字是首字母小写的形式（`Hello` → `hello`，`URLFor` → `urlFor`，`HTTPServer` → `httpServer`）；`advanced.Rename` 可以改（包 `github.com/arcships/rutis/go/rutis/advanced`） |
| 方法签名 | `func (T) M([ctx context.Context,] 参数...) ([结果,] [error])` |
| 同步还是异步 | 默认 `async`；`rutis.MethodsOf[*T](rutis.Sync("M"))` 标出同步方法。同步方法会让 Node 调用方阻塞事件循环，做 I/O 的方法保持 `async` |
| `ctx` | `*rutis.Ctx` 是一个 `context.Context`，插件卸载时取消；插件自己起的 goroutine 用 `ctx.Go(...)` 或看 `ctx.Done()`；要在卸载时清理，用 `ctx.Effect(...)` |
| 调用链 | Node 同步调用你的方法、你的方法又同步回调 Node 时，只有带着收到的 ctx，Node 才认得出这是它正在等的调用链；换成 `context.Background()` 两边会互相等 |
| 并发 | 每个进来的调用在自己的 goroutine 上执行：提供的对象要能被并发调用，调用别的服务时不要持有锁 |
| 值 | 数据（数字、字符串、切片、map、结构体）复制，字段名按 `json` 标签；`time.Time`、`[]byte`、`*big.Int`、键不是字符串的 map、`map[K]struct{}`（Set）与 `advanced.DateTime`、`advanced.Date`、`advanced.TimeOfDay`、`advanced.Decimal`、`advanced.UUID` 按各自的值传；超出 ±(2^53−1) 的整数自动按大整数传；函数按引用；对象要按引用传时写 `rutis.Ref(v)`。见 [值怎样传递](plugin-api.md#值怎样传递) |
| 错误 | 返回的 `error` 带着类型名跨进程（`*QuotaError` → `QuotaError`，实现 `Name() string` 可以自定）；实现 `Code() string` 和 `Data() any` 时带上 code 和 data；panic 变成名为 `Panic` 的错误。收到的错误是 `*rutis.RemoteError`，有 `Name`、`Message`、`Code`、`Data` |
| 一个二进制几个插件 | `rutis.Serve(greeter.Plugin, other.Plugin)`：想把你的插件编进自己二进制的人直接导入你的包 |

## 进阶：插件 API

插件作为值、其余事件方式、作用域、过滤、带方法的对象、测试工具的其余接口：见 [插件 API](plugin-api.md)。
