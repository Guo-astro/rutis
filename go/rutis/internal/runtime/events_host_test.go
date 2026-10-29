package rutis

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// gated is the runtime's end of a session whose writes a test can hold:
// a far end that does not read (backpressure).
type gated struct {
	peer.Channel
	mu sync.Mutex
	// While armed, the first frame containing hold blocks until the
	// reader took the frame after one containing after.
	hold, after string
	armed, held bool
	took        bool
	blocked     chan struct{}
	processed   chan struct{}
	stuck       atomic.Bool
}

func (g *gated) arm(hold, after string) {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.hold, g.after, g.armed = hold, after, true
	g.blocked, g.processed = make(chan struct{}), make(chan struct{})
}

func (g *gated) Send(message []byte) error {
	g.mu.Lock()
	hold := g.armed && !g.held && bytes.Contains(message, []byte(g.hold))
	if hold {
		g.held = true
	}
	blocked, processed := g.blocked, g.processed
	g.mu.Unlock()
	if hold {
		close(blocked)
		select {
		case <-processed:
		case <-time.After(3 * time.Second):
			// The reader never came back for the next frame.
			g.stuck.Store(true)
		}
	}
	return g.Channel.Send(message)
}

func (g *gated) Recv() ([]byte, error) {
	g.mu.Lock()
	if g.took {
		// The frame before was handled: the reader asks for the next.
		g.took = false
		close(g.processed)
	}
	g.mu.Unlock()
	message, err := g.Channel.Recv()
	g.mu.Lock()
	if g.armed && g.held && g.after != "" && bytes.Contains(message, []byte(g.after)) {
		g.took, g.after = true, ""
	}
	g.mu.Unlock()
	return message, err
}

// fakeHost is a host session driving one runtime: it answers events.on
// (refusing the names `refuse` says), and logs the event frames it gets.
type fakeHost struct {
	t       *testing.T
	rt      *runtimeState
	host    *peer.Peer
	channel *gated
	refuse  func(name string) error
	// Ends the session, then the runtime's rows.
	end    func()
	mu     sync.Mutex
	frames []string
	// The handle of each service the runtime published, by id.
	handles map[string]string
}

// service calls `method` of the service `name` the runtime published.
func (h *fakeHost) service(name, method string, args ...any) error {
	h.t.Helper()
	var handle string
	testwait.Eventually(h.t, "the service "+name, func() bool {
		h.mu.Lock()
		defer h.mu.Unlock()
		handle = h.handles[name]
		return handle != ""
	}, nil)
	ctx, cancel := context.WithTimeout(context.Background(), testwait.HangGuard)
	defer cancel()
	if args == nil {
		args = []any{}
	}
	_, err := h.host.Call(ctx, handle, method, args, false)
	return err
}

func (h *fakeHost) log() []string {
	h.mu.Lock()
	defer h.mu.Unlock()
	return append([]string(nil), h.frames...)
}

func startFakeHost(t *testing.T, refuse func(string) error, defs ...*Definition) *fakeHost {
	t.Helper()
	return startFakeHostWith(t, nil, refuse, defs...)
}

// startFakeHostWith is startFakeHost whose host writes and reads through
// wrap(its channel), when wrap is not nil.
func startFakeHostWith(t *testing.T, wrap func(peer.Channel) peer.Channel, refuse func(string) error, defs ...*Definition) *fakeHost {
	t.Helper()
	runtimeSide, hostSide := net.Pipe()
	h := &fakeHost{t: t, refuse: refuse, channel: &gated{Channel: peer.NewLines(runtimeSide)}}
	// Each side's hello waits for the other to read (net.Pipe).
	started := make(chan *session, 1)
	go func() {
		s, err := startSession(h.channel, runnerEndpoint("go", "main"), defs)
		if err != nil {
			t.Error(err)
		}
		started <- s
	}()
	var err error
	var hostChannel peer.Channel = peer.NewLines(hostSide)
	if wrap != nil {
		hostChannel = wrap(hostChannel)
	}
	h.host, err = peer.New(hostChannel, peer.Options{
		Endpoint:     &peer.Endpoint{Local: "main"},
		Capabilities: peer.TestCapabilities,
		Dispatch: func(_ context.Context, target, method string, args []any) (any, error) {
			if target == "" && method == "service" && len(args) > 1 {
				var id, handle string
				_ = convert.Decode(args[0], &id)
				_ = convert.Decode(args[1], &handle)
				h.mu.Lock()
				if h.handles == nil {
					h.handles = map[string]string{}
				}
				h.handles[id] = handle
				h.mu.Unlock()
			}
			if target == "" && method == "events.dispatch" {
				h.mu.Lock()
				h.frames = append(h.frames, method)
				h.mu.Unlock()
			}
			return nil, nil
		},
		Arrive: func(_ context.Context, method string, args []any) func() (any, error) {
			if method != "events.on" && method != "events.off" {
				return nil
			}
			h.mu.Lock()
			h.frames = append(h.frames, method)
			h.mu.Unlock()
			var registration struct{ Name string }
			if method == "events.on" && len(args) > 0 {
				_ = convert.Decode(args[0], &registration)
			}
			var err error
			if method == "events.on" && h.refuse != nil {
				err = h.refuse(registration.Name)
			}
			return func() (any, error) { return nil, err }
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := h.host.Start(); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the handshake", h.host.Ready())
	s := testwait.Within(t, "the runtime starts", started)
	if s == nil {
		t.FailNow()
	}
	h.rt = s.runtime
	var once sync.Once
	h.end = func() {
		once.Do(func() {
			h.host.Close(nil)
			hostSide.Close()
			<-s.peer.Closed()
			_ = s.end(nil)
		})
	}
	t.Cleanup(h.end)
	h.call("mount", map[string]any{"announce": true, "runtime": "go"})
	return h
}

func (h *fakeHost) call(method string, args ...any) error {
	h.t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), testwait.HangGuard)
	defer cancel()
	_, err := h.host.Call(ctx, "", method, args, false)
	return err
}

func (h *fakeHost) load(key, entry string) error {
	return h.call("rows.load", key, entry, json.RawMessage("null"), []any{}, []any{}, map[string]any{}, map[string]any{"scope": []any{}, "id": key})
}

// guarantee: events/registration (a refused events.on fails the row's
// load, reported only outside a load: §4.4, §11)
func TestARefusedRegistrationFailsTheLoad(t *testing.T) {
	reports := captureReports(t)
	var cleaned atomic.Bool
	var late *Ctx
	refused := Define(Plugin[NoConfig]{Name: "refused", Apply: func(c *Ctx, _ NoConfig) error {
		c.Effect(func(context.Context) error { cleaned.Store(true); return nil })
		c.On("t/fine", func(context.Context, ...any) (any, error) { return nil, nil })
		c.On("t/refused", func(context.Context, ...any) (any, error) { return nil, nil })
		return nil
	}})
	later := Define(Plugin[NoConfig]{Name: "later", Apply: func(c *Ctx, _ NoConfig) error { late = c; return nil }})
	h := startFakeHost(t, func(name string) error {
		if strings.HasPrefix(name, "t/refused") {
			return peer.Errorf("BindingError", "event %s is refused", name)
		}
		return nil
	}, refused, later)
	err := h.load("r", "refused")
	var remote *peer.RemoteError
	if !errors.As(err, &remote) || remote.Name != "BindingError" || remote.Message != "event t/refused is refused" {
		t.Fatalf("the load did not fail with the refusal: %#v", err)
	}
	h.rt.mu.Lock()
	_, loaded := h.rt.rows["r"]
	listeners := len(h.rt.byID)
	h.rt.mu.Unlock()
	if loaded || listeners != 0 || !cleaned.Load() {
		t.Fatalf("the row was not unloaded: loaded %v, listeners %d, cleaned %v", loaded, listeners, cleaned.Load())
	}
	if got := reports.list(); len(got) != 0 {
		t.Fatalf("a refusal during a load is reported: %q", got)
	}
	// Outside a load: reported, the row stays.
	if err := h.load("l", "later"); err != nil {
		t.Fatal(err)
	}
	late.On("t/refused-late", func(context.Context, ...any) (any, error) { return nil, nil })
	want := "rutis: registration of listener of t/refused-late in row l failed: BindingError: event t/refused-late is refused"
	testwait.Eventually(t, "the refusal is reported", func() bool { return slices.Contains(reports.list(), want) }, func() string { return fmt.Sprint(reports.list()) })
	h.rt.mu.Lock()
	_, loaded = h.rt.rows["l"]
	h.rt.mu.Unlock()
	if !loaded {
		t.Fatal("a refusal outside a load unloaded the row")
	}
}

// guarantee: events/registration (no session write holds rt.mu: a write
// blocked by backpressure does not stop the reader, which takes rt.mu for
// the frames it handles inline)
func TestABlockedWriteDoesNotStopTheReader(t *testing.T) {
	var c *Ctx
	def := Define(Plugin[NoConfig]{Name: "writer", Apply: func(ctx *Ctx, _ NoConfig) error { c = ctx; return nil }})
	h := startFakeHost(t, nil, def)
	if err := h.load("w", "writer"); err != nil {
		t.Fatal(err)
	}
	h.channel.arrange()
	registered := make(chan struct{})
	go func() {
		c.On("t/blocked", func(context.Context, ...any) (any, error) { return nil, nil })
		close(registered)
	}()
	testwait.Within(t, "the registration's write blocks", h.channel.blocked)
	// A frame the reader handles inline (taking rt.mu) while the write is
	// blocked; its answer waits for the write.
	changed := make(chan error, 1)
	go func() {
		changed <- h.call("events.call", map[string]any{"listener": "none", "name": "t/x", "mode": "serial", "args": []any{}})
	}()
	testwait.Within(t, "the registration is sent", registered)
	if h.channel.stuck.Load() {
		t.Fatal("the reader waited for rt.mu while a write held it")
	}
	if err := testwait.Within(t, "the call is answered", changed); err == nil || !strings.Contains(err.Error(), "withdrawn") {
		t.Fatalf("a call for no listener: %v", err)
	}
}

// arrange holds the next events.on until the reader took the
// events.call after it.
func (g *gated) arrange() { g.arm(`"events.on"`, `"events.call"`) }

// guarantee: events/order (an events.on precedes a later events.dispatch
// of the same process, §4.4 1)
func TestARegistrationPrecedesALaterDispatch(t *testing.T) {
	var c *Ctx
	def := Define(Plugin[NoConfig]{Name: "ordered", Apply: func(ctx *Ctx, _ NoConfig) error { c = ctx; return nil }})
	h := startFakeHost(t, nil, def)
	if err := h.load("o", "ordered"); err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	for range 20 {
		c.On("t/order", func(context.Context, ...any) (any, error) { return nil, nil })
		if _, err := c.Serial(ctx, "t/order"); err != nil {
			t.Fatal(err)
		}
	}
	log := h.log()
	for i := 0; i+1 < len(log); i += 2 {
		if log[i] != "events.on" || log[i+1] != "events.dispatch" {
			t.Fatalf("frames out of order: %v", log)
		}
	}
	if len(log) != 40 {
		t.Fatalf("frames %v", log)
	}
}
