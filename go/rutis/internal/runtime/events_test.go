package rutis

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"reflect"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/events"
	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// eventRows loads rows of one plugin in the scopes given ("" is the
// root), with no session: every dispatch is on the fast path.
func eventRows(t *testing.T, scopes ...string) (*runtimeState, []*Ctx) {
	t.Helper()
	var mu sync.Mutex
	var contexts []*Ctx
	def := Define(Plugin[NoConfig]{Name: "events", Apply: func(c *Ctx, _ NoConfig) error {
		mu.Lock()
		contexts = append(contexts, c)
		mu.Unlock()
		return nil
	}})
	rt := newRuntime([]*Definition{def})
	for i, path := range scopes {
		var chain []Scope
		if path != "" {
			segments := strings.Split(path, "/")
			for j := range segments {
				chain = append(chain, Scope{ID: strings.Join(segments[:j+1], "/"), Group: "g"})
			}
		}
		if err := rt.load(context.Background(), fmt.Sprintf("r%d", i), "events", nil, nil, nil, chain); err != nil {
			t.Fatal(err)
		}
	}
	t.Cleanup(func() { _ = rt.dispose(context.Background()) })
	return rt, contexts
}

// journal records what listeners did, in order.
type journal struct {
	mu      sync.Mutex
	entries []string
}

func (j *journal) add(entry string) {
	j.mu.Lock()
	j.entries = append(j.entries, entry)
	j.mu.Unlock()
}

func (j *journal) list() []string {
	j.mu.Lock()
	defer j.mu.Unlock()
	return append([]string(nil), j.entries...)
}

func returns(j *journal, label string, value any) func(context.Context, ...any) (any, error) {
	return func(context.Context, ...any) (any, error) { j.add(label); return value, nil }
}

// captureReports collects the lines of §5.6 for the test.
func captureReports(t *testing.T) *journal {
	j := &journal{}
	previous := events.SetReport(j.add)
	t.Cleanup(func() { events.SetReport(previous) })
	return j
}

// guarantee: events/registration (disposers, unload, reserved names)
func TestEventRegistrationLifecycle(t *testing.T) {
	rt, rows := eventRows(t, "", "")
	a, b := rows[0], rows[1]
	ctx := context.Background()
	var calls atomic.Int64
	listen := func(context.Context, ...any) (any, error) { calls.Add(1); return nil, nil }
	a.On("changed", listen)
	off := b.On("changed", listen)
	b.On("changed", listen)
	if err := a.Parallel(ctx, "changed"); err != nil || calls.Load() != 3 {
		t.Fatalf("parallel: %v, calls %d", err, calls.Load())
	}
	off()
	off()
	if err := a.Parallel(ctx, "changed"); err != nil || calls.Load() != 5 {
		t.Fatalf("off: %v, calls %d", err, calls.Load())
	}
	// A listener unloading a later row: the snapshot skips its listener.
	a.On("unload", func(ctx context.Context, _ ...any) (any, error) { return nil, rt.unload(ctx, "r1") })
	b.On("unload", listen)
	if _, err := a.Serial(ctx, "unload"); err != nil || calls.Load() != 5 {
		t.Fatalf("withdrawn snapshot listener ran: %v, calls %d", err, calls.Load())
	}
	if err := a.Parallel(ctx, "changed"); err != nil || calls.Load() != 6 {
		t.Fatalf("unload: %v, calls %d", err, calls.Load())
	}
	if _, err := peer.Protect(func() (any, error) { a.On("internal/update", listen); return nil, nil }); err == nil {
		t.Fatal("reserved registration accepted")
	}
	// The unloaded row: reported, not registered (TestAnUnloadedRowCannotProvide).
	var written bytes.Buffer
	savedOutput := rowOutput
	rowOutput = &written
	b.On("changed", listen)()
	rowOutput = savedOutput
	if !strings.Contains(written.String(), "row r1 is not active") {
		t.Fatalf("inactive registration not reported: %q", written.String())
	}
	if err := a.Parallel(ctx, "changed"); err != nil || calls.Load() != 7 {
		t.Fatalf("inactive registration accepted: %v, calls %d", err, calls.Load())
	}
	a.On("rutis/row", listen)
	if err := a.Emit(ctx, "internal/update"); err == nil {
		t.Fatal("internal emission accepted")
	}
	for _, emit := range []func() error{
		func() error { return a.Emit(ctx, "rutis/row") },
		func() error { return a.Parallel(ctx, "rutis/row") },
		func() error { _, err := a.Serial(ctx, "rutis/row"); return err },
		func() error { _, err := a.Waterfall(ctx, "rutis/row", nil); return err },
		func() error { _, err := a.Bail(ctx, "rutis/row"); return err },
		func() error {
			return a.Filter(func(Listener) (bool, error) { return true, nil }).Emit(ctx, "rutis/row")
		},
	} {
		err := emit()
		if !errors.Is(err, events.ErrReservedEvent) || err.Error() != "rutis/* events are emitted by rutis only: rutis/row" || peer.ErrorName(err) != "ReservedEvent" {
			t.Fatalf("rutis/* emission: %v", err)
		}
	}
	if err := b.Emit(ctx, "changed"); !errors.Is(err, ErrBinding) {
		t.Fatalf("an unloaded row emitted: %v", err)
	}
	var workers sync.WaitGroup
	for range 4 {
		workers.Add(1)
		go func() {
			defer workers.Done()
			for range 100 {
				off := a.On("changed", listen)
				_ = a.Parallel(ctx, "changed")
				_ = a.Emit(ctx, "changed")
				off()
			}
		}()
	}
	workers.Wait()
}

// guarantee: events/emit (returns at once; order per listener; failures
// reported, isolated)
func TestEmitStartsInOrderAndReportsFailures(t *testing.T) {
	reports := captureReports(t)
	_, rows := eventRows(t, "")
	a := rows[0]
	ctx, cancel := context.WithCancel(context.Background())
	j := &journal{}
	release := make(chan struct{})
	a.On("t/emit", func(ctx context.Context, args ...any) (any, error) {
		<-release
		j.add(fmt.Sprintf("slow:%v", args[0]))
		if ctx.Err() != nil {
			return nil, errors.New("an emit's listener saw the emitter's cancellation")
		}
		return nil, nil
	})
	a.On("t/emit", func(_ context.Context, args ...any) (any, error) {
		j.add(fmt.Sprintf("fails:%v", args[0]))
		return nil, peer.Errorf("Boom", "failed on %v", args[0])
	})
	for i := 1; i <= 3; i++ {
		if err := a.Emit(ctx, "t/emit", i); err != nil {
			t.Fatal(err)
		}
	}
	// Emit returned with the slow listener still held, and the emitter
	// giving up does not cancel what it emitted.
	cancel()
	testwait.Eventually(t, "the fast listener ran each emit", func() bool { return len(j.list()) == 3 }, func() string { return fmt.Sprint(j.list()) })
	close(release)
	testwait.Eventually(t, "every emit ran", func() bool { return len(j.list()) == 6 }, func() string { return fmt.Sprint(j.list()) })
	var slow, fails []string
	for _, entry := range j.list() {
		if strings.HasPrefix(entry, "slow:") {
			slow = append(slow, entry)
		} else {
			fails = append(fails, entry)
		}
	}
	if fmt.Sprint(slow) != "[slow:1 slow:2 slow:3]" || fmt.Sprint(fails) != "[fails:1 fails:2 fails:3]" {
		t.Fatalf("emits out of order per listener: %v", j.list())
	}
	want := []string{
		"rutis: listener of t/emit in row r0 failed: Boom: failed on 1",
		"rutis: listener of t/emit in row r0 failed: Boom: failed on 2",
		"rutis: listener of t/emit in row r0 failed: Boom: failed on 3",
	}
	if got := reports.list(); !slices.Equal(got, want) {
		t.Fatalf("reported %q", got)
	}
}

// guarantee: events/parallel (all run; AggregateError in registration
// order, even for one failure)
func TestParallelAggregatesInRegistrationOrder(t *testing.T) {
	_, rows := eventRows(t, "")
	a := rows[0]
	ctx := context.Background()
	a.On("t/par", func(context.Context, ...any) (any, error) {
		time.Sleep(50 * time.Millisecond)
		return nil, peer.Errorf("AErr", "a")
	})
	a.On("t/par", func(context.Context, ...any) (any, error) { return nil, peer.Errorf("BErr", "b") })
	var ran atomic.Bool
	a.On("t/par", func(context.Context, ...any) (any, error) { ran.Store(true); return "C", nil })
	err := a.Parallel(ctx, "t/par")
	var aggregate *peer.AggregateError
	if !errors.As(err, &aggregate) || len(aggregate.Errors) != 2 || peer.ErrorName(aggregate.Errors[0]) != "AErr" || peer.ErrorName(aggregate.Errors[1]) != "BErr" || !ran.Load() {
		t.Fatalf("parallel: %v", err)
	}
	if aggregate.Message != "2 listeners of t/par failed" || peer.Message(aggregate) != aggregate.Message {
		t.Fatalf("message %q", aggregate.Message)
	}
	a.On("t/one", func(context.Context, ...any) (any, error) { panic("boom") })
	err = a.Parallel(ctx, "t/one")
	if !errors.As(err, &aggregate) || len(aggregate.Errors) != 1 || peer.ErrorName(aggregate.Errors[0]) != "Panic" {
		t.Fatalf("one failure is aggregated too: %v", err)
	}
	if err := a.Parallel(ctx, "t/none"); err != nil {
		t.Fatal(err)
	}
}

// guarantee: events/serial, events/bail (first result; nil and false are
// none, 0 is one; an error stops)
func TestSerialAndBailReturnTheFirstResult(t *testing.T) {
	_, rows := eventRows(t, "")
	a := rows[0]
	ctx := context.Background()
	j := &journal{}
	a.On("t/serial", returns(j, "a", nil))
	a.On("t/serial", returns(j, "b", false))
	a.On("t/serial", returns(j, "c", (*int)(nil)))
	a.On("t/serial", returns(j, "d", 0))
	a.On("t/serial", returns(j, "e", "E"))
	for _, emit := range []func(context.Context, string, ...any) (any, error){a.Serial, a.Bail} {
		if value, err := emit(ctx, "t/serial"); err != nil || value != 0 {
			t.Fatalf("serial: %v %v", value, err)
		}
	}
	if got := j.list(); fmt.Sprint(got) != "[a b c d a b c d]" {
		t.Fatalf("ran %v", got)
	}
	a.On("t/none", returns(j, "x", false))
	if value, err := a.Serial(ctx, "t/none"); err != nil || value != nil {
		t.Fatalf("no result: %v %v", value, err)
	}
	a.On("t/error", func(context.Context, ...any) (any, error) { return nil, peer.Errorf("AErr", "a") })
	a.On("t/error", returns(j, "after", "B"))
	if _, err := a.Bail(ctx, "t/error"); peer.ErrorName(err) != "AErr" || slices.Contains(j.list(), "after") {
		t.Fatalf("an error stops: %v %v", err, j.list())
	}
}

// guarantee: events/waterfall (next, veto, terminal, next twice, after the
// end)
func TestWaterfallChainsThroughNext(t *testing.T) {
	_, rows := eventRows(t, "")
	a := rows[0]
	ctx := context.Background()
	link := func(text string) func(context.Context, ...any) (any, error) {
		return func(ctx context.Context, args ...any) (any, error) {
			next := args[len(args)-1].(events.Next)
			rest, err := next(ctx)
			if err != nil {
				return nil, err
			}
			if rest == nil {
				rest = ""
			}
			return text + rest.(string), nil
		}
	}
	a.On("t/wf", link("a"))
	a.On("t/wf", link("b"))
	terminal := func(context.Context, ...any) (any, error) { return "t", nil }
	if value, err := a.Waterfall(ctx, "t/wf", terminal); err != nil || value != "abt" {
		t.Fatalf("chain: %v %v", value, err)
	}
	if value, err := a.Waterfall(ctx, "t/wf", nil); err != nil || value != "ab" {
		t.Fatalf("no terminal: %v %v", value, err)
	}
	if value, err := a.Waterfall(ctx, "t/empty", terminal); err != nil || value != "t" {
		t.Fatalf("no listeners: %v %v", value, err)
	}
	if value, err := a.Waterfall(ctx, "t/empty", nil); err != nil || value != nil {
		t.Fatalf("nothing at all: %v %v", value, err)
	}
	j := &journal{}
	a.On("t/veto", link("a"))
	a.On("t/veto", returns(j, "veto", "v"))
	a.On("t/veto", returns(j, "never", "x"))
	if value, err := a.Waterfall(ctx, "t/veto", terminal); err != nil || value != "av" || slices.Contains(j.list(), "never") {
		t.Fatalf("veto: %v %v %v", value, err, j.list())
	}
	a.On("t/raw", func(context.Context, ...any) (any, error) { return false, nil })
	if value, err := a.Waterfall(ctx, "t/raw", terminal); err != nil || value != false {
		t.Fatalf("the outermost value as it is: %v %v", value, err)
	}
	a.On("t/twice", func(ctx context.Context, args ...any) (any, error) {
		next := args[len(args)-1].(events.Next)
		if _, err := next(ctx); err != nil {
			return nil, err
		}
		return next(ctx)
	})
	a.On("t/twice", func(context.Context, ...any) (any, error) { return "b", nil })
	_, err := a.Waterfall(ctx, "t/twice", nil)
	if !errors.Is(err, events.ErrWaterfallNext) || err.Error() != "next of t/twice was already called" {
		t.Fatalf("next twice: %v", err)
	}
	var late events.Next
	a.On("t/late", func(_ context.Context, args ...any) (any, error) {
		late = args[len(args)-1].(events.Next)
		return "v", nil
	})
	if _, err := a.Waterfall(ctx, "t/late", nil); err != nil {
		t.Fatal(err)
	}
	if _, err := late(ctx); !errors.Is(err, events.ErrWaterfallNext) || err.Error() != "next of t/late was called after its waterfall ended" {
		t.Fatalf("next after the end: %v", err)
	}
	a.On("t/args", func(_ context.Context, args ...any) (any, error) { return len(args), nil })
	if value, err := a.Serial(ctx, "t/args", 1, 2); err != nil || value != 2 {
		t.Fatalf("only a waterfall passes next: %v %v", value, err)
	}
	if value, err := a.Waterfall(ctx, "t/args", nil, 1, 2); err != nil || value != 3 {
		t.Fatalf("a waterfall passes next last: %v %v", value, err)
	}
}

// guarantee: events/flat (events are delivered by name)
func TestListenersHearEveryEvent(t *testing.T) {
	_, rows := eventRows(t, "", "t1", "t1/s1", "t2")
	root, t1, s1, t2 := rows[0], rows[1], rows[2], rows[3]
	ctx := context.Background()
	j := &journal{}
	root.On("t/scope", returns(j, "r", nil))
	t1.On("t/scope", returns(j, "u", nil))
	s1.On("t/scope", returns(j, "s", nil))
	t2.On("t/scope", returns(j, "v", nil))
	for _, step := range []struct {
		from *Ctx
		want string
	}{{s1, "[r u s v]"}, {t1, "[r u s v]"}, {t2, "[r u s v]"}, {root, "[r u s v]"}} {
		j.entries = nil
		if _, err := step.from.Serial(ctx, "t/scope"); err != nil {
			t.Fatal(err)
		}
		if got := fmt.Sprint(j.list()); got != step.want {
			t.Fatalf("from %v: %s, want %s", step.from.Scope(), got, step.want)
		}
	}
	if got := s1.Scope(); len(got) != 2 || got[0].ID != "t1" || got[1].ID != "t1/s1" {
		t.Fatalf("scope %v", got)
	}
}

// guarantee: events/filter (sees descriptors in order before any call;
// Global skips it; Prepend orders first)
func TestFilterGlobalAndPrepend(t *testing.T) {
	_, rows := eventRows(t, "", "t2")
	a, other := rows[0], rows[1]
	ctx := context.Background()
	j := &journal{}
	a.On("t/filter", returns(j, "a", nil))
	a.On("t/filter", returns(j, "g", nil), Global())
	a.On("t/filter", returns(j, "b", nil))
	other.On("t/filter", returns(j, "d", nil))
	a.On("t/filter", returns(j, "p", nil), Prepend())
	var seen []Listener
	keep := func(l Listener) (bool, error) {
		seen = append(seen, l)
		j.add("filter")
		return len(seen) != 2, nil
	}
	if _, err := a.Filter(keep).Serial(ctx, "t/filter"); err != nil {
		t.Fatal(err)
	}
	if got := fmt.Sprint(j.list()); got != "[filter filter filter filter p g b d]" {
		t.Fatalf("ran %s", got)
	}
	if seen[0].Row != "r0" || seen[0].Plugin != "events" || seen[0].Name != "t/filter" || seen[0].Global || len(seen[0].Scope) != 0 {
		t.Fatalf("descriptor %+v", seen[0])
	}
	j.entries = nil
	if err := a.Filter(func(Listener) (bool, error) { panic("no") }).Parallel(ctx, "t/filter"); peer.ErrorName(err) != "Panic" || len(j.list()) != 0 {
		t.Fatalf("a failing filter fails the dispatch before any call: %v %v", err, j.list())
	}
	if _, err := a.Filter(func(Listener) (bool, error) { return false, peer.Errorf("FErr", "no") }).Serial(ctx, "t/filter"); peer.ErrorName(err) != "FErr" || err.Error() != "no" || len(j.list()) != 0 {
		t.Fatalf("the error a filter returns fails the dispatch with it, before any call: %v %v", err, j.list())
	}
	reports := captureReports(t)
	if err := a.Filter(func(Listener) (bool, error) { panic("no") }).Emit(ctx, "t/filter"); err != nil {
		t.Fatal(err)
	}
	if got := reports.list(); len(got) != 1 || !strings.HasPrefix(got[0], "rutis: filter of t/filter from row r0 failed: Panic: ") {
		t.Fatalf("a failing emit filter is reported: %q", got)
	}
}

// guarantee: events/unload (cancels and waits, bounded by the deadline)
func TestUnloadCancelsAndWaitsForListeners(t *testing.T) {
	rt, rows := eventRows(t, "", "")
	a, b := rows[0], rows[1]
	ctx := context.Background()
	started := make(chan struct{}, 1)
	var finished atomic.Bool
	b.On("t/unload", func(ctx context.Context, _ ...any) (any, error) {
		started <- struct{}{}
		<-ctx.Done()
		time.Sleep(20 * time.Millisecond)
		finished.Store(true)
		return nil, ctx.Err()
	})
	b.Effect(func(context.Context) error {
		if !finished.Load() {
			return errors.New("cleanup ran before the cancelled listener finished")
		}
		return nil
	})
	done := make(chan error, 1)
	go func() { done <- a.Parallel(ctx, "t/unload") }()
	testwait.Within(t, "the listener starts", started)
	if err := rt.unload(ctx, "r1"); err != nil {
		t.Fatal(err)
	}
	err := testwait.Within(t, "the dispatch settles", done)
	var aggregate *peer.AggregateError
	if !errors.As(err, &aggregate) || !errors.Is(aggregate.Errors[0], context.Canceled) {
		t.Fatalf("the cancelled listener's failure: %v", err)
	}

	// One that does not stop is given up after the deadline.
	t.Setenv("RUTIS_SHUTDOWN_TIMEOUT", "0.05")
	reports := captureReports(t)
	stuck := make(chan struct{})
	defer close(stuck)
	a.On("t/stuck", func(context.Context, ...any) (any, error) { started <- struct{}{}; <-stuck; return nil, nil })
	if err := a.Emit(ctx, "t/stuck"); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the stuck listener starts", started)
	if err := rt.unload(ctx, "r0"); err != nil {
		t.Fatal(err)
	}
	if got := reports.list(); len(got) != 1 || got[0] != "rutis: listeners of row r0 still running after 0.05s; unloading anyway" {
		t.Fatalf("reported %q", got)
	}
}

// guarantee: events/cancel (an emitter giving up cancels its listeners)
func TestAnEmitterGivingUpCancelsItsListeners(t *testing.T) {
	_, rows := eventRows(t, "")
	a := rows[0]
	started := make(chan struct{}, 1)
	cancelled := make(chan struct{})
	a.On("t/cancel", func(ctx context.Context, _ ...any) (any, error) {
		started <- struct{}{}
		<-ctx.Done()
		close(cancelled)
		return nil, nil
	})
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- a.Parallel(ctx, "t/cancel") }()
	testwait.Within(t, "the listener starts", started)
	cancel()
	if err := testwait.Within(t, "the emitter's call ends", done); !errors.Is(err, context.Canceled) {
		t.Fatalf("parallel: %v", err)
	}
	testwait.Within(t, "the listener is cancelled", cancelled)
}

// guarantee: events/registration (failed load and update)
func TestFailedApplyAndUpdateRemoveListeners(t *testing.T) {
	type config struct {
		Label string
		Fail  bool
	}
	var mu sync.Mutex
	var seen []string
	def := Define(Plugin[config]{Name: "events", Apply: func(c *Ctx, config config) error {
		c.On("changed", func(context.Context, ...any) (any, error) {
			mu.Lock()
			defer mu.Unlock()
			seen = append(seen, config.Label)
			return nil, nil
		})
		if config.Fail {
			return errors.New("apply failed")
		}
		return nil
	}})
	var emitter *Ctx
	emitting := Define(Plugin[NoConfig]{Name: "emitter", Apply: func(c *Ctx, _ NoConfig) error { emitter = c; return nil }})
	rt := newRuntime([]*Definition{def, emitting})
	ctx := context.Background()
	t.Cleanup(func() { _ = rt.dispose(ctx) })
	if err := rt.load(ctx, "a", "events", map[string]any{"label": "first"}, nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	if err := rt.update(ctx, "a", map[string]any{"label": "updated"}); err != nil {
		t.Fatal(err)
	}
	if err := rt.load(ctx, "f", "events", map[string]any{"label": "failed", "fail": true}, nil, nil, nil); err == nil {
		t.Fatal("the failing apply loaded")
	}
	if err := rt.load(ctx, "e", "emitter", nil, nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	if err := emitter.Parallel(ctx, "changed"); err != nil {
		t.Fatal(err)
	}
	if len(seen) != 1 || seen[0] != "updated" {
		t.Fatalf("listeners of the old config or the failed row remain: %v", seen)
	}
}

// guarantee: events/withdrawn (a call for a listener no longer there)
func TestACallForAWithdrawnListenerFails(t *testing.T) {
	rt, rows := eventRows(t, "")
	off := rows[0].On("t/gone", func(context.Context, ...any) (any, error) { return "x", nil })
	call := func() error {
		_, err := rt.control(context.Background(), "events.call", []any{map[string]any{"listener": "1", "dispatch": "d1", "own": false, "name": "t/gone", "mode": "serial", "args": []any{}}})
		return err
	}
	if err := call(); err != nil {
		t.Fatal(err)
	}
	off()
	if err := call(); !errors.Is(err, ErrBinding) || err.Error() != "listener 1 of t/gone is withdrawn" {
		t.Fatalf("withdrawn: %v", err)
	}
}

// guarantee: events/cancel (a cancelled waterfall fails a later next, §5.7)
func TestNextAfterTheEmitterGaveUpFails(t *testing.T) {
	_, rows := eventRows(t, "")
	a := rows[0]
	held := make(chan events.Next, 1)
	a.On("t/wf-cancel", func(ctx context.Context, args ...any) (any, error) {
		held <- args[len(args)-1].(events.Next)
		<-ctx.Done()
		return nil, ctx.Err()
	})
	a.On("t/wf-cancel", func(context.Context, ...any) (any, error) { return "never", nil })
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { _, err := a.Waterfall(ctx, "t/wf-cancel", nil); done <- err }()
	next := testwait.Within(t, "the listener starts", held)
	cancel()
	if err := testwait.Within(t, "the emitter's call ends", done); !errors.Is(err, context.Canceled) {
		t.Fatalf("waterfall: %v", err)
	}
	if _, err := next(context.Background()); !errors.Is(err, events.ErrWaterfallNext) || err.Error() != "next of t/wf-cancel was called after its waterfall ended" {
		t.Fatalf("next after cancellation: %v", err)
	}
}

// refusingHost attaches to `rt` a session whose host refuses every
// `events.on` and `events.dispatch` (answering the rest with undefined),
// or, when `silent`, never answers an `events.dispatch`.
func refusingHost(t *testing.T, rt *runtimeState, silent bool) (end func()) {
	t.Helper()
	a, b := net.Pipe()
	hold := make(chan struct{})
	host, err := peer.New(peer.NewLines(a), peer.Options{Host: true, Dispatch: func(ctx context.Context, target, method string, args []any) (any, error) {
		switch {
		case method == "events.dispatch" && silent:
			<-hold
			return peer.Undefined, nil
		case method == "events.on" || method == "events.dispatch":
			return nil, peer.Errorf("BindingError", "the host refused %s", method)
		}
		return peer.Undefined, nil
	}})
	if err != nil {
		t.Fatal(err)
	}
	runtime, err := peer.New(peer.NewLines(b), peer.Options{Dispatch: rt.dispatch, Arrive: rt.arrive})
	if err != nil {
		t.Fatal(err)
	}
	started := make(chan error, 2)
	for _, p := range []*peer.Peer{host, runtime} {
		go func(p *peer.Peer) { started <- p.Start() }(p)
	}
	for range 2 {
		if err := testwait.Within(t, "the session to start", started); err != nil {
			t.Fatal(err)
		}
	}
	for _, p := range []*peer.Peer{host, runtime} {
		testwait.Within(t, "the handshake", p.Ready())
	}
	rt.mu.Lock()
	rt.session = runtime
	rt.mu.Unlock()
	t.Cleanup(func() {
		close(hold)
		host.Close(nil)
		runtime.Close(nil)
		a.Close()
		b.Close()
	})
	return func() { host.Close(nil); a.Close() }
}

// guarantee: events/reports (§11: an emit through the host refused, a
// registration refused after its row loaded)
func TestRefusedEmitsAndRegistrationsAreReported(t *testing.T) {
	reports := captureReports(t)
	rt, rows := eventRows(t, "")
	a := rows[0]
	refusingHost(t, rt, false)
	a.On("t/late", func(context.Context, ...any) (any, error) { return nil, nil })
	rt.mu.Lock()
	rt.elsewhere["t/e"] = true
	rt.mu.Unlock()
	if err := a.Emit(context.Background(), "t/e"); err != nil {
		t.Fatal(err)
	}
	want := []string{
		"rutis: emit of t/e from row r0 failed: BindingError: the host refused events.dispatch",
		"rutis: registration of listener of t/late in row r0 failed: BindingError: the host refused events.on",
	}
	testwait.Eventually(t, "both reports", func() bool { return len(reports.list()) == 2 }, func() string { return fmt.Sprint(reports.list()) })
	got := reports.list()
	slices.Sort(got)
	if !slices.Equal(got, want) {
		t.Fatalf("got %q", got)
	}
}

// guarantee: events/registration (§11: a registration the host refuses
// during a row load fails the load instead of being reported)
func TestARegistrationRefusedDuringTheLoadFailsIt(t *testing.T) {
	reports := captureReports(t)
	def := Define(Plugin[NoConfig]{Name: "events", Apply: func(c *Ctx, _ NoConfig) error {
		c.On("t/early", func(context.Context, ...any) (any, error) { return nil, nil })
		return nil
	}})
	rt := newRuntime([]*Definition{def})
	t.Cleanup(func() { _ = rt.dispose(context.Background()) })
	refusingHost(t, rt, false)
	err := rt.load(context.Background(), "r0", "events", nil, nil, nil, nil)
	if err == nil || len(reports.list()) != 0 {
		t.Fatalf("the load gave %v and reported %q; §11 wants the load to fail with the refusal and no line", err, reports.list())
	}
}

// guarantee: events/reports (§11: an emit through the host the session
// ends before answering)
func TestAnUnansweredEmitIsReportedWhenTheSessionEnds(t *testing.T) {
	reports := captureReports(t)
	rt, rows := eventRows(t, "")
	end := refusingHost(t, rt, true)
	rt.mu.Lock()
	rt.elsewhere["t/e"] = true
	rt.mu.Unlock()
	if err := rows[0].Emit(context.Background(), "t/e"); err != nil {
		t.Fatal(err)
	}
	end()
	// The session's end as this process's session gives it.
	want := "rutis: emit of t/e from row r0 failed: BindingError: the host disconnected"
	testwait.Eventually(t, "the report", func() bool {
		got := reports.list()
		return len(got) == 1 && got[0] == want
	}, func() string { return fmt.Sprint(reports.list()) })
}

// guarantee: events/filter (a host descriptor may carry more fields, such
// as the emitter's own `listener` id: the selector ignores them)
func TestTheSelectorIgnoresExtraDescriptorFields(t *testing.T) {
	var seen []Listener
	sel := selector(keepOf(func(l Listener) (bool, error) { seen = append(seen, l); return l.Row == "chat", nil }))
	descriptor := `{"name":"t/n","prefix":false,"plugin":"chat-ui","row":"chat","runtime":"go","node":null,"scope":[{"id":"17","group":"tenant"}],"global":false,"listener":"7"}`
	for _, arg := range []any{
		json.RawMessage("[" + descriptor + `,{"name":"t/n","plugin":"other","row":null,"runtime":null,"node":null,"scope":[],"global":false}]`),
		[]any{json.RawMessage(descriptor)},
	} {
		seen = nil
		answers, err := sel(context.Background(), []any{arg})
		if err != nil {
			t.Fatal(err)
		}
		want := Listener{Name: "t/n", Plugin: "chat-ui", Row: "chat", Runtime: "go", Scope: []Scope{{ID: "17", Group: "tenant"}}}
		if len(seen) == 0 || !reflect.DeepEqual(seen[0], want) || answers.([]any)[0] != true {
			t.Fatalf("descriptor %+v, answers %v", seen, answers)
		}
	}
}

// guarantee: events/filter (a filter gets a copy of the scope: changing it
// changes neither the entry nor the row)
func TestAFilterGetsACopyOfTheScope(t *testing.T) {
	_, rows := eventRows(t, "t1")
	a := rows[0]
	ctx := context.Background()
	j := &journal{}
	a.On("t/scope", returns(j, "a", nil))
	var seen []string
	keep := func(l Listener) (bool, error) {
		seen = append(seen, l.Scope[0].ID)
		l.Scope[0].ID = "changed"
		return true, nil
	}
	for range 2 {
		if _, err := a.Filter(keep).Serial(ctx, "t/scope"); err != nil {
			t.Fatal(err)
		}
	}
	if fmt.Sprint(seen) != "[t1 t1]" || a.Scope()[0].ID != "t1" || fmt.Sprint(j.list()) != "[a a]" {
		t.Fatalf("seen %v, scope %v, ran %v", seen, a.Scope(), j.list())
	}
}

// guarantee: events/waterfall (the terminal gets the event's arguments,
// §5.4)
func TestTheTerminalGetsTheEventArguments(t *testing.T) {
	_, rows := eventRows(t, "")
	a := rows[0]
	ctx := context.Background()
	terminal := func(_ context.Context, args ...any) (any, error) { return fmt.Sprint(len(args), args), nil }
	a.On("t/wf-args", func(ctx context.Context, args ...any) (any, error) {
		rest, err := args[len(args)-1].(events.Next)(ctx)
		return fmt.Sprint("a:", rest), err
	})
	if value, err := a.Waterfall(ctx, "t/wf-args", terminal, "x", 2); err != nil || value != "a:2 [x 2]" {
		t.Fatalf("through a listener: %v %v", value, err)
	}
	if value, err := a.Waterfall(ctx, "t/none", terminal, "x"); err != nil || value != "1 [x]" {
		t.Fatalf("no listeners: %v %v", value, err)
	}
}

// guarantee: events/next (next runs the rest of the chain when called,
// before it returns)
func TestNextRunsTheRestWhenCalled(t *testing.T) {
	_, rows := eventRows(t, "")
	a := rows[0]
	ctx := context.Background()
	j := &journal{}
	a.On("t/eager", func(ctx context.Context, args ...any) (any, error) {
		j.add("a:before")
		rest, err := args[len(args)-1].(events.Next)(ctx)
		j.add("a:after")
		return rest, err
	})
	a.On("t/eager", returns(j, "b", "B"))
	if value, err := a.Waterfall(ctx, "t/eager", nil); err != nil || value != "B" {
		t.Fatalf("waterfall: %v %v", value, err)
	}
	if got := fmt.Sprint(j.list()); got != "[a:before b a:after]" {
		t.Fatalf("ran %s", got)
	}
}

// guarantee: events/unload (a listener its row's unload stops is not a
// failure: no report; emitting from an unloaded row is refused)
func TestAListenerStoppedByItsUnloadIsNotReported(t *testing.T) {
	rt, rows := eventRows(t, "", "")
	a, b := rows[0], rows[1]
	ctx := context.Background()
	reports := captureReports(t)
	started := make(chan struct{}, 2)
	b.On("t/stop", func(ctx context.Context, _ ...any) (any, error) {
		started <- struct{}{}
		<-ctx.Done()
		return nil, ctx.Err()
	})
	b.On("t/stop", func(ctx context.Context, _ ...any) (any, error) {
		started <- struct{}{}
		<-ctx.Done()
		return nil, fmt.Errorf("stopping: %w", ctx.Err())
	})
	if err := a.Emit(ctx, "t/stop"); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the first listener starts", started)
	testwait.Within(t, "the second listener starts", started)
	if err := rt.unload(ctx, "r1"); err != nil {
		t.Fatal(err)
	}
	if got := reports.list(); len(got) != 0 {
		t.Fatalf("stopped listeners were reported: %q", got)
	}
	err := b.Emit(ctx, "t/stop")
	if !errors.Is(err, ErrBinding) || peer.ErrorName(err) != "BindingError" || err.Error() != "row r1 is not loaded in this runtime" {
		t.Fatalf("an unloaded row emitted: %v", err)
	}
	if _, err := b.Filter(func(Listener) (bool, error) { return true, nil }).Serial(ctx, "t/stop"); err == nil || err.Error() != "row r1 is not loaded in this runtime" {
		t.Fatalf("an unloaded row's filtered emitter emitted: %v", err)
	}
}
