package rutis_test

import (
	"context"
	"errors"
	"fmt"
	"slices"
	"strings"
	"sync"
	"testing"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/advanced"
	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
	"github.com/arcships/rutis/go/rutis/rutistest"
)

type eventHost struct{ chains chan []string }

func (h eventHost) Check(ctx context.Context, callback func(context.Context) error) error {
	h.chains <- peer.ChainOf(ctx)
	return callback(ctx)
}

// guarantee: sdk/leaf-events (host dispatch reaches the runtime's
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
			c.On("failed", func(context.Context, ...any) (any, error) {
				return nil, &rutis.RemoteError{Name: "TypeError", Message: "first"}
			})
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
	// Through the host and back once more: the plugin's own emission goes
	// through the host too.
	if chain := testwait.Within(t, "reentrant emission", local); len(chain) != 4 {
		t.Fatalf("local listener chain: %v", chain)
	}
	err := loaded.Parallel(ctx, "failed")
	var aggregate *advanced.AggregateError
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
		Waterfall func(context.Context, string, rutis.Terminal, ...any) (any, error)
		Bail      func(context.Context, string, ...any) (any, error)
	}{e.c.Emit, e.c.Parallel, e.c.Serial, e.c.Waterfall, e.c.Bail}
	if keep != "" {
		filtered := e.c.Filter(func(l rutis.Listener) (bool, error) {
			e.add("filter:" + l.Plugin + "/" + l.Runtime)
			return l.Plugin == keep, nil
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
		return emitter.Waterfall(ctx, name, func(context.Context, ...any) (any, error) { e.add("terminal"); return "z", nil })
	case "waterfall-args":
		// The terminal gets the event's arguments, also called by the host.
		return emitter.Waterfall(ctx, name, func(_ context.Context, args ...any) (any, error) {
			return fmt.Sprintf("%d:%v|%v", len(args), args[0], args[1]), nil
		}, "p", 2)
	}
	return nil, fmt.Errorf("mode %s", mode)
}

// EmitN emits `name` with n, from the row.
func (e *emitting) EmitN(ctx context.Context, name string, n int) error {
	return e.c.Emit(ctx, name, n)
}

func linking(text string) func(context.Context, ...any) (any, error) {
	return func(ctx context.Context, args ...any) (any, error) {
		rest, err := args[len(args)-1].(advanced.Next)(ctx)
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
			c.On("t/par", func(context.Context, ...any) (any, error) {
				return nil, &rutis.RemoteError{Name: "PluginErr", Message: "p"}
			})
			c.On("t/switch", func(_ context.Context, args ...any) (any, error) {
				state.add(fmt.Sprint("switch:", args[0]))
				return nil, nil
			})
			c.Provide("emitter", state)
			return nil
		},
	})
	return rutistest.Load(t, plugin, nil, nil), state
}

// guarantee: sdk/leaf-events (a listener elsewhere sends the plugin's
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
	if value, err := run("waterfall-args", "t/wf", ""); err != nil || value != "at2:p|2" {
		t.Fatalf("the host calls the terminal with the event's arguments: %v %v", value, err)
	}
	if value, err := loaded.Waterfall(ctx, "t/wf", func(_ context.Context, args ...any) (any, error) {
		return fmt.Sprint(args...), nil
	}, "q"); err != nil || value != "atq" {
		t.Fatalf("the testing tool's terminal gets the event's arguments: %v %v", value, err)
	}
	loaded.On("t/par", func(context.Context, ...any) (any, error) {
		return nil, &rutis.RemoteError{Name: "TestErr", Message: "t"}
	})
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
	if got := fmt.Sprint(state.Log()); got != "[filter:emitting/test filter:rutistest/test]" {
		t.Fatalf("the filter ran in the emitter's process, once per listener, before any: %s", got)
	}
}

// guarantee: sdk/leaf-events (Parallel's failures arrive as an
// *AggregateError in registration order)
func TestAggregateErrorThroughTheHost(t *testing.T) {
	failing := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name: "failing",
		Apply: func(c *rutis.Ctx, _ rutis.NoConfig) error {
			c.On("t/par", func(context.Context, ...any) (any, error) {
				return nil, &rutis.RemoteError{Name: "PluginErr", Message: "p"}
			})
			c.Provide("emitter", &aggregating{c})
			return nil
		},
	})
	loaded := rutistest.Load(t, failing, nil, nil)
	loaded.On("t/par", func(context.Context, ...any) (any, error) {
		return nil, &rutis.RemoteError{Name: "TestErr", Message: "t"}
	})
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
	var aggregate *advanced.AggregateError
	if !errors.As(err, &aggregate) {
		return nil, fmt.Errorf("not an aggregate: %v", err)
	}
	names := []string{aggregate.Message}
	for _, failure := range aggregate.Errors {
		names = append(names, failure.(*rutis.RemoteError).Name)
	}
	return names, nil
}

// guarantee: sdk/leaf-events (an emit through the host, then one on the
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

// guarantee: sdk/leaf-events (the testing tool unloads a row while its
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

// guarantee: sdk/leaf-events (descriptors name the row by its id; the
// runtime waits for running listeners up to RUTIS_SHUTDOWN_TIMEOUT)
func TestTheTestingToolsDescriptorsAndDeadline(t *testing.T) {
	t.Setenv("RUTIS_SHUTDOWN_TIMEOUT", "0.05")
	started := make(chan struct{}, 1)
	stuck := make(chan struct{})
	defer close(stuck)
	plugin := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name: "stuck",
		Apply: func(c *rutis.Ctx, _ rutis.NoConfig) error {
			c.On("t/stuck", func(context.Context, ...any) (any, error) { started <- struct{}{}; <-stuck; return nil, nil })
			return nil
		},
	})
	loaded := rutistest.Load(t, plugin, nil, nil)
	ctx := context.Background()
	var seen []string
	keep := func(l rutis.Listener) (bool, error) {
		seen = append(seen, l.Plugin+"/"+l.Row)
		return false, nil
	}
	if _, err := loaded.Filter(keep).Serial(ctx, "t/stuck"); err != nil {
		t.Fatal(err)
	}
	if fmt.Sprint(seen) != "[stuck/stuck]" {
		t.Fatalf("descriptors %v", seen)
	}
	if err := loaded.Emit(ctx, "t/stuck"); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the listener starts", started)
	loaded.Unload()
	want := "rutis: listeners of row stuck still running after 0.05s; unloading anyway"
	if got := loaded.Reported(); !slices.Contains(got, want) {
		t.Fatalf("reported %q", got)
	}
}

// guarantee: sdk/leaf-events (the testing tool sees what the runtime
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
	want := "rutis: listener of t/emit-error in row reporting failed: Boom: a failed"
	testwait.Eventually(t, "the failure is reported", func() bool {
		for _, line := range loaded.Reported() {
			if line == want {
				return true
			}
		}
		return false
	}, func() string { return fmt.Sprint(loaded.Reported()) })
}
