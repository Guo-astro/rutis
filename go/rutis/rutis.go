// Package rutis writes rutis plugins in Go, and is the runtime that runs
// them.
//
// A plugin declares the services it injects, the services it provides and
// its configuration, and has an Apply function:
//
//	var Plugin = rutis.Define(rutis.Plugin[Config]{
//		Name:   "weather",
//		Inject: []string{"llm"},
//		Apply: func(ctx *rutis.Ctx, config Config) error {
//			var llm LLM
//			if err := ctx.Use("llm", &llm); err != nil {
//				return err
//			}
//			ctx.Provide("weather", &Weather{llm: llm, city: config.City})
//			return nil
//		},
//	})
//
// What Ctx.Provide provides reaches rutis: its methods are the value's
// exported ones, all asynchronous. Provides is optional; it marks methods
// synchronous, `Provides: rutis.Provides{"weather":
// rutis.MethodsOf[*Weather](rutis.Sync("Unit"))}`, and tells `rutis-host
// check` what the plugin provides before it runs.
//
// A binary serves one or more plugins: `func main() { rutis.Serve(weather.Plugin) }`.
// rutis decides when a plugin runs: Apply runs once every injected service
// is there, and the plugin is unloaded when one goes away.
//
// What a plugin author rarely writes is in package advanced.
package rutis

import runtime "github.com/arcships/rutis/go/rutis/internal/runtime"

// Plugin declares a plugin whose configuration is C: its Name, the
// services it injects (Inject), the shapes of those it provides
// (Provides, optional), its configuration's JSON Schema (Schema, derived
// from C when nil) and Apply.
type Plugin[C any] = runtime.Plugin[C]

// NoConfig is the configuration of a plugin that takes none.
type NoConfig = runtime.NoConfig

// Definition is a defined plugin, for Serve. It is also a value: handed to
// a rutis service as an argument or result, it crosses as a plugin
// definition that rutis can mount (each copy running here, in this
// process), as in Cordis.
type Definition = runtime.Definition

// Define checks a plugin's declarations and makes it servable. It panics
// on a declaration that cannot work, so a broken plugin fails at start.
func Define[C any](p Plugin[C]) *Definition { return runtime.DefineWrapped(p, 1) }

// Serve runs the plugins `defs` as a rutis runtime, as the command line
// says, and exits the process: with --rutis-manifest it prints what the
// plugins declare; otherwise it serves the session on the channel it is
// given until the session ends.
func Serve(defs ...*Definition) { runtime.Serve(defs...) }

// Ctx is what Apply receives. It is a context.Context cancelled when the
// plugin unloads: goroutines the plugin starts should watch it.
type Ctx = runtime.Ctx

// Provides maps service names to their shapes.
type Provides = runtime.Provides

// Shape is the methods a provided service has on the wire, and whether
// each is synchronous or asynchronous.
type Shape = runtime.Shape

// ShapeOption adjusts MethodsOf.
type ShapeOption = runtime.ShapeOption

// MethodsOf is the shape of a service of type T: its exported methods,
// asynchronous unless Sync says otherwise.
func MethodsOf[T any](options ...ShapeOption) *Shape { return runtime.MethodsOf[T](options...) }

// Sync marks the named Go methods synchronous: a caller in another
// language waits for them without awaiting.
func Sync(methods ...string) ShapeOption { return runtime.Sync(methods...) }

// Ref marks v to cross by reference: the other process gets a proxy
// calling v's exported methods. Values otherwise cross as data.
func Ref(v any) any { return runtime.Ref(v) }

// RemoteError is an error thrown in another process.
type RemoteError = runtime.RemoteError

// ErrBinding matches (errors.Is) a transport failure, a withdrawn
// listener, a refused frame.
var ErrBinding = runtime.ErrBinding

// Emitter emits events, filtered: Ctx.Filter makes one.
type Emitter = runtime.Emitter

// ListenOption adjusts On.
type ListenOption = runtime.ListenOption

// Listener is what the bus says of a listener, as a Filter predicate sees
// it. Empty strings are absent values: Row is empty for a listener of no
// row, Runtime for one of the host, Node for one of this node.
type Listener = runtime.Listener

// Scope is one loader instance (a group copy, such as a tenant or a
// session) of a scope chain, outermost first.
type Scope = runtime.Scope

// Terminal is a waterfall's innermost step: `next` of the last listener
// calls it with the event's arguments; its result is that `next`'s.
type Terminal = runtime.Terminal
