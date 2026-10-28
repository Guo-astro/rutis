# rutis for Go

Write [rutis](https://github.com/arcships/rutis) plugins in Go; the runtime that runs them is in the same module, so a plugin binary needs nothing else.

```go
package weather

import (
	"context"

	"github.com/arcships/rutis/go/rutis"
)

type LLM struct {
	Ask func(ctx context.Context, question string) (string, error)
}

type Weather struct {
	llm  LLM
	city string
}

func (w *Weather) Today(ctx context.Context) (string, error) {
	answer, err := w.llm.Ask(ctx, "weather in "+w.city)
	return answer + " in " + w.city, err
}

type Config struct {
	City string `json:"city,omitempty"`
}

var Plugin = rutis.Define(rutis.Plugin[Config]{
	Name:   "weather",
	Inject: []string{"llm"},
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

What `ctx.Provide` provides reaches rutis by itself, its methods the value's exported ones, called as `async`. `Provides` is optional: `Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Unit"))}` marks methods synchronous and tells `rutis-host check` what the plugin provides before it runs.

A binary serves one or more plugins, and is what a host runs:

```go
func main() { rutis.Serve(weather.Plugin) }
```

Test it without a host (`rutistest` runs the plugin in its runtime, across a real session):

```go
loaded := rutistest.Load(t, weather.Plugin, weather.Config{City: "Oslo"}, map[string]any{"llm": fakeLLM{}})
var today string
err := loaded.Service("weather").Call(context.Background(), "today", &today)
```

Events: every listener on a host, in any language or process, is on one bus. `ctx.On(name, func(ctx context.Context, args ...any) (any, error), advanced.Prepend()/advanced.Global())` registers a listener (a waterfall passes `next`, an `advanced.Next`, last); `ctx.Emit` returns at once (its error is only a refusal; a listener's failure is reported where it runs), `ctx.Parallel` waits for all (`*advanced.AggregateError` in registration order), `ctx.Serial` / `ctx.Bail` return the first result (nil and false are none), `ctx.Waterfall(callerCtx, name, terminal, args...)` chains through `next` (the last `next` calls the `rutis.Terminal`, `func(ctx context.Context, args ...any) (any, error)`, with the event's arguments; nil for none), and `ctx.Filter(func(rutis.Listener) (bool, error))` filters any of them (an error it returns, or a panic, fails the dispatch before any listener runs; under `Emit` it is reported, not returned; `Listener.Plugin` is the plugin's `Name`, `Listener.Row` the row's id in the configuration). A row hears every event of the name on the one bus; a scope is a filter descriptor. A listener the host refuses while its row loads fails the load; refused later, it is reported on stderr. Unloading cancels running listeners and waits for them, up to `RUTIS_SHUTDOWN_TIMEOUT`. `internal/*` is reserved; `rutis/*` can be listened to, not emitted.

Errors across processes: an error a listener in another process returns (in `Parallel`'s `*advanced.AggregateError`, or from `Serial`, `Bail`, `Waterfall`) arrives as a `*rutis.RemoteError` carrying its name and message, not as its Go type. Match it by name: `var remote *rutis.RemoteError; errors.As(err, &remote) && remote.Name == "QuotaError"` (`errors.Is` works for rutis's own names, such as `advanced.ErrWaterfallNext`). When every listener is in this process (the fast path), the listener's original error value may arrive instead (an error type with a `Name() string` method crosses with that name), so code handling both checks the `*rutis.RemoteError`'s name and, for its own errors, their Go type.

In tests the test side is the bus's host: `loaded.Parallel(callerCtx, "weather/city", "Bergen")` (and the other modes) emit from the row's scope, `loaded.On` listens as the host does, `loaded.Load(config, rutistest.InScope("t1/s1"))` loads another row in a scope. Descriptors name the runtime `go` (`rutistest.RuntimeName` changes it); unloading waits for running listeners up to `RUTIS_SHUTDOWN_TIMEOUT`, as under a host.

Start a project with `rutis-host new <name> --lang go`. Guide: [docs/guide/go-plugin.en.md](https://github.com/arcships/rutis/blob/main/docs/guide/go-plugin.en.md) (中文: [go-plugin.md](https://github.com/arcships/rutis/blob/main/docs/guide/go-plugin.md)). Go 1.24 or later; standard library only.
