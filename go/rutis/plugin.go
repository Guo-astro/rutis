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
package rutis

import (
	"fmt"
	"reflect"
	"runtime"
	"strings"

	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// Plugin declares a plugin whose configuration is C.
type Plugin[C any] struct {
	// Name is the plugin's name in its binary; rows name it `go:<Name>`.
	// It is required of a plugin Serve serves, and is the `plugin` of its
	// listeners' registrations and descriptors (Listener.Plugin). A
	// definition only handed to a service as a value may leave it empty:
	// its copies are then called by their row's id, as in other languages.
	Name string
	// Inject lists the services it uses: it runs only while all are there.
	Inject []string
	// Provides declares the services it provides and their methods
	// (optional). What Ctx.Provide provides reaches rutis without it, its
	// methods the value's exported ones, all asynchronous; Provides marks
	// methods synchronous (Sync) or renames them, and tells `rutis-host
	// check` what the plugin provides before it runs.
	Provides Provides
	// Schema is the JSON Schema of the configuration; without it, one is
	// derived from C.
	Schema map[string]any
	// Apply starts the plugin. An error fails the load; the cleanups
	// registered so far run.
	Apply func(ctx *Ctx, config C) error
}

// NoConfig is the configuration of a plugin that takes none.
type NoConfig struct{}

// Provides maps service names to their shapes.
type Provides map[string]*Shape

// Definition is a defined plugin, for Serve. It is also a value: handed to
// a rutis service as an argument or result, it crosses as a plugin
// definition that rutis can mount (each copy running here, in this
// process), as in Cordis.
type Definition struct {
	name     string
	inject   []string
	provides map[string]*Shape
	schema   any
	api      int
	pkg      string
	config   reflect.Type
	apply    func(ctx *Ctx, config any) error
	// Set instead of the rest for a definition another process handed
	// over: what a test double receives in rutistest, which loads it in
	// the runtime that defined it.
	remote *peer.RemotePlugin
}

// Name is the plugin's name.
func (d *Definition) Name() string { return d.name }

// A *Definition handed to another process (a service method's argument or
// result) crosses as a plugin definition, which rutis can mount: a `plugin`
// reference (docs/design-plugin-values-2026-10-25.md §5).
func init() {
	convert.PluginType = reflect.TypeFor[*Definition]()
	convert.PluginToPeer = func(value any) any {
		def := value.(*Definition)
		if def.remote != nil {
			return def.remote
		}
		return peer.Plugin{Value: def}
	}
	convert.PluginFromPeer = func(remote *peer.RemotePlugin) any { return &Definition{remote: remote} }
}

// Define checks a plugin's declarations and makes it servable. It panics
// on a declaration that cannot work, so a broken plugin fails at start.
func Define[C any](p Plugin[C]) *Definition {
	if strings.ContainsAny(p.Name, ":/\x00") {
		panic(fmt.Sprintf("rutis: invalid plugin name %q", p.Name))
	}
	if p.Apply == nil {
		panic(fmt.Sprintf("rutis: plugin %q has no Apply", p.Name))
	}
	for name, shape := range p.Provides {
		if shape == nil {
			panic(fmt.Sprintf("rutis: plugin %s: provides %s has no shape", p.Name, name))
		}
		if shape.err != nil {
			panic(fmt.Sprintf("rutis: plugin %s: provides %s: %v", p.Name, name, shape.err))
		}
		if strings.ContainsAny(name, "#\x00") {
			panic(fmt.Sprintf("rutis: plugin %s: service name %q cannot be projected", p.Name, name))
		}
	}
	configType := reflect.TypeFor[C]()
	var schema any
	if p.Schema != nil {
		schema = p.Schema
	} else if configType != reflect.TypeFor[NoConfig]() {
		derived, err := schemaOf(configType)
		if err != nil {
			panic(fmt.Sprintf("rutis: plugin %s: config: %v", p.Name, err))
		}
		schema = derived
	}
	pkg := ""
	if pc, _, _, ok := runtime.Caller(1); ok {
		if fn := runtime.FuncForPC(pc); fn != nil {
			pkg = packageOf(fn.Name())
		}
	}
	apply := p.Apply
	return &Definition{
		name:     p.Name,
		inject:   append([]string{}, p.Inject...),
		provides: p.Provides,
		schema:   schema,
		api:      PluginAPI,
		pkg:      pkg,
		config:   configType,
		apply: func(ctx *Ctx, config any) error {
			value, err := convert.FromPeer(config, configType)
			if err != nil {
				return fmt.Errorf("config: %w", err)
			}
			return apply(ctx, value.Interface().(C))
		},
	}
}

// packageOf is the package of a function's full name
// (example.com/a/b.init.func1 -> example.com/a/b).
func packageOf(function string) string {
	slash := strings.LastIndex(function, "/")
	dot := strings.Index(function[slash+1:], ".")
	if dot < 0 {
		return function
	}
	return function[:slash+1+dot]
}

// describe is what the plugin declares, as `rows.schema` answers it.
func (d *Definition) describe() map[string]any {
	provides := map[string]any{}
	for name, shape := range d.provides {
		provides[name] = shape.Kinds()
	}
	inject := d.inject
	if inject == nil {
		inject = []string{}
	}
	var version any
	if v := moduleVersion(d.pkg); v != "" {
		version = v
	}
	return map[string]any{
		"config":   d.schema,
		"inject":   inject,
		"provides": provides,
		"version":  version,
		// Every plugin of this SDK may provide services Provides does not
		// list: they reach rutis by themselves. Declared ones still count
		// as provided for `check`.
		"announces": true,
	}
}

// ── Shapes ───────────────────────────────────────────────────────

// Shape is the methods a provided service has on the wire, and whether
// each is synchronous or asynchronous.
type Shape struct {
	typ     reflect.Type
	methods map[string]shapeMethod
	err     error
}

type shapeMethod struct {
	goName string
	kind   string
}

// ShapeOption adjusts MethodsOf.
type ShapeOption func(*shapeOptions)

type shapeOptions struct {
	sync    []string
	renames map[string]string
}

// Sync marks the named Go methods synchronous: a caller in another
// language waits for them without awaiting.
func Sync(methods ...string) ShapeOption {
	return func(o *shapeOptions) { o.sync = append(o.sync, methods...) }
}

// Rename gives the Go method `method` the wire name `wire`.
func Rename(method, wire string) ShapeOption {
	return func(o *shapeOptions) {
		if o.renames == nil {
			o.renames = map[string]string{}
		}
		o.renames[method] = wire
	}
}

// MethodsOf is the shape of a service of type T: its exported methods,
// asynchronous unless Sync says otherwise.
func MethodsOf[T any](options ...ShapeOption) *Shape {
	var o shapeOptions
	for _, option := range options {
		option(&o)
	}
	t := reflect.TypeFor[T]()
	shape := &Shape{typ: t, methods: map[string]shapeMethod{}}
	goNames := map[string]bool{}
	for i := 0; i < t.NumMethod(); i++ {
		method := t.Method(i)
		goNames[method.Name] = true
		if err := convert.CheckSignature(method.Type, false); err != nil {
			shape.err = fmt.Errorf("method %s: %w", method.Name, err)
			return shape
		}
		wire := convert.WireName(method.Name)
		if renamed, ok := o.renames[method.Name]; ok {
			wire = renamed
		}
		if other, taken := shape.methods[wire]; taken {
			shape.err = fmt.Errorf("methods %s and %s are both %s on the wire", other.goName, method.Name, wire)
			return shape
		}
		shape.methods[wire] = shapeMethod{goName: method.Name, kind: "async"}
	}
	for _, name := range o.sync {
		if !goNames[name] {
			shape.err = fmt.Errorf("Sync(%q): %s has no exported method %s", name, t, name)
			return shape
		}
	}
	for name := range o.renames {
		if !goNames[name] {
			shape.err = fmt.Errorf("Rename(%q): %s has no exported method %s", name, t, name)
			return shape
		}
	}
	for wire, method := range shape.methods {
		for _, name := range o.sync {
			if name == method.goName {
				method.kind = "sync"
				shape.methods[wire] = method
			}
		}
	}
	if len(shape.methods) == 0 {
		shape.err = fmt.Errorf("%s has no exported methods", t)
	}
	return shape
}

// Kinds maps the wire names of the methods to "sync" or "async".
func (s *Shape) Kinds() map[string]string {
	kinds := make(map[string]string, len(s.methods))
	for wire, method := range s.methods {
		kinds[wire] = method.kind
	}
	return kinds
}

// method is the Go method of `value` behind the wire name `wire`.
func (s *Shape) method(value reflect.Value, wire string) (reflect.Value, bool) {
	if s != nil {
		if m, ok := s.methods[wire]; ok {
			found := value.MethodByName(m.goName)
			return found, found.IsValid()
		}
		return reflect.Value{}, false
	}
	index, ok := convert.Methods(value.Type())[wire]
	if !ok {
		return reflect.Value{}, false
	}
	return value.Method(index), true
}

// wires lists the wire names of the methods `value` offers through `s`.
func (s *Shape) wires(value reflect.Value) map[string]bool {
	out := map[string]bool{}
	if s != nil {
		for wire := range s.methods {
			out[wire] = true
		}
		return out
	}
	for wire := range convert.Methods(value.Type()) {
		out[wire] = true
	}
	return out
}
