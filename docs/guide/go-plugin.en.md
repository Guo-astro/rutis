# Write a Go Plugin

From creating a project to publishing it, and to a host using it. Needs Go 1.24 or later.

Go plugins follow the same conventions as TypeScript and Python plugins (see the [plugin API](plugin-api.en.md)); what differs is how they are distributed: **a Go plugin is compiled into a binary, and a binary is one runtime process in the host**. A binary holds one or several plugins; a host can run many binaries at once, starting each only when a row uses it.

## 1. Create the project

The `rutis-host` below (to create the project, run it in a local host, check it) needs installing first, in one of the ways in [rutis-host: installation](rutis-host.en.md#installation): for example `npm install -g @arcships/rutis-host`, or the binary from GitHub Releases on your `PATH`; without installing, write each command as `npx @arcships/rutis-host …`.

```bash
rutis-host new weather --lang go
cd weather
go mod tidy
```

The project:

| File | What it is |
| --- | --- |
| `plugin.go` | The plugin (package `weather`, exporting `Plugin`) |
| `plugin_test.go` | Unit tests, no host needed |
| `cmd/weather/main.go` | The binary: `rutis.Serve(weather.Plugin)` |
| `rutis.dev.json` | Configuration of the local runtime and other plugins for testing |
| `go.mod` | The module path (change it to yours); depends on `github.com/arcships/rutis/go/rutis` |
| `.github/workflows/release.yml` | On a `v*` tag: tests, then binaries for every platform on a GitHub release |

## 2. Write the plugin

The generated `plugin.go` is a `greeter` that uses no service, and `plugin_test.go` is its test. Here it becomes a `weather` that uses an `llm` service: replace `plugin.go` with the code below, and `plugin_test.go` with the one in §3 at the same time (the two files go together).

```go
package weather

import (
	"context"

	"github.com/arcships/rutis/go/rutis"
)

// The configuration: the SDK derives its JSON Schema from this type.
type Config struct {
	City string `json:"city,omitempty" doc:"the city to report on"`
}

// A service it uses: a struct of function fields, named after the methods (Ask -> ask).
type LLM struct {
	Ask func(ctx context.Context, question string) (string, error)
}

// The service it provides: its exported methods are the service's methods.
type Weather struct {
	llm  LLM
	city string
}

func (w *Weather) Today(ctx context.Context) (string, error) {
	answer, err := w.llm.Ask(ctx, "weather in "+w.city) // pass ctx on
	if err != nil {
		return "", err
	}
	return answer + " in " + w.city, nil
}

func (w *Weather) Unit() string { return "celsius" }

var Plugin = rutis.Define(rutis.Plugin[Config]{
	Name:     "weather",
	Inject:   []string{"llm"},                                                       // runs only while all are there
	Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Unit"))}, // declares what it provides (check knows before running); Unit sync, the rest async
	Apply: func(ctx *rutis.Ctx, config Config) error {
		var llm LLM
		if err := ctx.Use("llm", &llm); err != nil {
			return err
		}
		city := config.City
		if city == "" {
			city = "Oslo" // a field the row leaves out is its zero value: the default goes here
		}
		ctx.Provide("weather", &Weather{llm: llm, city: city})
		ctx.Effect(func(context.Context) error { return nil }) // a cleanup, optional
		return nil
	},
})
```

In short (`advanced.*` is package `github.com/arcships/rutis/go/rutis/advanced`: the names a plugin rarely writes, such as value types, services called by name, and the options and errors of events):

| Topic | Rule |
| --- | --- |
| Method names | An exported method's wire name has its first letter lowercased (`Today` → `today`, `URLFor` → `urlFor`, `HTTPServer` → `httpServer`); `advanced.Rename` overrides it |
| Sync or async | `async` by default; `rutis.Sync("Unit")` marks synchronous ones. A sync method blocks a Node caller's event loop: keep methods that do I/O `async` |
| Signatures | `func (T) M([ctx context.Context,] params...) ([result,] [error])` |
| Config | The JSON Schema is derived from the config type (field names follow `json` tags, a `doc` tag is the description), without `required`: a field the row's `config` leaves out is the zero value, so defaults go in `Apply` (`city` above); to tell "left out" from "given empty", use a pointer field (`*string`, which also takes `null`). Before `Apply` the config is validated against the schema, then decoded: a row whose config does not match fails with `config: …`, and `Apply` is not called ([plugin API: configuration](plugin-api.en.md#configuration)); a decoding error after the schema passed is written `<path> is not a value the plugin accepts`. The `Schema` field takes a hand-written JSON Schema instead: its `default`s are filled in before decoding |
| Using services | A struct of function fields may list only the methods it uses; or call by name with `*advanced.Service`: `svc.Call(ctx, "ask", &answer, "q")` |
| `ctx` | `*rutis.Ctx` is a `context.Context` cancelled when the plugin unloads; goroutines the plugin starts use `ctx.Go(...)` or watch `ctx.Done()` |
| Concurrency | Every incoming call runs on its own goroutine: provided objects must be safe for concurrent use; do not hold a lock while calling another service |
| Values | Data (numbers, strings, slices, maps, structs) is copied, struct fields and the items of slices and maps checked one by one (field names follow `json` tags); `time.Time` (an instant, in UTC), `[]byte`, `*big.Int`, maps whose keys are not strings, `map[K]struct{}` (a set) and `advanced.DateTime`, `advanced.Date`, `advanced.TimeOfDay`, `advanced.Decimal`, `advanced.UUID` cross as their own values, see [Passing values](plugin-api.en.md#passing-values); integers beyond ±(2^53−1) (`int64`, `uint64`, ...) cross as big integers by themselves, and a big integer received decodes into the declared integer type, or `*big.Int` into `any`; functions cross by reference; an object crosses by reference only as `rutis.Ref(v)` |
| Errors | A returned `error` crosses with its type name (`*QuotaError` → `QuotaError`; `Name() string` overrides); with `Code() string` and `Data() any` it carries a code and data, which the caller reads as its error's `code` and `data` (`Data` is encoded as JSON); a panic becomes an error named `Panic`. An error received is a `*rutis.RemoteError` with `Name`, `Message`, `Code` and `Data` |

**Always pass ctx on.** The call chain (`path`) lives in `context.Context`: when Node calls your method synchronously and your method calls back into Node synchronously, Node recognizes the chain it is waiting on only if the call carries the ctx you received; with `context.Background()` both sides wait for each other.

**A plugin is itself a value.** The result of `rutis.Define(...)` (a `*rutis.Definition`) can be handed to a service as an ordinary argument (an application's part registry, say), the function field's parameter typed `*rutis.Definition`; the receiver can mount it elsewhere, one copy per mount, each running in this process with what `Apply` prepared. A definition only handed over may leave out `Name`:

```go
var parts struct {
	Register func(ctx context.Context, scope string, def *rutis.Definition) error
}
if err := ctx.Use("parts", &parts); err != nil {
	return err
}
return parts.Register(ctx, "session", rutis.Define(rutis.Plugin[SessionConfig]{
	Inject: []string{"history"},
	Apply:  func(ctx *rutis.Ctx, config SessionConfig) error { /* use pool directly */ return nil },
}))
```

When this row unloads or reloads, every copy mounted from a definition it handed over ends first. See [Plugin API](plugin-api.en.md#plugin-definitions).

### Events

Every listener on a host, in any language and any process, is on one event bus ([design](../design-events-2026-10-18.en.md)): where an event is emitted and where its listeners are does not change the result.

```go
off := ctx.On("session/created", func(ctx context.Context, args ...any) (any, error) {
	return nil, nil // nil and false are no result; anything else is one
}, advanced.Prepend()) // options: advanced.Prepend(), advanced.Global()

err := ctx.Emit(callerCtx, "session/created", id)          // returns at once; err only for a refused emission
err = ctx.Parallel(callerCtx, "session/created", id)        // waits for all; *advanced.AggregateError in registration order
value, err := ctx.Serial(callerCtx, "config/resolve", key)   // the first result
value, err = ctx.Bail(callerCtx, "config/resolve", key)      // Serial as a synchronous call
value, err = ctx.Waterfall(callerCtx, "chat/reply", terminal, msg) // terminal may be nil; terminal is a rutis.Terminal, func(ctx context.Context, args ...any) (any, error), called with the event's arguments
value, err = ctx.Filter(func(l rutis.Listener) (bool, error) { return l.Row == "chat", nil }).Serial(callerCtx, "config/resolve", key)
```

A complete example: the plugin answers `greeting/pick` and adds a link to the `greeting/format` waterfall; the service it provides takes the first result with `Serial`, lets other plugins (in any language) rewrite the greeting with `Waterfall`, and notifies with `Emit`:

```go
package greeter

import (
	"context"
	"fmt"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/advanced"
)

type Greeter struct{ ctx *rutis.Ctx }

func (g *Greeter) Hello(ctx context.Context, name, lang string) (string, error) {
	word, err := g.ctx.Serial(ctx, "greeting/pick", lang) // the first result; nil when none
	if err != nil {
		return "", err
	}
	if word == nil {
		word = "Hello"
	}
	text, err := g.ctx.Waterfall(ctx, "greeting/format", func(_ context.Context, args ...any) (any, error) {
		return args[0], nil // the terminal: the innermost end of the chain
	}, fmt.Sprintf("%v, %s", word, name))
	if err != nil {
		return "", err
	}
	if err := g.ctx.Emit(ctx, "greeting/said", name); err != nil { // returns at once
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
			return nil, nil // no result: the next listener goes on
		})
		ctx.On("greeting/format", func(ctx context.Context, args ...any) (any, error) {
			next := args[len(args)-1].(advanced.Next) // waterfall: next is the last argument
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

Python or TypeScript plugins, or Rust code in the host, listening to the same names join the same chain; see [plugin API: events](plugin-api.en.md#events).

| Topic | Rule |
| --- | --- |
| Waterfall | The listener gets `next` (a `advanced.Next`, `func(context.Context) (any, error)`) as its last argument; calling it runs the rest of the chain, not calling it vetoes the rest; a second call fails with `WaterfallNext`. The outermost value is returned as it is |
| Failures | A failing `Emit` listener never reaches the emitter or the other listeners: it is reported where the listener runs (`rutis: listener of <name> in row <row> failed: …` on stderr). `Parallel` runs all and returns an `*advanced.AggregateError` (even for one failure); `Serial`, `Bail` and `Waterfall` stop at the first error and return it |
| Scope | Events are delivered by name on one bus; `ctx.Scope()` is a descriptor for filters, not a delivery rule |
| Filter | `Filter(keep)` asks `keep` once per listener, in registration order, before any runs, with its `rutis.Listener` (`Name`, `Prefix`, `Plugin`, `Row`, `Runtime`, `Node`, `Scope`, `Global`); `keep` returns `(bool, error)`: an error it returns (or a panic) fails the dispatch before any listener runs, with that error (`Emit` does not return it but reports a line `rutis: filter of <name> from row <row> failed: …`); listeners registered with `advanced.Global()` skip filters |
| Context | A listener's `ctx` is the emitter's (its call chain), cancelled when the row unloads, and when the emitter of a `Parallel`, `Serial`, `Waterfall` or `Bail` gives up; an `Emit` listener is not cancelled when the emitter gives up. Unloading waits for running listeners, at most `RUTIS_SHUTDOWN_TIMEOUT` |
| Names | `internal/*` is the runtime's own; `rutis/*` (`rutis/service`, `rutis/row`, `rutis/runtime`) can be listened to but not emitted (`ReservedEvent`) |

Pass on the context your caller gave you, as with service calls. `errors.Is(err, advanced.ErrWaterfallNext)` (and `ErrReservedEvent`, `ErrAsyncListener`, `ErrBinding`) matches an error of that name from any process.

### Objects with methods

Structs, slices and maps are data and are copied; to hand an object over by reference, write `rutis.Ref(v)`: the far end gets a proxy whose calls reach the original here. For an object another process passes, write the parameter's type as a struct of function fields (as for a `ctx.Use` target, the field names matching the methods by the same rule), or as `any` or `*advanced.Service`:

```go
type Cache struct {
	Get func(ctx context.Context, key string) (any, error)
	Set func(ctx context.Context, key string, value any) error
}

func (w *Warmer) Warm(ctx context.Context, cache Cache) (any, error) {
	if err := cache.Set(ctx, "a", 1); err != nil {
		return nil, err
	}
	return cache.Get(ctx, "a")
}
```

- A function field's call blocks; its first parameter is a `context.Context`. A struct with a field that is not a function, or a parameter of another type, fails the call with `bad-argument`: `warmer.warm: argument 1 should be a Cache, got an object (an object is received only as a struct of function fields or any)`.
- `any` receives a `*advanced.Service`, the type `ctx.Use` gives: `Call(ctx, method, result, args...)`. An object has no method table, so `Methods()` returns `nil`, and a method it lacks is answered `no-method` by the object's own process.
- Only a `*advanced.Service` keeps its identity: one object received several times is one `*advanced.Service`, and handed back to its process it is the original there. A struct of function fields is data: handed over, each field becomes a new function reference; to hand over the object itself, make the parameter `any`.
- An object does not follow its row: after the row that passed it unloads, it can still be called. But an on-demand runtime stops `idle` seconds after its last row unloads, and its objects held elsewhere do not keep it running; calls on them then fail with `peer-gone`: `a call on an object from go-netkit failed: runtime go-netkit stopped because it was idle`. To keep them usable, keep the row that passed them loaded, or make the runtime `eager`.
- An object's methods may be called concurrently.

## 3. Test

`rutistest` needs no host, but the plugin runs in its real runtime and values cross a real session:

```go
package weather

import (
	"context"
	"testing"

	"github.com/arcships/rutis/go/rutis/rutistest"
)

// A fake llm: a Go value whose exported methods are the service's (Ask is ask).
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

`rutistest.Load` validates the config against the schema as the runtime does, failing the test when it does not match, and writes extra keys on stderr. To test that a config is refused, use `rutistest.TryLoad(t, Plugin, config, services)`: it gives back the error, a `*rutis.RemoteError` with `Name` `BindingError` and `Code` `bad-config`.

`Provides` says what the plugin provides, so `rutis-host check` knows without running the plugin that this row provides `weather`: when a host checks its configuration with `check --strict` in CI, the rows that use `weather` pass (the template declares it by default). When present it applies (`Sync` marks synchronous methods, `Rename` renames them). It also runs without one: what `ctx.Provide` provides still reaches rutis by itself, its methods the value's exported ones, all called as `async`; `check` then only knows once the plugin runs, and `--strict` reports an error.

Events in tests: the test side is the host of the bus. `loaded.Emit`, `Parallel`, `Serial`, `Waterfall`, `Bail` and `Filter` emit from the row's scope; `loaded.On(name, fn)` listens as the host does (the plugin's emissions of that name then go through the test side, as they go through rutis); `loaded.Load(config, rutistest.InScope("t1/s1"))` loads another row in a scope; `loaded.Update(config)` changes a row's config as rutis does, by restarting the row: it unloads, then loads with the new config, its listeners registered anew, after the others (rutis sends a Go row no `rows.update`: a change of config is a restart; the row is gone while its cleanups run, so an event a cleanup emits fails); `loaded.Reported()` is what the runtime reported of failed `Emit` listeners. A test of the `greeter` above:

```go
func TestEvents(t *testing.T) {
	loaded := rutistest.Load(t, Plugin, struct{}{}, nil)
	said := make(chan any, 1)
	loaded.On("greeting/said", func(_ context.Context, args ...any) (any, error) {
		said <- args[0] // the test side listens as the host does
		return nil, nil
	})
	var text string
	if err := loaded.Service("greeter").Call(context.Background(), "hello", &text, "Ada", "sv"); err != nil {
		t.Fatal(err)
	}
	if text != "Hej, Ada!" || <-said != "Ada" {
		t.Fatalf("got %q", text)
	}
	word, err := loaded.Serial(context.Background(), "greeting/pick", "en") // the test emits too
	if err != nil || word != nil {
		t.Fatalf("got %v, %v", word, err)
	}
}
```

`Load` checks what a host would: the plugin uses only the services `Inject` declares; provided services are of the type `Provides` declares (an undeclared service is called as rutis calls it, by its exported methods, all async); cleanups succeed on unload. What works only within one process (passing a struct as an object, relying on shared memory) fails here.

A plugin definition the plugin hands to a test double (the double's method takes a `*rutis.Definition`) can be passed to `rutistest.Load` as it is: like rutis mounting a copy, it loads another row in the runtime that defined it, with what the plugin's `Apply` prepared; the `services` given then are that copy's own, as the services of the instance it is in, and so are the services it declares in `Provides`: several copies of one definition can be loaded at once when they provide only declared services (an undeclared service's name is not known in advance and is not isolated). Descriptors and reported lines call a copy's row by the id of its origin row. A definition belongs to the row that handed it over (as an argument, as the result of a service it provides, or in an event it emitted): when that row unloads or is updated (a restart), its copies unload first, and loading the definition fails from then on. A copy's config is fixed when it is mounted: `Update` and `Load` on a copy fail. A definition without a `Name` can also be passed to `rutistest.Load` directly.

## 4. Run it in a local host

```bash
rutis-host dev
```

`dev` builds `cmd/weather` and runs each of its plugins as the runtime `go-weather`; a change to `.go` files, `go.mod` or `go.sum` rebuilds it and restarts only that runtime, and a failed build keeps the old one running.

In `rutis.dev.json`, the row with the plugin's id (here `weather`) has no `name`: it gives the plugin itself its configuration; the other rows are plugins that run with it. The other services the plugin needs are such rows: a fake can be written in any language (a Python, TypeScript or Bun plugin, which `dev` runs), its language under `runtimes`, such as a row `./dev/fake-llm.ts`. For example a fake `llm` in Python, in the project's `dev/`:

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
uv venv dev/.venv                                  # the Python runtime's environment: <project>/.venv
uv pip install --python dev/.venv/bin/python "rutis==0.17.2"   # the version of your rutis-host
```

The Python runtime finds its interpreter in the `.venv` of `runtimes.py.project` (here `dev`), and that environment needs `rutis`; `dev/` holds no Go code, so `go build ./...` is not affected. In TypeScript it is the same: `"runtimes": { "node": { "project": "dev" } }`, `npm install @arcships/rutis` in `dev/`, and the row named `./dev/fake-llm.ts`.

When these rows' files change, only those rows reload, without a rebuild, and the Go runtime does not restart for it; a change to `rutis.dev.json` applies while it runs (see [Developing a plugin with `dev`](rutis-host.en.md#developing-a-plugin-with-dev)). `rutis-host check` lists every row and every Go binary with its plugins, versions and plugin API.

### Calling a method

After it starts and after each reload, `dev` prints which services there are and the row that provides each: `rutis-host dev: services: llm (llm), loader (host), weather (weather)`.

While `dev` runs, in another terminal in the project's directory, call a service the plugin provides, to see what it returns and what it says when it fails, without writing a plugin to call it:

```console
$ rutis-host call weather.today
"sunny in Oslo"
```

Each argument is a JSON value (a string with its quotes: `'"Oslo"'`); the result is printed as JSON; an error goes to stderr with exit status 1. See [rutis-host: Calling a service](rutis-host.en.md#calling-a-service).

### Debugging

`--debug` builds without optimisation (`-gcflags=all=-N -l`) and starts the runtime under [Delve](https://github.com/go-delve/delve): Delve listens on `127.0.0.1:2345`, the program runs as usual, and you set breakpoints once connected. Install Delve first:

```bash
go install github.com/go-delve/delve/cmd/dlv@latest
```

```bash
rutis-host dev --debug
```

In another terminal, connect, set a breakpoint on `Today` and continue; the runtime stops when `Today` is called:

```bash
dlv connect 127.0.0.1:2345
```

```
(dlv) break (*Weather).Today
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
- What `fmt.Println` and `log` print shows in `dev`'s output.
- Tests (§3) need no host: `dlv test .`.

## 5. Publish

Push a tag such as `v0.1.0`: the template's workflow runs the tests and `rutis-host check`, builds binaries for Linux and macOS (x64 / arm64) and Windows x64, and publishes them with `SHA256SUMS` on a GitHub release.

Whoever wants your plugin in a binary of their own imports your package: `rutis.Serve(weather.Plugin, other.Plugin)`.

## 6. Used by a host

How a host installs this plugin: [rutis-host: How a host installs plugins](rutis-host.en.md#how-a-host-installs-plugins).

## 7. Run it on another machine

A plugin binary can also run on another machine as a remote runtime, with `language` set to `go`. Two ways, with the same arguments as the Python runtime; see [Connect nodes](nodes.en.md#a-language-runtime-on-another-machine):

```bash
# Listen, and rutis connects (certificate and key in RUTIS_CERT, RUTIS_KEY)
RUTIS_TOKEN=secret RUTIS_CERT=server.pem RUTIS_KEY=server.key \
  ./weather listen:wss://0.0.0.0:7443/rutis --id gpu --peer main .

# Dial out to a rutis access point, reconnecting by fixed rules after a disconnect
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  ./weather dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

Dialing out, it reports its state on stderr (`rutis: connected to …` and so on); SIGTERM or Ctrl-C unloads all rows and exits; an incompatible access point makes it exit with status 1. For the rules, see [Runtimes that connect in](nodes.en.md#runtimes-that-connect-in).
