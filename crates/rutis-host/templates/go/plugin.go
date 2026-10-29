// Package {{package}} is a rutis plugin. The Go plugin guide
// (docs/guide/go-plugin.en.md) takes it on from here: step 2 explains this
// file, step 3 adds a service it uses.
package {{package}}

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
	Name: "{{id}}",
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
