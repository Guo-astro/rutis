package rutistest

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"testing"

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

type tools struct{ title string }

func (t *tools) Title(context.Context) (string, error) { return t.title, nil }

type partConfig struct {
	N string `json:"n"`
}

type seen struct{ line string }

func (s *seen) Line(context.Context) (string, error) { return s.line, nil }

// origin prepares a value in Apply and hands a part using it to `parts`;
// the part has no Name: it is only a value.
var origin = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:   "origin",
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
			Inject: []string{"tools"},
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

// A plugin definition handed to a test double can be loaded with Load:
// it runs in the runtime that defined it, with what its Apply prepared.
func TestADefinitionHandedToADoubleLoads(t *testing.T) {
	registry := &parts{}
	Load(t, origin, nil, map[string]any{"parts": registry})
	registry.mu.Lock()
	defs := append([]*rutis.Definition(nil), registry.defs...)
	registry.mu.Unlock()
	if len(defs) != 1 {
		t.Fatalf("registered %d definitions", len(defs))
	}
	for _, title := range []string{"A", "B"} {
		copy := Load(t, defs[0], partConfig{N: title}, map[string]any{"tools": &tools{title}}, Row("part-"+title))
		var line string
		if err := copy.Service("seen").Call(context.Background(), "line", &line); err != nil {
			t.Fatal(err)
		}
		if want := "part " + title + " sees " + title + " with prepared in apply"; line != want {
			t.Fatalf("%q, want %q", line, want)
		}
		copy.Unload()
	}
}

// A copy's injected service must be given, as in Load.
func TestACopyNeedsItsServices(t *testing.T) {
	registry := &parts{}
	Load(t, origin, nil, map[string]any{"parts": registry})
	failed := &fatal{TB: t}
	func() {
		defer func() { _ = recover() }()
		Load(failed, registry.defs[0], partConfig{N: "x"}, nil)
	}()
	if !strings.Contains(failed.message, "injects tools") {
		t.Fatalf("%q", failed.message)
	}
}

// fatal records a Fatalf instead of ending the test.
type fatal struct {
	testing.TB
	message string
}

func (f *fatal) Fatalf(format string, args ...any) {
	f.message = fmt.Sprintf(format, args...)
	panic(f)
}

func (f *fatal) Helper() {}
