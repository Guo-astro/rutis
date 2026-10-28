package peer

import (
	"context"
	"encoding/json"
	"net"
	"sync"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// farEnd is a scripted host (`main`): it greets, then hands
// each frame the runtime sends to `frames`.
func farEnd(t *testing.T, dispatch func(ctx context.Context, target, method string, args []any) (any, error)) (*Peer, *Lines, chan frame) {
	t.Helper()
	a, b := net.Pipe()
	runtime, err := New(NewLines(a), Options{Endpoint: &Endpoint{Local: "go"}, Capabilities: TestCapabilities, Dispatch: dispatch})
	if err != nil {
		t.Fatal(err)
	}
	far := NewLines(b)
	frames := make(chan frame, 16)
	go func() {
		if _, err := far.Recv(); err != nil { // its hello
			return
		}
		_ = far.Send([]byte(`{"op":"hello","version":4,"endpoint":"main","capabilities":["events-v2"]}`))
		for {
			message, err := far.Recv()
			if err != nil {
				return
			}
			var f frame
			_ = json.Unmarshal(message, &f)
			frames <- f
		}
	}()
	if err := runtime.Start(); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the handshake", runtime.Ready())
	t.Cleanup(func() {
		runtime.Close(nil)
		a.Close()
		b.Close()
	})
	return runtime, far, frames
}

// Events protocol §5.5: SendAnswered tells an emit's reply as its frame is
// read, before the next frame is handled.
func TestSendAnsweredTellsTheReplyBeforeTheNextFrame(t *testing.T) {
	var mu sync.Mutex
	answered := false
	seen := make(chan bool, 1)
	runtime, far, frames := farEnd(t, func(ctx context.Context, target, method string, args []any) (any, error) {
		mu.Lock()
		seen <- answered
		mu.Unlock()
		return nil, nil
	})
	told := make(chan error, 1)
	err := runtime.SendAnswered(context.Background(), "", "events.dispatch", []any{map[string]any{"mode": "emit"}}, func(err error) {
		mu.Lock()
		answered = true
		mu.Unlock()
		told <- err
	})
	if err != nil {
		t.Fatal(err)
	}
	sent := testwait.Within(t, "the dispatch", frames)
	// The reply, then at once a frame the runtime handles after it.
	_ = far.Send([]byte(`{"op":"return","id":` + string(sent.ID) + `,"value":{"type":"undefined"}}`))
	_ = far.Send([]byte(`{"op":"invoke","id":"main:1","path":[],"target":"","method":"after","args":{"type":"list","value":[]}}`))
	if !testwait.Within(t, "the next frame's call", seen) {
		t.Fatal("the frame after the reply was handled before the reply was told")
	}
	if err := testwait.Within(t, "the answer", told); err != nil {
		t.Fatal(err)
	}
}

// A 0.13 host answers a queued emit with a future: SendAnswered awaits
// it, and tells its settlement.
func TestSendAnsweredSettlesAFutureAnswer(t *testing.T) {
	runtime, far, frames := farEnd(t, nil)
	told := make(chan error, 1)
	if err := runtime.SendAnswered(context.Background(), "", "events.dispatch", []any{map[string]any{"mode": "emit"}}, func(err error) { told <- err }); err != nil {
		t.Fatal(err)
	}
	sent := testwait.Within(t, "the dispatch", frames)
	_ = far.Send([]byte(`{"op":"return","id":` + string(sent.ID) + `,"value":{"type":"reference","value":{"id":1,"kind":"future","home":false,"origin":[]}}}`))
	await := testwait.Within(t, "the await of the future", frames)
	if await.Op != "await" || string(await.Reference) != "1" {
		t.Fatalf("expected an await of reference 1, got %s %s", await.Op, await.Reference)
	}
	select {
	case err := <-told:
		t.Fatalf("told before the future settled: %v", err)
	default:
	}
	_ = far.Send([]byte(`{"op":"return","id":` + string(await.ID) + `,"value":{"type":"undefined"}}`))
	if err := testwait.Within(t, "the answer", told); err != nil {
		t.Fatal(err)
	}

	// A future that fails is told as that failure.
	if err := runtime.SendAnswered(context.Background(), "", "events.dispatch", []any{map[string]any{"mode": "emit"}}, func(err error) { told <- err }); err != nil {
		t.Fatal(err)
	}
	sent = testwait.Within(t, "the second dispatch", frames)
	_ = far.Send([]byte(`{"op":"return","id":` + string(sent.ID) + `,"value":{"type":"reference","value":{"id":2,"kind":"future","home":false,"origin":[]}}}`))
	await = testwait.Within(t, "the second await", frames)
	_ = far.Send([]byte(`{"op":"throw","id":` + string(await.ID) + `,"error":{"name":"BindingError","message":"refused"}}`))
	if err := testwait.Within(t, "the failure", told); err == nil {
		t.Fatal("a failed future was told as answered")
	}
}
