package peer

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// objectHost is rawHost with its own dispatch: a session whose far
// end the test writes and reads raw frames of.
func objectHost(t *testing.T, dispatch func(ctx context.Context, target, method string, args []any) (any, error)) (*Peer, Channel, func() map[string]any) {
	t.Helper()
	x, y := net.Pipe()
	runtime, err := New(NewLines(x), Options{Endpoint: &Endpoint{Local: "go"}, Capabilities: TestCapabilities, Dispatch: dispatch})
	if err != nil {
		t.Fatal(err)
	}
	far := NewLines(y)
	frames := make(chan map[string]any, 64)
	go func() {
		for {
			message, err := far.Recv()
			if err != nil {
				close(frames)
				return
			}
			var f map[string]any
			_ = json.Unmarshal(message, &f)
			frames <- f
		}
	}()
	go func() { _ = runtime.Start() }()
	if err := far.Send([]byte(`{"op":"hello","version":4,"endpoint":"main"}`)); err != nil {
		t.Fatal(err)
	}
	next := func() map[string]any {
		t.Helper()
		f := testwait.Within(t, "a frame", frames)
		if f == nil {
			t.Fatalf("the session ended: %v", runtime.Err())
		}
		return f
	}
	if hello := next(); hello["op"] != "hello" {
		t.Fatalf("%v", hello)
	}
	testwait.Within(t, "the handshake", runtime.Ready())
	t.Cleanup(func() { runtime.Close(nil) })
	return runtime, far, next
}

// slow is an object whose `wait` runs until its call is cancelled.
type slow struct{ cancelled chan struct{} }

func (s *slow) Call(ctx context.Context, method string, args []any) (any, error) {
	<-ctx.Done()
	close(s.cancelled)
	return nil, ctx.Err()
}
func (s *slow) Get(ctx context.Context, property string) (any, error) { return nil, nil }

// objects-case(go): cancel
// objects-case(go): holder-killed-releases
// An object of this side: a call of its method the caller gives up is
// cancelled here; once the host releases it (its holder let go or went),
// it is let go of.
func TestAnObjectHereIsCancelledThroughAndReleased(t *testing.T) {
	object := &slow{cancelled: make(chan struct{})}
	runtime, far, next := objectHost(t, func(ctx context.Context, target, method string, args []any) (any, error) {
		if method == "ping" {
			return "pong", nil
		}
		return object, nil
	})
	_ = far.Send([]byte(`{"op":"invoke","id":"main:1","path":[],"target":"x","method":"make","args":{"type":"list","value":[]}}`))
	reply := next()
	ref := fmt.Sprint(int64(reply["value"].(map[string]any)["value"].(map[string]any)["id"].(float64)))
	_ = far.Send([]byte(`{"op":"call","id":"main:2","path":[],"reference":` + ref + `,"method":"wait","args":{"type":"list","value":[]}}`))
	_ = far.Send([]byte(`{"op":"cancel","id":"main:2"}`))
	testwait.Within(t, "the method sees the cancel", object.cancelled)
	if f := next(); f["id"] != "main:2" {
		t.Fatalf("%v", f)
	}
	runtime.mu.Lock()
	before := len(runtime.exports)
	runtime.mu.Unlock()
	_ = far.Send([]byte(`{"op":"release","reference":` + ref + `,"count":1}`))
	// Frames are taken in order: once ping is answered, the release was.
	_ = far.Send([]byte(`{"op":"invoke","id":"main:3","path":[],"target":"x","method":"ping","args":{"type":"list","value":[]}}`))
	if f := next(); f["id"] != "main:3" {
		t.Fatalf("%v", f)
	}
	runtime.mu.Lock()
	after := len(runtime.exports)
	runtime.mu.Unlock()
	if after != before-1 {
		t.Fatalf("exports %d, then %d", before, after)
	}
	open(t, runtime)
}

// objects-case(go): receiver-releases
// objects-case(go): cancel
// objects-case(go): undefined-nested-null
// An object of the far end: a call this side gives up is cancelled there;
// let go of, it is released. Undefined inside a value is null; a whole
// argument keeps it.
func TestAnObjectOfTheFarEndIsCancelledAndReleased(t *testing.T) {
	got := make(chan []any, 1)
	runtime, far, next := objectHost(t, func(ctx context.Context, target, method string, args []any) (any, error) {
		got <- args
		return args[0], nil
	})
	object := `{"type":"reference","value":{"id":7,"kind":"object","home":false,"origin":[]}}`
	_ = far.Send([]byte(`{"op":"invoke","id":"main:1","path":[],"target":"x","method":"take","args":{"type":"list","value":[` + object + `,{"type":"undefined"},{"type":"list","value":[{"type":"data","value":1},{"type":"undefined"}]}]}}`))
	args := testwait.Within(t, "the arguments", got)
	next()
	remote := args[0].(*RemoteObject)
	if _, undefined := args[1].(UndefinedType); !undefined {
		t.Fatalf("a whole argument: %#v", args[1])
	}
	if inner := args[2].([]any); string(inner[1].(json.RawMessage)) != "null" {
		t.Fatalf("inside a value: %#v", inner[1])
	}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() {
		_, err := remote.Call(ctx, "slow", nil, false)
		done <- err
	}()
	call := next()
	if call["op"] != "call" || call["method"] != "slow" {
		t.Fatalf("%v", call)
	}
	cancel()
	if f := next(); f["op"] != "cancel" || f["id"] != call["id"] {
		t.Fatalf("%v", f)
	}
	testwait.Within(t, "the call gives up", done)
	runtime.Release(remote)
	for f := next(); f["op"] != "release"; f = next() {
	}
	open(t, runtime)
}
