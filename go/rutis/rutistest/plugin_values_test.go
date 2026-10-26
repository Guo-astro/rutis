package rutistest

import (
	"context"
	"errors"
	"fmt"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
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

// Two copies of one definition providing only declared services load at
// once, as in two instances: each has its own services.
func TestTwoCopiesLoadAtOnce(t *testing.T) {
	registry := &parts{}
	Load(t, originOf("origin", variant{declared: true}), nil, map[string]any{"parts": registry})
	def := registry.first(t)
	a := Load(t, def, partConfig{N: "A"}, map[string]any{"tools": &tools{title: "A"}}, Row("part-A"))
	b := Load(t, def, partConfig{N: "B"}, map[string]any{"tools": &tools{title: "B"}}, Row("part-B"))
	for title, copy := range map[string]*Loaded{"A": a, "B": b} {
		if got, want := line(t, copy), "part "+title+" sees "+title+" with prepared in apply"; got != want {
			t.Fatalf("%q, want %q", got, want)
		}
		if !slices.Contains(copy.Provided(), "seen") {
			t.Fatalf("%s provides %v", title, copy.Provided())
		}
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

const ended = "has since unloaded or restarted"

// A definition ends with the row that handed it over (R6): its copies end
// first, and it cannot be loaded any more.
func TestADefinitionEndsWithItsOrigin(t *testing.T) {
	registry := &parts{}
	handing := Load(t, originOf("origin", variant{declared: true}), nil, map[string]any{"parts": registry})
	def := registry.first(t)
	copy := Load(t, def, partConfig{N: "A"}, map[string]any{"tools": &tools{title: "A"}}, Row("part-A"))
	line(t, copy)
	handing.Unload()
	if provided := copy.Provided(); slices.Contains(provided, "seen") {
		t.Fatalf("the copy still provides %v", provided)
	}
	if message := fatalOf(func(tb testing.TB) {
		Load(tb, def, partConfig{N: "B"}, map[string]any{"tools": &tools{title: "B"}})
	}, t); !strings.Contains(message, "row test that handed this plugin definition over "+ended) {
		t.Fatalf("%q", message)
	}
}

// A restart of the origin row (Update) ends its definitions too: the
// copies end first, the old definition cannot be loaded, the new one can.
func TestADefinitionEndsWhenItsOriginRestarts(t *testing.T) {
	registry := &parts{}
	handing := Load(t, originOf("origin", variant{declared: true}), nil, map[string]any{"parts": registry})
	old := registry.first(t)
	copy := Load(t, old, partConfig{N: "A"}, map[string]any{"tools": &tools{title: "A"}}, Row("part-A"))
	line(t, copy)
	handing.Update(nil)
	if provided := copy.Provided(); slices.Contains(provided, "seen") {
		t.Fatalf("the copy still provides %v", provided)
	}
	if message := fatalOf(func(tb testing.TB) {
		Load(tb, old, partConfig{N: "B"}, map[string]any{"tools": &tools{title: "B"}})
	}, t); !strings.Contains(message, ended) {
		t.Fatalf("%q", message)
	}
	defs := registry.all()
	if len(defs) != 2 {
		t.Fatalf("registered %d definitions", len(defs))
	}
	again := Load(t, defs[1], partConfig{N: "C"}, map[string]any{"tools": &tools{title: "C"}}, Row("part-C"))
	if got := line(t, again); got != "part C sees C with prepared in apply" {
		t.Fatalf("%q", got)
	}
}

// A definition a service returns belongs to the row providing it (R6).
func TestAReturnedDefinitionBelongsToTheProvidingRow(t *testing.T) {
	registry := &parts{}
	handing := Load(t, origin, nil, map[string]any{"parts": registry})
	var made *rutis.Definition
	if err := handing.Service("maker").Call(context.Background(), "make", &made); err != nil {
		t.Fatal(err)
	}
	copy := Load(t, made, partConfig{N: "M"}, map[string]any{"tools": &tools{title: "M"}}, Row("part-M"))
	if got := line(t, copy); got != "part M sees M with prepared in apply" {
		t.Fatalf("%q", got)
	}
	handing.Unload()
	if message := fatalOf(func(tb testing.TB) {
		Load(tb, made, partConfig{N: "N"}, map[string]any{"tools": &tools{title: "N"}})
	}, t); !strings.Contains(message, "row test that handed this plugin definition over "+ended) {
		t.Fatalf("%q", message)
	}
}

// A definition the row emits belongs to it (R6).
func TestAnEmittedDefinitionBelongsToTheEmittingRow(t *testing.T) {
	registry := &parts{}
	handing := Load(t, origin, nil, map[string]any{"parts": registry})
	offered := make(chan *rutis.Definition, 1)
	handing.On("parts/offer", func(_ context.Context, args ...any) (any, error) {
		var def *rutis.Definition
		if err := convert.Decode(args[0], &def); err != nil {
			return nil, err
		}
		offered <- def
		return nil, nil
	})
	if err := handing.Service("maker").Call(context.Background(), "offer", nil); err != nil {
		t.Fatal(err)
	}
	def := testwait.Within(t, "the offered definition", offered)
	handing.Unload()
	if message := fatalOf(func(tb testing.TB) {
		Load(tb, def, partConfig{N: "E"}, map[string]any{"tools": &tools{title: "E"}})
	}, t); !strings.Contains(message, "row test that handed this plugin definition over "+ended) {
		t.Fatalf("%q", message)
	}
}

// A copy's rows are called by its origin row's id, as the host calls them
// (R5): a reported line names it.
func TestACopyIsCalledByItsOriginRowsID(t *testing.T) {
	registry := &parts{}
	Load(t, originOf("origin", variant{reports: true}), nil, map[string]any{"parts": registry}, Row("origin-row"))
	copy := Load(t, registry.first(t), partConfig{N: "A"}, map[string]any{"tools": &tools{title: "A"}}, Row("part-A"))
	testwait.Eventually(t, "the copy's listener failure reported", func() bool {
		return slices.ContainsFunc(copy.Reported(), func(line string) bool {
			return strings.Contains(line, "listener of part/boom in row origin-row failed")
		})
	}, func() string { return strings.Join(copy.Reported(), "\n") })
}

// A copy's config is fixed when it is mounted: Update and Load on it fail,
// saying so.
func TestACopyIsNotUpdatedOrLoadedAgain(t *testing.T) {
	registry := &parts{}
	Load(t, origin, nil, map[string]any{"parts": registry})
	copy := Load(t, registry.first(t), partConfig{N: "A"}, map[string]any{"tools": &tools{title: "A"}})
	if message := fatalOf(func(tb testing.TB) {
		copy.h.t = tb
		defer func() { copy.h.t = t }()
		copy.Update(partConfig{N: "B"})
	}, t); !strings.Contains(message, "never updates a copy") {
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

// A copy's failures are the caller's: they reach the TB Load was given.
func TestACopysFailureIsTheCallers(t *testing.T) {
	registry := &parts{}
	Load(t, origin, nil, map[string]any{"parts": registry})
	if message := fatalOf(func(tb testing.TB) {
		Load(tb, registry.first(t), partConfig{N: "A"}, map[string]any{"tools": &tools{title: "A"}}, Row("test"))
	}, t); !strings.Contains(message, "a row test is already loaded") {
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

// An empty row key works, as before per-row leases.
func TestAnEmptyRowKeyLoads(t *testing.T) {
	Load(t, failing, flaky{}, map[string]any{"tools": &tools{title: "x"}}, Row(""))
}

// A row that fails to load gives its leases back: a row of the same key
// loads afterwards.
func TestAFailedLoadGivesItsLeasesBack(t *testing.T) {
	first := Load(t, failing, flaky{}, map[string]any{"tools": &tools{title: "x"}}, Row("first"))
	if message := fatalOf(func(tb testing.TB) {
		first.h.t = tb
		defer func() { first.h.t = t }()
		first.Load(flaky{Fail: true}, Row("again"))
	}, t); !strings.Contains(message, "failing as asked") {
		t.Fatalf("%q", message)
	}
	first.Load(flaky{}, Row("again"))
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
