package rutistest

import (
	"context"
	"errors"
	"fmt"
	"slices"
	"strings"
	"sync"
	"testing"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// The edge cases of rutistest's copies (#654 review): a plugin whose
// parts behave as each test needs.

type edgeConfig struct {
	// The part hands a sub-part to its own `parts` (a copy of a copy).
	Nest bool `json:"nest"`
	// The part's cleanup fails.
	FailCleanup bool `json:"failCleanup"`
	// The part fails to load.
	Fail bool `json:"fail"`
}

// edgePart is a part called `name` (none: "") providing `seen`, reporting
// a failing listener (a line naming its row's id); with `nest`, it hands a
// sub-part over. It injects `tools` and `inject`.
func edgePart(name string, inject ...string) *rutis.Definition {
	return rutis.Define(rutis.Plugin[edgeConfig]{
		Name:     name,
		Inject:   append([]string{"tools"}, inject...),
		Provides: rutis.Provides{"seen": rutis.MethodsOf[*seen]()},
		Apply: func(ctx *rutis.Ctx, config edgeConfig) error {
			if config.Fail {
				return errors.New("failing as asked")
			}
			ctx.On("edge/boom", func(context.Context, ...any) (any, error) { return nil, errors.New("boom") })
			if err := ctx.Emit(ctx, "edge/boom"); err != nil {
				return err
			}
			if config.FailCleanup {
				ctx.Effect(func(context.Context) error { return errors.New("cleanup failed as asked") })
			}
			if config.Nest {
				var tools struct {
					Hand func(context.Context, *rutis.Definition) error
				}
				if err := ctx.Use("tools", &tools); err != nil {
					return err
				}
				if err := tools.Hand(ctx, edgePart("")); err != nil {
					return err
				}
			}
			ctx.Provide("seen", &seen{"edge"})
			return nil
		},
	})
}

// edgeTools is a part's `tools`: it keeps what a part hands to it.
type edgeTools struct {
	title string
	mu    sync.Mutex
	defs  []*rutis.Definition
}

func (t *edgeTools) Title(context.Context) (string, error) { return t.title, nil }

func (t *edgeTools) Hand(_ context.Context, def *rutis.Definition) error {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.defs = append(t.defs, def)
	return nil
}

func (t *edgeTools) handed() []*rutis.Definition {
	t.mu.Lock()
	defer t.mu.Unlock()
	return append([]*rutis.Definition(nil), t.defs...)
}

type edgeOrigin struct {
	// Its cleanup hands another part over.
	HandInCleanup bool `json:"handInCleanup"`
	// The name of the service with its tools' title (edge-seen).
	Seen string `json:"seen"`
	// The part it registers injects a name no host can lease.
	BadInject bool `json:"badInject"`
}

// badName is a service name with a NUL: a runtime refuses to lease it.
const badName = "bad\x00name"

// edge registers a part, answers `edge/ask` with another, reads its
// `tools` title into `edge-seen`, and with handInCleanup registers one
// more in its cleanup.
var edge = rutis.Define(rutis.Plugin[edgeOrigin]{
	Name:   "edge",
	Inject: []string{"parts", "tools"},
	Apply: func(ctx *rutis.Ctx, config edgeOrigin) error {
		var registry struct {
			Register func(ctx context.Context, scope string, def *rutis.Definition) error
		}
		if err := ctx.Use("parts", &registry); err != nil {
			return err
		}
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
		name := config.Seen
		if name == "" {
			name = "edge-seen"
		}
		ctx.Provide(name, &seen{title})
		// Asked with "named", the answer is a part with a name.
		ctx.On("edge/ask", func(_ context.Context, args ...any) (any, error) {
			if len(args) > 0 && args[0] == "named" {
				return edgePart("named-part"), nil
			}
			return edgePart(""), nil
		})
		if config.HandInCleanup {
			ctx.Effect(func(c context.Context) error { return registry.Register(c, "session", edgePart("")) })
		}
		if config.BadInject {
			return registry.Register(ctx, "session", edgePart("", badName))
		}
		return registry.Register(ctx, "session", edgePart(""))
	},
})

func loadEdge(t *testing.T, config edgeOrigin, opts ...Option) (*Loaded, *parts) {
	t.Helper()
	registry := &parts{}
	return Load(t, edge, config, map[string]any{"parts": registry, "tools": &edgeTools{title: "shared"}}, opts...), registry
}

// A definition a row hands over in its cleanup ends with the row (R6).
func TestADefinitionHandedOverInACleanupEnds(t *testing.T) {
	handing, registry := loadEdge(t, edgeOrigin{HandInCleanup: true})
	handing.Unload()
	defs := registry.all()
	if len(defs) != 2 {
		t.Fatalf("registered %d definitions", len(defs))
	}
	if message := fatalOf(func(tb testing.TB) {
		Load(tb, defs[1], edgeConfig{}, map[string]any{"tools": &edgeTools{title: "A"}})
	}, t); !strings.Contains(message, ended) {
		t.Fatalf("%q", message)
	}
}

// Update is a restart: what the old generation hands over in its cleanup
// ends with it; the new generation's definition loads.
func TestUpdateIsARestart(t *testing.T) {
	handing, registry := loadEdge(t, edgeOrigin{HandInCleanup: true})
	handing.Update(edgeOrigin{})
	defs := registry.all()
	if len(defs) != 3 {
		t.Fatalf("registered %d definitions", len(defs))
	}
	if message := fatalOf(func(tb testing.TB) {
		Load(tb, defs[1], edgeConfig{}, map[string]any{"tools": &edgeTools{title: "A"}})
	}, t); !strings.Contains(message, ended) {
		t.Fatalf("%q", message)
	}
	Load(t, defs[2], edgeConfig{}, map[string]any{"tools": &edgeTools{title: "B"}}, Row("part-B"))
}

// ask is a definition the test got from an emission of its own: its row
// is not known. With "named", the definition has a name.
func ask(t *testing.T, loaded *Loaded, args ...any) *rutis.Definition {
	t.Helper()
	answer, err := loaded.Serial(context.Background(), "edge/ask", args...)
	if err != nil {
		t.Fatal(err)
	}
	var def *rutis.Definition
	if err := convert.Decode(answer, &def); err != nil {
		t.Fatal(err)
	}
	return def
}

// A row of key "" is not "no row": unloading it leaves the copies whose
// row is not known.
func TestAnEmptyRowKeyIsNotAnUnknownOrigin(t *testing.T) {
	first, _ := loadEdge(t, edgeOrigin{})
	empty := first.Load(edgeOrigin{Seen: "edge-seen-empty"}, Row(""))
	unknown := Load(t, ask(t, first), edgeConfig{}, map[string]any{"tools": &edgeTools{title: "U"}}, Row("part-U"))
	empty.Unload()
	if provided := unknown.Provided(); !slices.Contains(provided, "seen") {
		t.Fatalf("the copy whose row is not known was unloaded: it provides %v", provided)
	}
}

// reportedIn is whether a copy's listener failure was reported naming the
// row `id`.
func reportedIn(loaded *Loaded, id string) bool {
	return slices.ContainsFunc(loaded.Reported(), func(line string) bool {
		return strings.Contains(line, "listener of edge/boom in row "+id+" failed")
	})
}

// What descriptors call a copy's row, as the host does (R5): the id of
// the row its definition belongs to, a copy of a copy by the first row's,
// and one whose row is not known by its name, or "plugin".
func TestACopysIDIsItsFirstOriginsOrPlugin(t *testing.T) {
	handing, registry := loadEdge(t, edgeOrigin{}, Row("edge-row"))
	own := &edgeTools{title: "A"}
	Load(t, registry.first(t), edgeConfig{Nest: true}, map[string]any{"tools": own}, Row("part-A"))
	nested := own.handed()
	if len(nested) != 1 {
		t.Fatalf("handed %d definitions", len(nested))
	}
	Load(t, nested[0], edgeConfig{}, map[string]any{"tools": &edgeTools{title: "N"}}, Row("part-N"))
	Load(t, ask(t, handing), edgeConfig{}, map[string]any{"tools": &edgeTools{title: "U"}}, Row("part-U"))
	Load(t, ask(t, handing, "named"), edgeConfig{}, map[string]any{"tools": &edgeTools{title: "V"}}, Row("part-V"))
	reported := func(id string) bool { return reportedIn(handing, id) }
	testwait.Eventually(t, "the copies' failures reported", func() bool {
		return reported("edge-row") && reported("plugin") && reported("named-part")
	}, func() string { return strings.Join(handing.Reported(), "\n") })
	for _, id := range []string{"part-A", "part-N", "part-U", "part-V"} {
		if reported(id) {
			t.Fatalf("a copy is called %s:\n%s", id, strings.Join(handing.Reported(), "\n"))
		}
	}
}

// A copy whose known row has the key "" is called by its definition's
// name, or "plugin": an empty id would make the runtime call it by its own
// key.
func TestACopyOfARowWithAnEmptyKeyIsCalledPlugin(t *testing.T) {
	handing, registry := loadEdge(t, edgeOrigin{}, Row(""))
	Load(t, registry.first(t), edgeConfig{}, map[string]any{"tools": &edgeTools{title: "A"}}, Row("part-A"))
	testwait.Eventually(t, "the copy's failure reported", func() bool { return reportedIn(handing, "plugin") },
		func() string { return strings.Join(handing.Reported(), "\n") })
	if reportedIn(handing, "part-A") {
		t.Fatalf("the copy is called by its own key:\n%s", strings.Join(handing.Reported(), "\n"))
	}
}

// A copy that fails to load (its config, a lease, or its apply) forgets
// the services given to it and gives back its leases: a later row of the
// same key uses the test's.
func TestAFailedCopyForgetsItsServices(t *testing.T) {
	for _, failure := range []struct {
		name    string
		origin  edgeOrigin
		config  any
		message string
	}{
		{"config", edgeOrigin{}, make(chan int), "config"},
		{"lease", edgeOrigin{BadInject: true}, edgeConfig{}, "provide"},
		{"load", edgeOrigin{}, edgeConfig{Fail: true}, "failing as asked"},
	} {
		t.Run(failure.name, func(t *testing.T) {
			handing, registry := loadEdge(t, failure.origin)
			own := map[string]any{"tools": &edgeTools{title: "stale"}, badName: &edgeTools{title: "bad"}}
			if message := fatalOf(func(tb testing.TB) {
				Load(tb, registry.first(t), failure.config, own, Row("x"))
			}, t); !strings.Contains(message, failure.message) {
				t.Fatalf("%q", message)
			}
			// Nothing given to it is left, a name holding a NUL neither.
			handing.h.mu.Lock()
			for id := range handing.h.own {
				if strings.HasSuffix(id, leaseLabel("x")) {
					t.Errorf("left behind: %q", id)
				}
			}
			handing.h.mu.Unlock()
			again := handing.Load(edgeOrigin{Seen: "edge-seen-x"}, Row("x"))
			var title string
			if err := again.Service("edge-seen-x").Call(context.Background(), "line", &title); err != nil {
				t.Fatal(err)
			}
			if title != "shared" {
				t.Fatalf("the row of key x uses %q, want the test's tools", title)
			}
		})
	}
}

// A copy's unload failures are the TB's it was loaded with.
func TestACopysUnloadFailureIsTheCallers(t *testing.T) {
	_, registry := loadEdge(t, edgeOrigin{})
	failed := &recorder{TB: t}
	copy := Load(failed, registry.first(t), edgeConfig{FailCleanup: true}, map[string]any{"tools": &edgeTools{title: "A"}})
	copy.Unload()
	if !slices.ContainsFunc(failed.errors, func(line string) bool { return strings.Contains(line, "cleanup failed as asked") }) {
		t.Fatalf("%q", failed.errors)
	}
}

// recorder records Errorf instead of failing the test.
type recorder struct {
	testing.TB
	mu     sync.Mutex
	errors []string
}

func (r *recorder) Errorf(format string, args ...any) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.errors = append(r.errors, fmt.Sprintf(format, args...))
}

func (r *recorder) Helper() {}
