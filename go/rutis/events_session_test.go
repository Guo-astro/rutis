package rutis_test

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"strings"
	"sync"
	"testing"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
	"github.com/arcships/rutis/go/rutis/rutistest"
)

type eventHost struct{ chains chan []string }

func (h eventHost) Check(ctx context.Context, callback func(context.Context) error) error {
	h.chains <- peer.ChainOf(ctx)
	return callback(ctx)
}

// guarantee: events/session (host dispatch reaches the runtime's
// listeners; call chains; failures aggregated in registration order)
func TestEventsCrossTheSessionAndKeepReentrantCallChains(t *testing.T) {
	chains := make(chan []string, 1)
	local := make(chan []string, 1)
	plugin := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name: "events", Inject: []string{"check"},
		Apply: func(c *rutis.Ctx, _ rutis.NoConfig) error {
			var host struct {
				Check func(context.Context, func(context.Context) error) error
			}
			if err := c.Use("check", &host); err != nil {
				return err
			}
			c.On("local", func(ctx context.Context, args ...any) (any, error) {
				if payload, ok := args[0].(map[string]any); !ok || payload["city"] != "Bergen" {
					return nil, fmt.Errorf("payload was not decoded: %T %v", args[0], args[0])
				}
				local <- peer.ChainOf(ctx)
				return nil, nil
			})
			c.On("changed", func(ctx context.Context, args ...any) (any, error) {
				return nil, host.Check(ctx, func(ctx context.Context) error { return c.Parallel(ctx, "local", args...) })
			})
			c.On("failed", func(context.Context, ...any) (any, error) { return nil, &rutis.RemoteError{Name: "TypeError", Message: "first"} })
			c.On("failed", func(context.Context, ...any) (any, error) { panic("second") })
			return nil
		},
	})
	loaded := rutistest.Load(t, plugin, nil, map[string]any{"check": eventHost{chains}})
	ctx := context.Background()
	if err := loaded.Parallel(ctx, "changed", map[string]any{"city": "Bergen"}); err != nil {
		t.Fatal(err)
	}
	if chain := testwait.Within(t, "host event callback", chains); len(chain) != 2 {
		t.Fatalf("host callback chain: %v", chain)
	}
	if chain := testwait.Within(t, "reentrant local emission", local); len(chain) != 3 {
		t.Fatalf("local listener chain: %v", chain)
	}
	err := loaded.Parallel(ctx, "failed")
	var aggregate *rutis.AggregateError
	if !errors.As(err, &aggregate) || len(aggregate.Errors) != 2 || aggregate.Message != "2 listeners of failed failed" {
		t.Fatalf("aggregate: %v", err)
	}
	one, two := aggregate.Errors[0].(*rutis.RemoteError), aggregate.Errors[1].(*rutis.RemoteError)
	if one.Name != "TypeError" || one.Message != "first" || two.Name != "Panic" || two.Message != "panic: second" {
		t.Fatalf("child errors lost or reordered: %v", err)
	}
}

// emitting is a plugin whose service `emitter` emits from its row in any
// mode, and which listens with a few listeners of its own.
type emitting struct {
	c        *rutis.Ctx
	mu       sync.Mutex
	log      []string
	switched chan struct{}
}

func (e *emitting) add(entry string) {
	e.mu.Lock()
	e.log = append(e.log, entry)
	e.mu.Unlock()
}

func (e *emitting) Log() []string {
	e.mu.Lock()
	defer e.mu.Unlock()
	return append([]string{}, e.log...)
}

func (e *emitting) Run(ctx context.Context, mode, name string, keep string) (any, error) {
	emitter := struct {
		Emit      func(context.Context, string, ...any) error
		Parallel  func(context.Context, string, ...any) error
		Serial    func(context.Context, string, ...any) (any, error)
		Waterfall func(context.Context, string, rutis.Next, ...any) (any, error)
		Bail      func(context.Context, string, ...any) (any, error)
	}{e.c.Emit, e.c.Parallel, e.c.Serial, e.c.Waterfall, e.c.Bail}
	if keep != "" {
		filtered := e.c.Filter(func(l rutis.Listener) bool {
			e.add("filter:" + l.Plugin + "/" + l.Runtime)
			return l.Plugin == keep
		})
		emitter.Emit, emitter.Parallel, emitter.Serial, emitter.Waterfall, emitter.Bail = filtered.Emit, filtered.Parallel, filtered.Serial, filtered.Waterfall, filtered.Bail
	}
	switch mode {
	case "emit":
		return nil, emitter.Emit(ctx, name, 1)
	case "parallel":
		return nil, emitter.Parallel(ctx, name)
	case "serial":
		return emitter.Serial(ctx, name)
	case "bail":
		return emitter.Bail(ctx, name)
	case "waterfall":
		return emitter.Waterfall(ctx, name, func(context.Context) (any, error) { e.add("terminal"); return "z", nil })
	}
	return nil, fmt.Errorf("mode %s", mode)
}

// EmitN emits `name` with n, from the row.
func (e *emitting) EmitN(ctx context.Context, name string, n int) error {
	return e.c.Emit(ctx, name, n)
}

func linking(text string) func(context.Context, ...any) (any, error) {
	return func(ctx context.Context, args ...any) (any, error) {
		rest, err := args[len(args)-1].(rutis.Next)(ctx)
		if err != nil {
			return nil, err
		}
		if rest == nil {
			rest = ""
		}
		return text + rest.(string), nil
	}
}

func loadEmitting(t *testing.T) (*rutistest.Loaded, *emitting) {
	state := &emitting{}
	plugin := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name: "emitting",
		Apply: func(c *rutis.Ctx, _ rutis.NoConfig) error {
			state.c = c
			c.On("t/serial", func(context.Context, ...any) (any, error) { state.add("plugin"); return nil, nil })
			c.On("t/wf", linking("a"))
			c.On("t/par", func(context.Context, ...any) (any, error) { return nil, &rutis.RemoteError{Name: "PluginErr", Message: "p"} })
			c.On("t/switch", func(_ context.Context, args ...any) (any, error) { state.add(fmt.Sprint("switch:", args[0])); return nil, nil })
			c.Provide("emitter", state)
			return nil
		},
	})
	return rutistest.Load(t, plugin, nil, nil), state
}

// guarantee: events/via-host (a listener elsewhere sends the plugin's
// emissions through the host; the result is the fast path's)
func TestPluginEmissionsGoThroughTheHostWhenListenedElsewhere(t *testing.T) {
	loaded, state := loadEmitting(t)
	ctx := context.Background()
	service := loaded.Service("emitter")
	run := func(mode, name, keep string) (any, error) {
		var value any
		err := service.Call(ctx, "run", &value, mode, name, keep)
		return value, err
	}
	loaded.On("t/serial", func(context.Context, ...any) (any, error) { return "T", nil })
	if value, err := run("serial", "t/serial", ""); err != nil || value != "T" || fmt.Sprint(state.Log()) != "[plugin]" {
		t.Fatalf("serial through the host: %v %v %v", value, err, state.Log())
	}
	if value, err := run("bail", "t/serial", ""); err != nil || value != "T" {
		t.Fatalf("bail through the host: %v %v", value, err)
	}
	loaded.On("t/wf", linking("t"))
	if value, err := run("waterfall", "t/wf", ""); err != nil || value != "atz" {
		t.Fatalf("waterfall through the host: %v %v", value, err)
	}
	loaded.On("t/par", func(context.Context, ...any) (any, error) { return nil, &rutis.RemoteError{Name: "TestErr", Message: "t"} })
	_, err := run("parallel", "t/par", "")
	if err == nil || !strings.Contains(err.Error(), "2 listeners of t/par failed") {
		t.Fatalf("parallel through the host: %v", err)
	}
	state.mu.Lock()
	state.log = nil
	state.mu.Unlock()
	if value, err := run("serial", "t/serial", "rutistest"); err != nil || value != "T" {
		t.Fatalf("filtered through the host: %v %v", value, err)
	}
	if got := fmt.Sprint(state.Log()); got != "[filter:emitting/go filter:rutistest/]" {
		t.Fatalf("the filter ran in the emitter's process, once per listener, before any: %s", got)
	}
}

// guarantee: events/via-host (Parallel's failures arrive as an
// *AggregateError in registration order)
func TestAggregateErrorThroughTheHost(t *testing.T) {
	failing := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name: "failing",
		Apply: func(c *rutis.Ctx, _ rutis.NoConfig) error {
			c.On("t/par", func(context.Context, ...any) (any, error) { return nil, &rutis.RemoteError{Name: "PluginErr", Message: "p"} })
			c.Provide("emitter", &aggregating{c})
			return nil
		},
	})
	loaded := rutistest.Load(t, failing, nil, nil)
	loaded.On("t/par", func(context.Context, ...any) (any, error) { return nil, &rutis.RemoteError{Name: "TestErr", Message: "t"} })
	var names []string
	if err := loaded.Service("emitter").Call(context.Background(), "names", &names); err != nil {
		t.Fatal(err)
	}
	if fmt.Sprint(names) != "[2 listeners of t/par failed PluginErr TestErr]" {
		t.Fatalf("got %v", names)
	}
}

type aggregating struct{ c *rutis.Ctx }

// Names is what the plugin's Parallel returned: the aggregate's message,
// then each failure's name.
func (a *aggregating) Names(ctx context.Context) ([]string, error) {
	err := a.c.Parallel(ctx, "t/par")
	var aggregate *rutis.AggregateError
	if !errors.As(err, &aggregate) {
		return nil, fmt.Errorf("not an aggregate: %v", err)
	}
	names := []string{aggregate.Message}
	for _, failure := range aggregate.Errors {
		names = append(names, failure.(*rutis.RemoteError).Name)
	}
	return names, nil
}

// guarantee: events/emit-order (an emit through the host, then one on the
// fast path, reach the listener in emission order)
func TestEmitOrderAcrossTheFastPath(t *testing.T) {
	loaded, state := loadEmitting(t)
	ctx := context.Background()
	service := loaded.Service("emitter")
	heard := make(chan struct{}, 1)
	off := loaded.On("t/switch", func(context.Context, ...any) (any, error) { heard <- struct{}{}; return nil, nil })
	if err := service.Call(ctx, "emitN", nil, "t/switch", 1); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the test's listener hears it", heard)
	off()
	if err := service.Call(ctx, "emitN", nil, "t/switch", 2); err != nil {
		t.Fatal(err)
	}
	testwait.Eventually(t, "both emits arrive", func() bool { return len(state.Log()) == 2 }, func() string { return fmt.Sprint(state.Log()) })
	if got := fmt.Sprint(state.Log()); got != "[switch:1 switch:2]" {
		t.Fatalf("got %s", got)
	}
}

type scoped struct {
	Label string `json:"label"`
}

// guarantee: events/scope (rows in scopes, the testing tool's emissions
// from a row's scope)
func TestRowsInScopesHearUpNotDown(t *testing.T) {
	var mu sync.Mutex
	var heard []string
	plugin := rutis.Define(rutis.Plugin[scoped]{
		Name: "scoped",
		Apply: func(c *rutis.Ctx, config scoped) error {
			c.On("t/scope", func(context.Context, ...any) (any, error) {
				mu.Lock()
				heard = append(heard, config.Label)
				mu.Unlock()
				return nil, nil
			})
			return nil
		},
	})
	root := rutistest.Load(t, plugin, scoped{"r"}, nil)
	t1 := root.Load(scoped{"u"}, rutistest.InScope("t1", "tenant"))
	s1 := root.Load(scoped{"s"}, rutistest.InScope("t1/s1", "tenant", "session"))
	t2 := root.Load(scoped{"v"}, rutistest.InScope("t2", "tenant"))
	ctx := context.Background()
	for _, step := range []struct {
		from *rutistest.Loaded
		want string
	}{{s1, "[r s u]"}, {t1, "[r u]"}, {t2, "[r v]"}, {root, "[r]"}} {
		mu.Lock()
		heard = nil
		mu.Unlock()
		if err := step.from.Parallel(ctx, "t/scope"); err != nil {
			t.Fatal(err)
		}
		mu.Lock()
		got := append([]string{}, heard...)
		mu.Unlock()
		sortStrings(got)
		if fmt.Sprint(got) != step.want {
			t.Fatalf("from %v: %v, want %s", step.from.Scope(), got, step.want)
		}
	}
	if scope := s1.Scope(); len(scope) != 2 || scope[1] != (rutis.Scope{ID: "t1/s1", Group: "session"}) {
		t.Fatalf("scope %v", scope)
	}
	var seen []rutis.Listener
	if _, err := s1.Filter(func(l rutis.Listener) bool { seen = append(seen, l); return false }).Serial(ctx, "t/scope"); err != nil {
		t.Fatal(err)
	}
	if len(seen) != 3 || seen[2].Row != "test-3" || seen[2].Runtime != "go" || len(seen[2].Scope) != 2 {
		t.Fatalf("descriptors %+v", seen)
	}
}

func sortStrings(list []string) {
	for i := range list {
		for j := i + 1; j < len(list); j++ {
			if list[j] < list[i] {
				list[i], list[j] = list[j], list[i]
			}
		}
	}
}

// guarantee: events/unload (the testing tool unloads a row while its
// listener runs: cancelled, waited for)
func TestUnloadingARowCancelsItsRunningListener(t *testing.T) {
	started := make(chan struct{}, 1)
	cancelled := make(chan struct{}, 1)
	plugin := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name: "waiting",
		Apply: func(c *rutis.Ctx, _ rutis.NoConfig) error {
			c.On("t/unload", func(ctx context.Context, _ ...any) (any, error) {
				started <- struct{}{}
				<-ctx.Done()
				cancelled <- struct{}{}
				return nil, nil
			})
			return nil
		},
	})
	loaded := rutistest.Load(t, plugin, nil, nil)
	done := make(chan error, 1)
	go func() { done <- loaded.Parallel(context.Background(), "t/unload") }()
	testwait.Within(t, "the listener starts", started)
	loaded.Unload()
	testwait.Within(t, "the listener is cancelled", cancelled)
	testwait.Within(t, "the dispatch settles", done)
	if value, err := loaded.Serial(context.Background(), "t/unload"); err != nil || value != nil {
		t.Fatalf("an unloaded row's listener ran: %v %v", value, err)
	}
}

// guarantee: events/emit-reported (the testing tool sees what the runtime
// reports of a failed emit listener)
func TestEmitFailuresAreReported(t *testing.T) {
	plugin := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name: "reporting",
		Apply: func(c *rutis.Ctx, _ rutis.NoConfig) error {
			c.On("t/emit-error", func(context.Context, ...any) (any, error) {
				return nil, &rutis.RemoteError{Name: "Boom", Message: "a failed"}
			})
			return nil
		},
	})
	loaded := rutistest.Load(t, plugin, nil, nil)
	if err := loaded.Emit(context.Background(), "t/emit-error"); err != nil {
		t.Fatal(err)
	}
	want := "rutis: listener of t/emit-error in row test failed: Boom: a failed"
	testwait.Eventually(t, "the failure is reported", func() bool {
		for _, line := range loaded.Reported() {
			if line == want {
				return true
			}
		}
		return false
	}, func() string { return fmt.Sprint(loaded.Reported()) })
}

// guarantee: events/handshake (a host without events-v2 is refused, with
// the line of §2.2)
func TestAHostOlderThanTheEventProtocolIsRefused(t *testing.T) {
	runtimeSide, hostSide := net.Pipe()
	defer hostSide.Close()
	plugin := rutis.Define(rutis.Plugin[rutis.NoConfig]{Name: "any", Apply: func(*rutis.Ctx, rutis.NoConfig) error { return nil }})
	done := make(chan error, 1)
	go func() { done <- rutis.ServeConn(runtimeSide, plugin) }()
	hello, _ := json.Marshal(map[string]any{"op": "hello", "version": 2, "capabilities": []string{"sync-wait"}})
	go func() {
		buffer := make([]byte, 4096)
		for {
			if _, err := hostSide.Read(buffer); err != nil {
				return
			}
		}
	}()
	if _, err := hostSide.Write(append(hello, '\n')); err != nil {
		t.Fatal(err)
	}
	err := testwait.Within(t, "the runtime stops", done)
	want := "refused: incompatible (this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later); stopping"
	if err == nil || err.Error() != want {
		t.Fatalf("got %v", err)
	}
}
