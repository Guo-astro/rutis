# Write a Go Plugin

From a new project to a release: a plugin `greeter` that provides the service `greeter` and uses a service `translate` that someone else provides. Needs Go 1.24 or later.

The `rutis-host` commands below (create the project, run it in a local host, call a service) run as `npx @arcships/rutis-host …`, which needs Node; or install it first, see [rutis-host: Installation](rutis-host.en.md#installation), and write `rutis-host …`.

## 1. Start a project

<!-- example: go-en run -->
```bash
npx @arcships/rutis-host new greeter --lang go
cd greeter
go mod tidy
go test ./...
```

A Go plugin is compiled into a binary, and a binary is one runtime process in the host.

The project:

| File | What it is |
| --- | --- |
| `plugin.go` | The plugin (package `greeter`, exports `Plugin`) |
| `plugin_test.go` | Its test, no host needed |
| `cmd/greeter/main.go` | The binary: `rutis.Serve(greeter.Plugin)` |
| `rutis.dev.json` | The rows when you run it locally (step 7) |
| `go.mod` | The module path (change it to yours); depends on `github.com/arcships/rutis/go/rutis` |
| `.github/workflows/release.yml` | On a `v*` tag, tests and publishes the binaries for each platform to a GitHub Release (step 8) |

## 2. Provide a service

The generated `plugin.go`:

<!-- example: go-en file=greeter/plugin.go -->
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

- `rutis.Define` defines the plugin; `Apply` runs when the plugin starts.
- `ctx.Provide("greeter", …)` provides the service `greeter`: the value's exported methods are the service's methods, written with a lower-case first letter in other languages (`Hello` is `hello`).
- `Provides` says which services the plugin provides and their methods, so the host knows without running the plugin.

## 3. Use a service

`greeter` translates its greeting with the `translate` service. Replace `plugin.go` with:

<!-- example: go-en file=greeter/plugin.go -->
```go
package greeter

import (
	"context"

	"github.com/arcships/rutis/go/rutis"
)

type Config struct {
	Greeting string `json:"greeting,omitempty" doc:"how to greet"`
}

// The service it uses: one function field per method (Text is text).
type Translate struct {
	Text func(ctx context.Context, text string) (string, error)
}

type Greeter struct {
	translate Translate
	greeting  string
}

func (g *Greeter) Hello(ctx context.Context, name string) (string, error) {
	word, err := g.translate.Text(ctx, g.greeting) // pass ctx on
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

- `Inject` lists the services it uses. The plugin starts once they are all ready, and stops when one of them goes away.
- `ctx.Use("translate", &translate)` fills a struct of function fields with the service: the field names are the method names, and with a lower-case first letter they are the names on the wire.
- The `ctx` a method receives carries the call chain: pass it on when you call another service; do not replace it with `context.Background()`.

The template's test fails now (it gives no `translate`); step 6 replaces it.

## 4. Configuration

`Config` is the configuration: its JSON Schema is derived from the type, field names from the `json` tags, descriptions from the `doc` tags.

- A field the row leaves out is the zero value, so defaults are given in `Apply`: above, an empty `greeting` is `Hello`.
- Before `Apply`, the configuration is validated against the Schema: a row whose configuration does not match fails with `config: …`, and `Apply` is not called.
- Where it comes from: in a test, the third argument of `rutistest.Load` (step 6); at run time, the `config` of the plugin's row in the host (`rutis.dev.json` in step 7).

A hand-written Schema, defaults with `default`, telling "left out" from "written empty": see [plugin API: Configuration](plugin-api.en.md#configuration).

## 5. Emit and listen to events

Each greeting, `greeter` emits the event `greeter/greeted` with the name; it also listens to that event itself to count the greetings, which a new method `count` returns. Replace `plugin.go` with:

<!-- example: go-en file=greeter/plugin.go -->
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

// The service it uses: one function field per method (Text is text).
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
	word, err := g.translate.Text(ctx, g.greeting) // pass ctx on
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

- `ctx.Emit(ctx, name, args...)` emits an event and returns at once, without waiting for the listeners; the `error` only says the event was refused. The first argument is the `ctx` the caller gave.
- `ctx.On(name, listener)` listens to an event. All the listeners in a host, in every language and process, are on one event bus: other plugins, in other languages too, receive `greeter/greeted` as well. Listeners go away when the plugin unloads.

The other ways to emit (waiting for the listeners' results, and so on): see [plugin API: Events](plugin-api.en.md#events).

## 6. Test

Replace `plugin_test.go` with:

<!-- example: go-en file=greeter/plugin_test.go -->
```go
package greeter

import (
	"context"
	"testing"

	"github.com/arcships/rutis/go/rutis/rutistest"
)

// The fake translate: a Go value whose exported methods are the service's.
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

<!-- example: go-en run -->
```bash
go test ./...
```

- `rutistest.Load(t, plugin, config, services)` loads the plugin: it runs in its real runtime and values go through a real session, with no host. The last argument gives the services the plugin uses, here a fake `translate`.
- `loaded.Service("greeter")` gets a service the plugin provides; `Call(ctx, method, &result, args...)` calls it.
- `loaded.On` listens to the plugin's events as a host would.
- `loaded.Unload()` unloads the plugin.
- `Load` also checks what a host checks: the configuration matches the Schema; the plugin uses only the services in `Inject`; the services it provides match `Provides`.

The service types a host gives (`rutis-host types`) come in TypeScript and Python only; a Go plugin writes the struct of function fields from the methods in the host's documentation. The rest of the testing tool: see [plugin API: The Go test tool](plugin-api.en.md#the-go-test-tool).

## 7. Run it locally and try it

`rutis-host dev` runs the project in a local host. Each plugin in a host is a **row**: `rutis.dev.json` lists the rows. `greeter` uses `translate`, so there are two:

<!-- example: go-en file=greeter/rutis.dev.json -->
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

- The row with the plugin's id (`greeter`) is the plugin itself, with no `name`; `config` is its configuration (step 4).
- The other row is a fake `translate`: `name` says which plugin the row runs, and `py:fake_translate` is the module `fake_translate` in the Python runtime. `runtimes.py.project` says the Python project is `dev/`.

Write fakes in another language `dev` runs (Python, TypeScript, Bun). Here it is Python, in the project's `dev/`:

<!-- example: go-en file=greeter/dev/fake_translate.py -->
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

The Python runtime finds its interpreter in `dev/.venv`, which needs `rutis` installed (with [uv](https://docs.astral.sh/uv/)). There is no Go code in `dev/`, so `go build ./...` is not affected:

<!-- example: go-en run -->
```bash
uv venv dev/.venv
uv pip install --python dev/.venv/bin/python rutis
```

In TypeScript instead: `"runtimes": { "node": { "project": "dev" } }`, `npm install @arcships/rutis` in `dev/`, and the row's `name` is `./dev/fake-translate.ts`.

Run it:

<!-- example: go-en run until="greeter: running" -->
```bash
npx @arcships/rutis-host dev
```

```text
greeter: building
greeter: built in 0.3s
rutis-host dev: running greeter; changes reload it (Ctrl-C ends)
```

`dev` builds `cmd/greeter` and runs it as the runtime `go-greeter`. Then each row prints `greeter: running`, `translate: running` as it comes up (in either order); `rutis-host dev: services:` gives the services there are now and the row providing each (`loader` is the host's own); it is printed once every row is running, waiting or failed, and again whenever the services change, before or after the rows' state lines:

```text
rutis-host dev: services: greeter (greeter), loader (host), translate (translate)
```

In another terminal, in the project directory, see the host's state:

```console
$ npx @arcships/rutis-host status
rows:
  greeter: running
  translate: running
services:
  greeter: greeter
  loader: host
  translate: translate
runtimes:
  go-greeter: running (pid 63269)
  py: running (pid 63219)
calls:
  none
```

Call the service once: each argument is a JSON value (strings in quotes), and the result prints as JSON; an error goes to standard error with exit status 1 ([rutis-host: Calling a service](rutis-host.en.md#calling-a-service)):

```console
$ npx @arcships/rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada!"
$ npx @arcships/rutis-host call greeter.count
1
```

Change the code: in `plugin.go`, change `"!"` to `"!!"` and save. `dev` rebuilds and restarts only the runtime `go-greeter`; when the build fails, the old one keeps running:

```text
greeter: building
greeter: built in 0.3s
go-greeter: rebuilt and restarted
rutis-host dev: services: loader (host), translate (translate)
rutis-host dev: services: greeter (greeter), loader (host), translate (translate)
```

(While it restarts, `greeter` is missing from the services.)

```console
$ npx @arcships/rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada!!"
```

A change to `dev/fake_translate.py` reloads only the row `translate`, without a rebuild; a change to `rutis.dev.json` takes effect while it runs too. What `fmt.Println` and `log` print shows in `dev`'s output. The other rules: see [rutis-host: Developing a plugin with dev](rutis-host.en.md#developing-a-plugin-with-dev).

## 8. Publish

- The package name: the module path in `go.mod`; change it to your repository, for example `github.com/you/greeter`, and the import in `cmd/greeter/main.go` with it.
- Push a tag like `v0.1.0`: the template's workflow runs the tests and `rutis-host check`, builds binaries for Linux (x64 / arm64), macOS (x64 / arm64) and Windows x64, and publishes them with `SHA256SUMS` to a GitHub Release.
- The dependency range: the `require github.com/arcships/rutis/go/rutis` in `go.mod` is the lowest version; a binary has the version chosen when it was built compiled in.

How a host installs your plugin: see [rutis-host: How a host installs plugins](rutis-host.en.md#how-a-host-installs-plugins). To run it on another machine, see [the remote section of rutis-host.en.md](rutis-host.en.md#remote-runtimes).

## Debugging

`--debug` builds without optimisation (`-gcflags=all=-N -l`) and starts the runtime under [Delve](https://github.com/go-delve/delve): Delve listens on `127.0.0.1:2345`, the program runs as usual, and you set breakpoints once connected. Install Delve first:

```bash
go install github.com/go-delve/delve/cmd/dlv@latest
```

```bash
npx @arcships/rutis-host dev --debug
```

In another terminal, connect, set a breakpoint on `Hello` and continue; the runtime stops when `Hello` is called:

```bash
dlv connect 127.0.0.1:2345
```

```
(dlv) break (*Greeter).Hello
(dlv) continue
```

Once stopped, `stack` shows the call stack, `locals` and `print <variable>` the variables.

- To disconnect and leave the runtime running: `exit -c`. A plain `exit` asks whether to end Delve: answer `n`.
- VS Code: in `launch.json`, `{ "type": "go", "request": "attach", "mode": "remote", "host": "127.0.0.1", "port": 2345 }`.
- Each rebuild starts the runtime and Delve again: connect again and set the breakpoints again.
- Delve's address is fixed: under `--debug`, one Go binary runs.
- A Python runtime that `rutis.dev.json` adds reads this terminal then too (see the [Python guide](python-plugin.en.md#debugging)), and Ctrl-C reaches it as well.
- While it is stopped at a breakpoint, the whole runtime waits: calls to the other plugins in this binary wait too.
- On macOS, the first debugging session asks to allow developer tools.
- Tests need no host: `dlv test .`.

## Go's rules

| Topic | Rule |
| --- | --- |
| Method names | An exported method's name on the wire has a lower-case first letter (`Hello` → `hello`, `URLFor` → `urlFor`, `HTTPServer` → `httpServer`); `advanced.Rename` changes it (package `github.com/arcships/rutis/go/rutis/advanced`) |
| Method signatures | `func (T) M([ctx context.Context,] args...) ([result,] [error])` |
| Sync or async | `async` by default; `rutis.MethodsOf[*T](rutis.Sync("M"))` marks sync methods. A sync method blocks a Node caller's event loop: keep methods doing I/O `async` |
| `ctx` | `*rutis.Ctx` is a `context.Context`, cancelled when the plugin unloads; goroutines the plugin starts use `ctx.Go(...)` or watch `ctx.Done()`; cleanups at unload go in `ctx.Effect(...)` |
| The call chain | When Node calls your method synchronously and your method calls back into Node synchronously, Node recognises the call chain it is waiting on only if you pass on the ctx you received; with `context.Background()` both sides wait for each other |
| Concurrency | Each incoming call runs on its own goroutine: provided objects must allow concurrent calls; do not hold a lock while calling another service |
| Values | Data (numbers, strings, slices, maps, structs) is copied, field names from the `json` tags; `time.Time`, `[]byte`, `*big.Int`, maps with non-string keys, `map[K]struct{}` (Set) and `advanced.DateTime`, `advanced.Date`, `advanced.TimeOfDay`, `advanced.Decimal`, `advanced.UUID` pass as their own values; integers beyond ±(2^53−1) pass as big integers; functions pass by reference; to pass an object by reference, write `rutis.Ref(v)`. See [Passing values](plugin-api.en.md#passing-values) |
| Errors | A returned `error` crosses processes with its type name (`*QuotaError` → `QuotaError`; implement `Name() string` to choose it); with `Code() string` and `Data() any` it carries a code and data; a panic becomes an error named `Panic`. A received error is a `*rutis.RemoteError` with `Name`, `Message`, `Code`, `Data` |
| Several plugins in a binary | `rutis.Serve(greeter.Plugin, other.Plugin)`: whoever wants your plugin in their own binary imports your package |

## Advanced: plugin API

Plugins as values, the other ways to emit, scopes, filters, objects with methods, the rest of the testing tool: see the [plugin API](plugin-api.en.md).
