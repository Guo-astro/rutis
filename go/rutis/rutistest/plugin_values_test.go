// guarantee: bridge/plugin-values
package rutistest

import (
	"context"
	"errors"
	"fmt"
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

func (p *parts) all() []*rutis.Definition {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]*rutis.Definition(nil), p.defs...)
}

func (p *parts) first(t *testing.T) *rutis.Definition {
	t.Helper()
	defs := p.all()
	if len(defs) != 1 {
		t.Fatalf("registered %d definitions", len(defs))
	}
	return defs[0]
}

// tools is a part's `tools`: it counts the calls.
type tools struct {
	title string
	mu    sync.Mutex
	calls int
}

func (t *tools) Title(context.Context) (string, error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.calls++
	return t.title, nil
}

func (t *tools) called() int {
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.calls
}

type partConfig struct {
	N string `json:"n"`
}

type seen struct{ line string }

func (s *seen) Line(context.Context) (string, error) { return s.line, nil }

// maker returns another part, as a service of the origin row; Offer
// emits one instead.
type maker struct {
	make  func() *rutis.Definition
	offer func(context.Context) error
}

func (m *maker) Make(context.Context) (*rutis.Definition, error) { return m.make(), nil }

func (m *maker) Offer(ctx context.Context) error { return m.offer(ctx) }

type variant struct {
	// The part declares the service it provides.
	declared bool
	// Its cleanup uses its `tools` once more.
	cleanup bool
	// It reports a failing emit listener (a line naming its row's id).
	reports bool
}

// originOf is a plugin that prepares a value in Apply and hands a part
// using it to `parts`, and provides `maker`, whose Make returns another;
// the part has no Name: it is only a value.
func originOf(name string, v variant) *rutis.Definition {
	var provides rutis.Provides
	if v.declared {
		provides = rutis.Provides{"seen": rutis.MethodsOf[*seen]()}
	}
	part := func(prepared string) *rutis.Definition {
		return rutis.Define(rutis.Plugin[partConfig]{
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
				if v.cleanup {
					ctx.Effect(func(c context.Context) error {
						_, err := tools.Title(c)
						return err
					})
				}
				if v.reports {
					ctx.On("part/boom", func(context.Context, ...any) (any, error) { return nil, errors.New("boom") })
					if err := ctx.Emit(ctx, "part/boom"); err != nil {
						return err
					}
				}
				ctx.Provide("seen", &seen{fmt.Sprintf("part %s sees %s with %s", config.N, title, prepared)})
				return nil
			},
		})
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
			ctx.Provide("maker", &maker{
				make: func() *rutis.Definition { return part(prepared) },
				offer: func(c context.Context) error {
					return ctx.Emit(c, "parts/offer", part(prepared))
				},
			})
			return registry.Register(ctx, "session", part(prepared))
		},
	})
}

var origin = originOf("origin", variant{})

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
		copy := Load(t, def, partConfig{N: title}, map[string]any{"tools": &tools{title: title}}, Row("part-"+title))
		if got, want := line(t, copy), "part "+title+" sees "+title+" with prepared in apply"; got != want {
			t.Fatalf("%q, want %q", got, want)
		}
		copy.Unload()
	}
}

// A copy's cleanup still has the services given to it alone.
func TestACopysCleanupHasItsServices(t *testing.T) {
	registry := &parts{}
	Load(t, originOf("origin", variant{cleanup: true}), nil, map[string]any{"parts": registry})
	own := &tools{title: "A"}
	copy := Load(t, registry.first(t), partConfig{N: "A"}, map[string]any{"tools": own})
	copy.Unload()
	if calls := own.called(); calls != 2 {
		t.Fatalf("the copy's tools were called %d times, want 2 (apply, cleanup)", calls)
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
	go func() { done <- Load(t, nameless, partConfig{N: "x"}, map[string]any{"tools": &tools{title: "x"}}) }()
	select {
	case loaded := <-done:
		if got := line(t, loaded); got != "nameless x" {
			t.Fatalf("%q", got)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("a nameless definition hangs Load")
	}
}

type flaky struct {
	Fail bool `json:"fail"`
}

var failing = rutis.Define(rutis.Plugin[flaky]{
	Name:   "flaky",
	Inject: []string{"tools"},
	Apply: func(ctx *rutis.Ctx, config flaky) error {
		if config.Fail {
			return errors.New("failing as asked")
		}
		return nil
	},
})

// A row that fails to load leaves no row behind: a row of the same id
// loads afterwards.
func TestAFailedLoadLeavesNoRow(t *testing.T) {
	_, err := TryLoad(t, failing, flaky{Fail: true}, map[string]any{"tools": &tools{title: "x"}}, Row("again"))
	if err == nil || !strings.Contains(err.Error(), "failing as asked") {
		t.Fatalf("%v", err)
	}
	if again := Load(t, failing, flaky{}, map[string]any{"tools": &tools{title: "x"}}, Row("again")); again.id != "again" {
		t.Fatalf("%s", again.id)
	}
}
