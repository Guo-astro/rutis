// Package {{package}} is a rutis plugin.
package {{package}}

import (
	"context"

	"github.com/arcships/rutis/go/rutis"
)

// Config is the plugin's configuration; its JSON Schema is derived from it.
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
	Name: "{{id}}",
	// Services this plugin uses: Inject: []string{"llm"}, then ctx.Use("llm", &llm).
	Inject: []string{},
	// What it provides, so that `rutis-host check` (and `check --strict` in a
	// host's CI) knows it before the plugin runs: the exported methods of
	// *Greeter, async; rutis.MethodsOf[*Greeter](rutis.Sync("Hello")) would
	// make Hello sync.
	Provides: rutis.Provides{"greeter": rutis.MethodsOf[*Greeter]()},
	Apply: func(ctx *rutis.Ctx, config Config) error {
		greeting := config.Greeting
		if greeting == "" {
			greeting = "Hello"
		}
		ctx.Provide("greeter", &Greeter{greeting: greeting})
		// Register cleanups with ctx.Effect; goroutines watch ctx.Done().
		ctx.Effect(func(context.Context) error { return nil })
		return nil
	},
})
