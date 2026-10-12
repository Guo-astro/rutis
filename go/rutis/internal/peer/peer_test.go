package peer

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	goruntime "runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// pair is a host session and a runtime session over a pipe.
func pair(t *testing.T, host, runtime func(ctx context.Context, target, method string, args []any) (any, error)) (*Peer, *Peer) {
	t.Helper()
	h, r, _ := recordedPair(t, host, runtime)
	return h, r
}

// recorder is a channel that keeps what it received, in order.
type recorder struct {
	Channel
	mu       sync.Mutex
	received []frame
}

func (r *recorder) Recv() ([]byte, error) {
	message, err := r.Channel.Recv()
	if err == nil {
		var f frame
		_ = json.Unmarshal(message, &f)
		r.mu.Lock()
		r.received = append(r.received, f)
		r.mu.Unlock()
	}
	return message, err
}

// frames is what the channel received so far.
func (r *recorder) frames() []frame {
	r.mu.Lock()
	defer r.mu.Unlock()
	return append([]frame(nil), r.received...)
}

// recordedPair is pair, with what the host session receives recorded.
// The session handles what it receives one frame at a time, in order: a
// frame recorded before another is handled before it.
func recordedPair(t *testing.T, host, runtime func(ctx context.Context, target, method string, args []any) (any, error)) (*Peer, *Peer, *recorder) {
	t.Helper()
	a, b := net.Pipe()
	hostChannel := &recorder{Channel: NewLines(a)}
	h, err := New(hostChannel, Options{Host: true, Dispatch: host})
	if err != nil {
		t.Fatal(err)
	}
	r, err := New(NewLines(b), Options{Dispatch: runtime})
	if err != nil {
		t.Fatal(err)
	}
	// A pipe holds nothing: each side's hello waits for the other to read.
	started := make(chan error, 2)
	for _, p := range []*Peer{h, r} {
		go func(p *Peer) { started <- p.Start() }(p)
	}
	for range 2 {
		if err := testwait.Within(t, "the session to start", started); err != nil {
			t.Fatal(err)
		}
	}
	for _, p := range []*Peer{h, r} {
		testwait.Within(t, "the handshake", p.Ready())
	}
	t.Cleanup(func() {
		h.Close(nil)
		r.Close(nil)
		a.Close()
		b.Close()
	})
	return h, r, hostChannel
}

func text(v any) string {
	var s string
	_ = json.Unmarshal(v.(json.RawMessage), &s)
	return s
}

func TestTheChainTravelsInTheContext(t *testing.T) {
	var host *Peer
	var seen []string
	var mu sync.Mutex
	host, runtime := pair(t,
		func(ctx context.Context, target, method string, args []any) (any, error) {
			mu.Lock()
			seen = ChainOf(ctx)
			mu.Unlock()
			return "ok", nil
		},
		func(ctx context.Context, target, method string, args []any) (any, error) {
			// Calls back with the chain it was given.
			return host2(ctx), nil
		})
	_ = runtime
	host2Peer = runtime
	if _, err := host.Call(context.Background(), "x", "go", nil, true); err != nil {
		t.Fatal(err)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(seen) != 2 || seen[0] != "rust:1" || !strings.HasPrefix(seen[1], "node:") {
		t.Fatalf("the callback carries the chain rust:1 -> node:N: %v", seen)
	}
}

var host2Peer *Peer

func host2(ctx context.Context) any {
	value, err := host2Peer.Call(ctx, "host:x", "back", nil, true)
	if err != nil {
		return err.Error()
	}
	return value
}

func TestCancellingAnAsyncCallCancelsTheMethod(t *testing.T) {
	cancelled := make(chan struct{})
	host, _, received := recordedPair(t, nil, func(ctx context.Context, target, method string, args []any) (any, error) {
		if method != "wait" {
			return "ok", nil
		}
		<-ctx.Done()
		close(cancelled)
		return nil, ctx.Err()
	})
	// The deadline is what cancels the call: the method only ends when
	// cancelled, so any duration tests the same thing.
	ctx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
	defer cancel()
	if _, err := host.Call(ctx, "x", "wait", nil, false); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("%v", err)
	}
	testwait.Within(t, "the method to be cancelled", cancelled)
	// The method still replies, late: the host receives the reply and drops
	// it. A call after that reply is handled after it (the session handles
	// what it receives in order): the session went on.
	replied := func() bool {
		for _, f := range received.frames() {
			if f.Op == "return" || f.Op == "throw" {
				return true
			}
		}
		return false
	}
	testwait.Eventually(t, "the late reply", replied, nil)
	if _, err := host.Call(context.Background(), "x", "after", nil, true); err != nil {
		t.Fatal(err)
	}
	if host.Err() != nil {
		t.Fatal(host.Err())
	}
}

func TestReferencesAreReleasedWithTheirCount(t *testing.T) {
	var held *RemoteFunction
	host, runtime := pair(t, nil, func(ctx context.Context, target, method string, args []any) (any, error) {
		switch method {
		case "hold":
			held = args[0].(*RemoteFunction)
			return Undefined, nil
		case "fire":
			return held.Call(ctx, nil, true)
		case "drop":
			runtime := held.peer
			runtime.Release(held)
			held = nil
			return Undefined, nil
		}
		return nil, errors.New("no method")
	})
	_ = runtime
	callback := Func(func(ctx context.Context, args []any) (any, error) { return "fired", nil })
	ctx := context.Background()
	for i := 0; i < 2; i++ {
		if _, err := host.Call(ctx, "x", "hold", []any{callback}, true); err != nil {
			t.Fatal(err)
		}
	}
	value, err := host.Call(ctx, "x", "fire", nil, true)
	if err != nil || text(value) != "fired" {
		t.Fatalf("%v %v", value, err)
	}
	if _, err := host.Call(ctx, "x", "drop", nil, true); err != nil {
		t.Fatal(err)
	}
	// The first proxy was dropped unreleased: collecting it releases it.
	left := func() int {
		host.mu.Lock()
		defer host.mu.Unlock()
		return len(host.exports)
	}
	testwait.Eventually(t, "the exports to be released", func() bool {
		goruntime.GC()
		return left() == 0
	}, func() string { return fmt.Sprintf("%d exports left", left()) })
}

func TestANotificationGoesOutBeforeTheReply(t *testing.T) {
	var order []string
	var mu sync.Mutex
	var runtimePeer *Peer
	host, runtime, received := recordedPair(t,
		func(ctx context.Context, target, method string, args []any) (any, error) {
			mu.Lock()
			order = append(order, method)
			mu.Unlock()
			return nil, nil
		},
		func(ctx context.Context, target, method string, args []any) (any, error) {
			runtimePeer.Notify("", "service", []any{"x"})
			return "done", nil
		})
	runtimePeer = runtime
	if _, err := host.Call(context.Background(), "x", "load", nil, true); err != nil {
		t.Fatal(err)
	}
	// The order on the wire: the greeting, the notification, the reply.
	var ops []string
	for _, f := range received.frames() {
		ops = append(ops, f.Op)
	}
	if strings.Join(ops, " ") != "hello invoke return" {
		t.Fatalf("received %v", ops)
	}
	// The notification runs on its own goroutine: wait for it.
	served := func() []string {
		mu.Lock()
		defer mu.Unlock()
		return append([]string(nil), order...)
	}
	testwait.Eventually(t, "the notification to be served", func() bool { return len(served()) > 0 }, nil)
	if got := served(); len(got) != 1 || got[0] != "service" {
		t.Fatalf("%v", got)
	}
}

func TestErrorsCrossWithTheirNames(t *testing.T) {
	host, _ := pair(t, nil, func(ctx context.Context, target, method string, args []any) (any, error) {
		switch method {
		case "plain":
			return nil, errors.New("plain")
		case "panic":
			return Protect(func() (any, error) { panic("boom") })
		}
		return nil, &RemoteError{Name: "SyncWaitCycle", Message: "cycle"}
	})
	ctx := context.Background()
	_, err := host.Call(ctx, "x", "plain", nil, true)
	var remote *RemoteError
	if !errors.As(err, &remote) || remote.Name != "Error" || remote.Message != "plain" {
		t.Fatalf("%v", err)
	}
	_, err = host.Call(ctx, "x", "panic", nil, true)
	if !errors.As(err, &remote) || remote.Name != "Panic" || !bytes.Contains(remote.Graph, []byte("boom")) {
		t.Fatalf("%v", err)
	}
	_, err = host.Call(ctx, "x", "cycle", nil, true)
	if !errors.Is(err, ErrSyncWaitCycle) {
		t.Fatalf("%v", err)
	}
}

type chunks struct{ data []byte }

func (c *chunks) Read(p []byte) (int, error) {
	if len(c.data) == 0 {
		return 0, io.EOF
	}
	n := copy(p, c.data)
	c.data = c.data[n:]
	return n, nil
}

func TestLinesRefuseTooLongMessages(t *testing.T) {
	a, b := net.Pipe()
	defer a.Close()
	defer b.Close()
	lines := NewLines(a)
	if err := lines.Send([]byte("a\nb")); err == nil {
		t.Fatal("a raw newline must not be sent")
	}
	if err := lines.Send(make([]byte, MaxMessage+1)); err == nil {
		t.Fatal("a message over the limit must not be sent")
	}
	go func() {
		big := bytes.Repeat([]byte("x"), MaxMessage+10)
		_, _ = b.Write(append(big, '\n'))
	}()
	if _, err := NewLines(a).Recv(); err == nil || !strings.Contains(err.Error(), "exceeds the limit") {
		t.Fatalf("%v", err)
	}
}

func TestACallBeforeTheHandshakeEndsTheSession(t *testing.T) {
	a, b := net.Pipe()
	defer a.Close()
	defer b.Close()
	runtime, _ := New(NewLines(a), Options{})
	go func() {
		reader := NewLines(b)
		_, _ = reader.Recv() // its hello
		_ = reader.Send([]byte(`{"op":"invoke","id":"rust:1","path":[],"target":"x","method":"m","args":{"type":"list","value":[]}}`))
	}()
	_ = runtime.Start()
	testwait.Within(t, "the session to end", runtime.Closed())
	if !strings.Contains(runtime.Err().Error(), "before protocol handshake") {
		t.Fatal(runtime.Err())
	}
}

func TestConcurrentCallsGoOutInTheOrderOfTheirIdentities(t *testing.T) {
	host, runtime := pair(t, nil, func(ctx context.Context, target, method string, args []any) (any, error) {
		return "ok", nil
	})
	// The far end refuses an identity not above the last it received
	// (#238): many goroutines calling at once must not reorder theirs.
	var calls sync.WaitGroup
	failures := make(chan error, 64*20)
	for range 64 {
		calls.Add(1)
		go func() {
			defer calls.Done()
			for range 20 {
				if _, err := host.Call(context.Background(), "x", "go", nil, true); err != nil {
					failures <- err
					return
				}
			}
		}()
	}
	calls.Wait()
	close(failures)
	for err := range failures {
		t.Fatal(err)
	}
	if err := runtime.Err(); err != nil {
		t.Fatal(err)
	}
}

// A reference's origin, in the compat format, holds this session's ids and
// other sessions' tagged with them, whatever their format: refusing those
// ended the session of a nested call (#225).
func TestCompatOriginsTakeOtherSessionsTaggedIds(t *testing.T) {
	for _, id := range []string{"node:1", "rust:12", "s1/rust:7", "s1/node:2", "s12/mac-2:3"} {
		if !compatOrigin.MatchString(id) {
			t.Errorf("%s: refused", id)
		}
	}
	for _, id := range []string{"py:1", "node:0", "s1/s2/node:1", "s1/node", "S1/node:1", "node:01"} {
		if compatOrigin.MatchString(id) {
			t.Errorf("%s: taken", id)
		}
	}
}
