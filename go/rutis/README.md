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

Events are shared by plugins in the same process. `ctx.On(name, func(context.Context, ...any) error)` returns an independent disposer and removes its listener when the row unloads. `ctx.Emit(callerCtx, name, args...)` calls listeners in order and returns the first error; pass your caller's context so calls made by listeners keep the synchronous wait chain. Each listener's context is also cancelled when its row unloads. `internal/*` names are reserved.

A host declares incoming names with `RuntimePlugin::emits` (or `Mount::emits`); host emission waits for every listener and collects their failures in an `AggregateError`. In tests, declare those names as the final arguments of `rutistest.Load(..., services, "weather/city")`, then call `loaded.Emit(callerCtx, "weather/city", "Bergen")` through the real session. Plugin emission stays within the Go process.

Start a project with `rutis-host new <name> --lang go`. Guide: [docs/guide/go-plugin.en.md](https://github.com/arcships/rutis/blob/main/docs/guide/go-plugin.en.md) (中文: [go-plugin.md](https://github.com/arcships/rutis/blob/main/docs/guide/go-plugin.md)). Go 1.24 or later; standard library only.
