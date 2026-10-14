package rutis

import (
	"context"
	"errors"
	"sync"
	"sync/atomic"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

func eventRows(t *testing.T) (*runtimeState, *Ctx, *Ctx) {
	t.Helper()
	var contexts []*Ctx
	def := Define(Plugin[NoConfig]{Name: "events", Apply: func(c *Ctx, _ NoConfig) error { contexts = append(contexts, c); return nil }})
	rt := newRuntime([]*Definition{def})
	for _, key := range []string{"a", "b"} {
		if err := rt.load(context.Background(), key, "events", nil, nil, nil); err != nil {
			t.Fatal(err)
		}
	}
	t.Cleanup(func() { _ = rt.dispose(context.Background()) })
	return rt, contexts[0], contexts[1]
}

// guarantee: sdk/leaf-events (registration, disposal, unload)
func TestEventRegistrationLifecycle(t *testing.T) {
	rt, a, b := eventRows(t)
	ctx := context.Background()
	var calls atomic.Int64
	listen := func(context.Context, ...any) error { calls.Add(1); return nil }
	a.On("changed", listen)
	off := b.On("changed", listen)
	b.On("changed", listen)
	if err := a.Emit(ctx, "changed"); err != nil || calls.Load() != 3 {
		t.Fatalf("emit: %v, calls %d", err, calls.Load())
	}
	off()
	off()
	if err := a.Emit(ctx, "changed"); err != nil || calls.Load() != 5 {
		t.Fatalf("off: %v, calls %d", err, calls.Load())
	}
	a.On("unload", func(ctx context.Context, _ ...any) error { return rt.unload(ctx, "b") })
	b.On("unload", listen)
	if err := a.Emit(ctx, "unload"); err != nil || calls.Load() != 5 {
		t.Fatalf("inactive snapshot listener ran: %v, calls %d", err, calls.Load())
	}
	if err := a.Emit(ctx, "changed"); err != nil || calls.Load() != 6 {
		t.Fatalf("unload: %v, calls %d", err, calls.Load())
	}
	for name, owner := range map[string]*Ctx{"changed": b, "internal/update": a} {
		if _, err := peer.Protect(func() (any, error) { owner.On(name, listen); return nil, nil }); err == nil {
			t.Fatalf("inactive/reserved registration accepted: %s", name)
		}
	}
	if err := a.Emit(ctx, "internal/update"); err == nil {
		t.Fatal("internal emission accepted")
	}
	first := errors.New("first listener")
	a.On("fails", func(context.Context, ...any) error { return first })
	a.On("fails", listen)
	if err := a.Emit(ctx, "fails"); !errors.Is(err, first) || calls.Load() != 6 {
		t.Fatalf("local emission did not stop at first error: %v", err)
	}
	var workers sync.WaitGroup
	for range 4 {
		workers.Add(1)
		go func() {
			defer workers.Done()
			for range 100 {
				off := a.On("changed", listen)
				_ = a.Emit(ctx, "changed")
				off()
			}
		}()
	}
	workers.Wait()
}

// guarantee: sdk/leaf-events (host emission waits; cleanups run last,
// a running listener delays them)
func TestHostEmissionWaitsForRowCancellation(t *testing.T) {
	rt, a, b := eventRows(t)
	caller := context.Background()
	first := errors.New("first")
	a.On("changed", func(context.Context, ...any) error { return first })
	started := make(chan struct{}, 1)
	var finished atomic.Bool
	b.On("changed", func(ctx context.Context, _ ...any) error {
		started <- struct{}{}
		<-ctx.Done()
		finished.Store(true)
		return ctx.Err()
	})
	b.Effect(func(context.Context) error {
		if !finished.Load() {
			return errors.New("cleanup ran before the cancelled listener finished")
		}
		return nil
	})
	_, err := rt.control(caller, "mount", []any{map[string]any{"emits": []string{"changed"}}})
	if err != nil {
		t.Fatal(err)
	}
	// The sentence of rutis's own check (Process::mount), naming every internal event.
	_, err = rt.control(caller, "mount", []any{map[string]any{"emits": []string{"changed", "internal/service", "internal/status"}}})
	if want := "framework-internal events do not cross the boundary (mount emits): internal/service, internal/status"; err == nil || err.Error() != want {
		t.Fatalf("a mount declaring internal events: %v", err)
	}
	if _, err := rt.control(caller, "emit", []any{"undeclared", []any{}}); err == nil {
		t.Fatal("undeclared emission accepted")
	}
	done := make(chan error, 1)
	go func() { _, err := rt.control(caller, "emit", []any{"changed", []any{}}); done <- err }()
	testwait.Within(t, "event listener starts", started)
	select {
	case err := <-done:
		t.Fatalf("returned before listener settled: %v", err)
	default:
	}
	if err := rt.unload(caller, "b"); err != nil {
		t.Fatal(err)
	}
	err = testwait.Within(t, "cancelled listener settles", done)
	var aggregate *AggregateError
	if !errors.As(err, &aggregate) || len(aggregate.Errors) != 2 || aggregate.Errors[0] != first || !errors.Is(aggregate.Errors[1], context.Canceled) {
		t.Fatalf("errors lost or reordered: %v", err)
	}
}

// guarantee: sdk/leaf-events (failed load and update)
func TestFailedApplyAndUpdateRemoveListeners(t *testing.T) {
	type config struct {
		Label string
		Fail  bool
	}
	var mu sync.Mutex
	var seen []string
	def := Define(Plugin[config]{Name: "events", Apply: func(c *Ctx, config config) error {
		c.On("changed", func(context.Context, ...any) error {
			mu.Lock()
			defer mu.Unlock()
			seen = append(seen, config.Label)
			return nil
		})
		if config.Fail {
			return errors.New("apply failed")
		}
		return nil
	}})
	rt := newRuntime([]*Definition{def})
	ctx := context.Background()
	t.Cleanup(func() { _ = rt.dispose(ctx) })
	if err := rt.load(ctx, "a", "events", map[string]any{"label": "first"}, nil, nil); err != nil {
		t.Fatal(err)
	}
	if err := rt.update(ctx, "a", map[string]any{"label": "updated"}); err != nil {
		t.Fatal(err)
	}
	if err := rt.load(ctx, "f", "events", map[string]any{"label": "failed", "fail": true}, nil, nil); err == nil {
		t.Fatal("the failing apply loaded")
	}
	if _, err := rt.control(ctx, "mount", []any{map[string]any{"emits": []string{"changed"}}}); err != nil {
		t.Fatal(err)
	}
	if _, err := rt.control(ctx, "emit", []any{"changed", []any{}}); err != nil {
		t.Fatal(err)
	}
	if len(seen) != 1 || seen[0] != "updated" {
		t.Fatalf("listeners of the old config or the failed row remain: %v", seen)
	}
}
