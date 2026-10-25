package rutistest

import (
	"context"
	"fmt"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis"
)

// parts is a test double of a part registry: a plugin hands it plugin
// definitions (docs/design-plugin-values-2026-10-25.md R9, R10).
type parts struct {
	mu   sync.Mutex
	defs []*rutis.Definition
}

func (p *parts) Register(_ context.Context, scope string, def *rutis.Definition) error {
	if scope != "session" {
		return fmt.Errorf("unknown scope %s", scope)
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	p.defs = append(p.defs, def)
	return nil
}

func (p *parts) first(t *testing.T) *rutis.Definition {
	t.Helper()
	p.mu.Lock()
	defer p.mu.Unlock()
	if len(p.defs) != 1 {
		t.Fatalf("registered %d definitions", len(p.defs))
	}
	return p.defs[0]
}

type tools struct{ title string }

func (t *tools) Title(context.Context) (string, error) { return t.title, nil }

type partConfig struct {
	N string `json:"n"`
}

type seen struct{ line string }

func (s *seen) Line(context.Context) (string, error) { return s.line, nil }

// originOf is a plugin that prepares a value in Apply and hands a part
// using it to `parts`; the part has no Name: it is only a value. With
// `declared`, the part declares the service it provides.
func originOf(name string, declared bool) *rutis.Definition {
	var provides rutis.Provides
	if declared {
		provides = rutis.Provides{"seen": rutis.MethodsOf[*seen]()}
	}
	return rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name:   name,
		Inject: []string{"parts"},
		Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
			prepared := "prepared in apply"
			var registry struct {
				Register func(ctx context.Context, scope string, def *rutis.Definition) error
			}
			if err := ctx.Use("parts", &registry); err != nil {
				return err
			}
			part := rutis.Define(rutis.Plugin[partConfig]{
				Inject:   []string{"tools"},
				Provides: provides,
				Apply: func(ctx *rutis.Ctx, config partConfig) error {
					var tools struct {
						Title func(context.Context) (string, error)
					}
					if err := ctx.Use("tools", &tools); err != nil {
						return err
					}
					title, err := tools.Title(ctx)
					if err != nil {
						return err
					}
					ctx.Provide("seen", &seen{fmt.Sprintf("part %s sees %s with %s", config.N, title, prepared)})
					return nil
				},
			})
			return registry.Register(ctx, "session", part)
		},
	})
}

var origin = originOf("origin", false)

func line(t *testing.T, loaded *Loaded) string {
	t.Helper()
	var line string
	if err := loaded.Service("seen").Call(context.Background(), "line", &line); err != nil {
		t.Fatal(err)
	}
	return line
}

// A plugin definition handed to a test double can be loaded with Load:
// it runs in the runtime that defined it, with what its Apply prepared.
func TestADefinitionHandedToADoubleLoads(t *testing.T) {
	registry := &parts{}
	Load(t, origin, nil, map[string]any{"parts": registry})
	def := registry.first(t)
	for _, title := range []string{"A", "B"} {
		copy := Load(t, def, partConfig{N: title}, map[string]any{"tools": &tools{title}}, Row("part-"+title))
		if got, want := line(t, copy), "part "+title+" sees "+title+" with prepared in apply"; got != want {
			t.Fatalf("%q, want %q", got, want)
		}
		copy.Unload()
	}
}

// Two copies of one definition load at once, as in two instances: each
// has its own services, and what it declares it provides is its own.
func TestTwoCopiesLoadAtOnce(t *testing.T) {
	registry := &parts{}
	Load(t, originOf("origin", true), nil, map[string]any{"parts": registry})
	def := registry.first(t)
	a := Load(t, def, partConfig{N: "A"}, map[string]any{"tools": &tools{"A"}}, Row("part-A"))
	b := Load(t, def, partConfig{N: "B"}, map[string]any{"tools": &tools{"B"}}, Row("part-B"))
	for title, copy := range map[string]*Loaded{"A": a, "B": b} {
		if got, want := line(t, copy), "part "+title+" sees "+title+" with prepared in apply"; got != want {
			t.Fatalf("%q, want %q", got, want)
		}
		if !slices.Contains(copy.Provided(), "seen") {
			t.Fatalf("%s provides %v", title, copy.Provided())
		}
	}
}

// A definition ends with the row that handed it over (R6): its copies end
// first, and it cannot be loaded any more.
func TestADefinitionEndsWithItsOrigin(t *testing.T) {
	registry := &parts{}
	handing := Load(t, originOf("origin", true), nil, map[string]any{"parts": registry})
	def := registry.first(t)
	copy := Load(t, def, partConfig{N: "A"}, map[string]any{"tools": &tools{"A"}}, Row("part-A"))
	line(t, copy)
	handing.Unload()
	if provided := copy.Provided(); slices.Contains(provided, "seen") {
		t.Fatalf("the copy still provides %v", provided)
	}
	if message := fatalOf(func(tb testing.TB) {
		Load(tb, def, partConfig{N: "B"}, map[string]any{"tools": &tools{"B"}})
	}, t); !strings.Contains(message, "row test that handed this plugin definition over is unloaded") {
		t.Fatalf("%q", message)
	}
}

// A copy's config is fixed when it is mounted: Update and Load on it fail,
// saying so.
func TestACopyIsNotUpdatedOrLoadedAgain(t *testing.T) {
	registry := &parts{}
	Load(t, origin, nil, map[string]any{"parts": registry})
	copy := Load(t, registry.first(t), partConfig{N: "A"}, map[string]any{"tools": &tools{"A"}})
	if message := fatalOf(func(tb testing.TB) {
		copy.h.t = tb
		defer func() { copy.h.t = t }()
		copy.Update(partConfig{N: "B"})
	}, t); !strings.Contains(message, "no rows.update") {
		t.Fatalf("%q", message)
	}
	if message := fatalOf(func(tb testing.TB) {
		copy.h.t = tb
		defer func() { copy.h.t = t }()
		copy.Load(partConfig{N: "B"})
	}, t); !strings.Contains(message, "load the definition again") {
		t.Fatalf("%q", message)
	}
}

// A copy's injected service must be given, as in Load.
func TestACopyNeedsItsServices(t *testing.T) {
	registry := &parts{}
	Load(t, origin, nil, map[string]any{"parts": registry})
	def := registry.first(t)
	if message := fatalOf(func(tb testing.TB) {
		Load(tb, def, partConfig{N: "x"}, nil)
	}, t); !strings.Contains(message, "injects tools") {
		t.Fatalf("%q", message)
	}
}

// A definition without a name, only meant to be handed over, loads too:
// it is served under a name of the test's.
func TestANamelessDefinitionLoads(t *testing.T) {
	nameless := rutis.Define(rutis.Plugin[partConfig]{
		Inject: []string{"tools"},
		Apply: func(ctx *rutis.Ctx, config partConfig) error {
			ctx.Provide("seen", &seen{"nameless " + config.N})
			return nil
		},
	})
	done := make(chan *Loaded, 1)
	// Before, the runtime refused it and Load hung on the handshake.
	go func() { done <- Load(t, nameless, partConfig{N: "x"}, map[string]any{"tools": &tools{"x"}}) }()
	select {
	case loaded := <-done:
		if got := line(t, loaded); got != "nameless x" {
			t.Fatalf("%q", got)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("a nameless definition hangs Load")
	}
}

// fatalOf is the message `f` fails `tb` with.
func fatalOf(f func(testing.TB), t *testing.T) string {
	t.Helper()
	failed := &fatal{TB: t}
	func() {
		defer func() {
			if recovered := recover(); recovered != nil && recovered != failed {
				panic(recovered)
			}
		}()
		f(failed)
	}()
	if failed.message == "" {
		t.Fatal("it did not fail")
	}
	return failed.message
}

// fatal records a Fatal instead of ending the test.
type fatal struct {
	testing.TB
	message string
}

func (f *fatal) Fatalf(format string, args ...any) {
	f.message = fmt.Sprintf(format, args...)
	panic(f)
}

func (f *fatal) Fatal(args ...any) {
	f.message = fmt.Sprint(args...)
	panic(f)
}

func (f *fatal) Helper() {}
