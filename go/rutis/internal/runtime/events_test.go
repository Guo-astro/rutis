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
// root), with no session: what the host does, a test does by calling the
// runtime (`invoke`), as the host would.
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

// invoke runs listener `id` as the host does (`events.call`), in `mode`,
// with `args`; it returns once the invocation ran.
func invoke(ctx context.Context, rt *runtimeState, id, name, mode string, args ...any) (any, error) {
	if args == nil {
		args = []any{}
	}
	return rt.control(ctx, "events.call", []any{map[string]any{"listener": id, "dispatch": "d", "own": false, "name": name, "mode": mode, "args": args}})
}

// guarantee: sdk/leaf-events (disposers, unload, reserved names)
func TestReservedNamesAndInactiveRows(t *testing.T) {
	rt, rows := eventRows(t, "", "")
	a, b := rows[0], rows[1]
	ctx := context.Background()
	listen := func(context.Context, ...any) (any, error) { return nil, nil }
	if _, err := peer.Protect(func() (any, error) { a.On("internal/update", listen); return nil, nil }); err == nil {
		t.Fatal("reserved registration accepted")
	}
	if err := rt.unload(ctx, "r1"); err != nil {
		t.Fatal(err)
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
	if err := b.Emit(ctx, "changed"); !errors.Is(err, ErrBinding) || err.Error() != "row r1 is not loaded in this runtime" {
		t.Fatalf("an unloaded row emitted: %v", err)
	}
}

// Every emission goes through the host: without a session (it ended)
// it fails, saying so, in every mode.
func TestAnEmitWithoutTheHostSaysItDisconnected(t *testing.T) {
	_, rows := eventRows(t, "")
	a := rows[0]
	ctx := context.Background()
	for _, emit := range []func() error{
		func() error { return a.Emit(ctx, "t/e") },
		func() error { return a.Parallel(ctx, "t/e") },
		func() error { _, err := a.Serial(ctx, "t/e"); return err },
		func() error { _, err := a.Waterfall(ctx, "t/e", nil); return err },
		func() error { _, err := a.Bail(ctx, "t/e"); return err },
	} {
		if err := emit(); !errors.Is(err, ErrBinding) || err.Error() != "the host disconnected" {
			t.Fatalf("an emit without the host: %v", err)
		}
	}
}

// guarantee: events/emit (order per listener; failures reported where
// the listener is, isolated)
func TestEmitInvocationsRunInOrderAndFailuresAreReported(t *testing.T) {
	reports := captureReports(t)
	rt, rows := eventRows(t, "")
	a := rows[0]
	j := &journal{}
	release := make(chan struct{})
	a.On("t/emit", func(ctx context.Context, args ...any) (any, error) {
		<-release
		j.add(fmt.Sprintf("slow:%v", args[0]))
		return nil, nil
	})
	a.On("t/emit", func(_ context.Context, args ...any) (any, error) {
		j.add(fmt.Sprintf("fails:%v", args[0]))
		return nil, peer.Errorf("Boom", "failed on %v", args[0])
	})
	// The host's calls arrive in frame order (arrive, on the reading
	// goroutine); each is answered once it ran.
	var answered sync.WaitGroup
	for i := 1; i <= 3; i++ {
		for _, id := range []string{"1", "2"} {
			run := rt.arrive(context.Background(), "events.call", []any{map[string]any{"listener": id, "dispatch": "d", "own": false, "name": "t/emit", "mode": "emit", "args": []any{i}}})
			answered.Add(1)
			go func() { defer answered.Done(); _, _ = run() }()
		}
	}
	testwait.Eventually(t, "the fast listener ran each emit", func() bool { return len(j.list()) == 3 }, func() string { return fmt.Sprint(j.list()) })
	close(release)
	answered.Wait()
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

// guarantee: sdk/leaf-events (cancels and waits, bounded by the deadline)
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
	go func() { _, err := invoke(ctx, rt, "1", "t/unload", "parallel"); done <- err }()
	testwait.Within(t, "the listener starts", started)
	if err := rt.unload(ctx, "r1"); err != nil {
		t.Fatal(err)
	}
	if err := testwait.Within(t, "the invocation settles", done); !errors.Is(err, context.Canceled) {
		t.Fatalf("the cancelled listener's failure: %v", err)
	}

	// One that does not stop is given up after the deadline.
	t.Setenv("RUTIS_SHUTDOWN_TIMEOUT", "0.05")
	reports := captureReports(t)
	stuck := make(chan struct{})
	defer close(stuck)
	a.On("t/stuck", func(context.Context, ...any) (any, error) { started <- struct{}{}; <-stuck; return nil, nil })
	go func() { _, _ = invoke(ctx, rt, "2", "t/stuck", "emit") }()
	testwait.Within(t, "the stuck listener starts", started)
	if err := rt.unload(ctx, "r0"); err != nil {
		t.Fatal(err)
	}
	if got := reports.list(); len(got) != 1 || got[0] != "rutis: listeners of row r0 still running after 0.05s; unloading anyway" {
		t.Fatalf("reported %q", got)
	}
}

// guarantee: sdk/leaf-events (an emitter giving up cancels its listeners:
// the host's call is cancelled)
func TestACancelledCallCancelsItsListener(t *testing.T) {
	rt, rows := eventRows(t, "")
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
	go func() { _, _ = invoke(ctx, rt, "1", "t/cancel", "parallel") }()
	testwait.Within(t, "the listener starts", started)
	cancel()
	testwait.Within(t, "the listener is cancelled", cancelled)
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
	rt := newRuntime([]*Definition{def})
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
	rt.mu.Lock()
	var ids []string
	for _, l := range rt.listeners["changed"] {
		ids = append(ids, l.id)
	}
	rt.mu.Unlock()
	for _, id := range ids {
		if _, err := invoke(ctx, rt, id, "changed", "parallel"); err != nil {
			t.Fatal(err)
		}
	}
	if len(seen) != 1 || seen[0] != "updated" {
		t.Fatalf("listeners of the old config or the failed row remain: %v", seen)
	}
}

// guarantee: sdk/leaf-events (a call for a listener no longer there)
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

// refusingHost attaches to `rt` a session whose host refuses every
// `events.on` and `events.dispatch` (answering the rest with undefined),
// or, when `silent`, never answers an `events.dispatch`.
func refusingHost(t *testing.T, rt *runtimeState, silent bool) (end func()) {
	t.Helper()
	a, b := net.Pipe()
	hold := make(chan struct{})
	host, err := peer.New(peer.NewLines(a), peer.Options{Endpoint: &peer.Endpoint{Local: "main"}, Capabilities: peer.TestCapabilities, Dispatch: func(ctx context.Context, target, method string, args []any) (any, error) {
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
	runtime, err := peer.New(peer.NewLines(b), peer.Options{Endpoint: &peer.Endpoint{Local: "go"}, Capabilities: peer.TestCapabilities, Dispatch: rt.dispatch, Arrive: rt.arrive})
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

// guarantee: sdk/leaf-events (§11: an emit through the host refused, a
// registration refused after its row loaded)
func TestRefusedEmitsAndRegistrationsAreReported(t *testing.T) {
	reports := captureReports(t)
	rt, rows := eventRows(t, "")
	a := rows[0]
	refusingHost(t, rt, false)
	a.On("t/late", func(context.Context, ...any) (any, error) { return nil, nil })
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

// guarantee: sdk/leaf-events (§11: a registration the host refuses
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

// guarantee: sdk/leaf-events (§11: an emit through the host the session
// ends before answering)
func TestAnUnansweredEmitIsReportedWhenTheSessionEnds(t *testing.T) {
	reports := captureReports(t)
	rt, rows := eventRows(t, "")
	end := refusingHost(t, rt, true)
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

// guarantee: sdk/leaf-events (a listener its row's unload stops is not a
// failure: no report; emitting from an unloaded row is refused)
func TestAListenerStoppedByItsUnloadIsNotReported(t *testing.T) {
	rt, rows := eventRows(t, "", "")
	b := rows[1]
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
	for _, id := range []string{"1", "2"} {
		go func() { _, _ = invoke(ctx, rt, id, "t/stop", "emit") }()
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
