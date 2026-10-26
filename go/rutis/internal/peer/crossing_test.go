package peer

import (
	"context"
	"encoding/json"
	"errors"
	"strings"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// What a call cannot send (#675): only that call fails, saying which call,
// which argument (or the result) and why; the session goes on.

const overLimitTail = "over the limit of 16777216 bytes (16 MiB)"

func open(t *testing.T, p *Peer) {
	t.Helper()
	select {
	case <-p.Closed():
		t.Fatalf("the session ended: %v", p.Err())
	default:
	}
}

func TestAnArgumentOverTheLimitFailsOnlyItsCall(t *testing.T) {
	host, runtime, received := recordedPair(t, func(ctx context.Context, target, method string, args []any) (any, error) {
		return "taken", nil
	}, nil)
	ctx := context.Background()
	callback := Func(func(ctx context.Context, args []any) (any, error) { return nil, nil })
	_, err := runtime.Call(ctx, "host:svc\x00scope", "take", []any{callback, strings.Repeat("x", MaxMessage)}, true)
	var refused *ValueError
	if err == nil || !errors.As(err, &refused) || !strings.HasPrefix(err.Error(), "argument 2 of svc.take: the message would be ") || !strings.HasSuffix(err.Error(), overLimitTail) {
		t.Fatalf("%T %v", err, err)
	}
	runtime.mu.Lock()
	exports := len(runtime.exports)
	runtime.mu.Unlock()
	if exports != 0 {
		t.Errorf("the callback is left granted: %d exports", exports)
	}
	// Nothing reached the host: the next call is the first it receives.
	if value, err := runtime.Call(ctx, "host:svc", "after", nil, true); err != nil || text(value) != "taken" {
		t.Fatalf("%v %v", value, err)
	}
	for _, f := range received.frames() {
		var method string
		_ = json.Unmarshal(f.Method, &method)
		if f.Op == "invoke" && method == "take" {
			t.Fatal("the call over the limit was sent")
		}
	}
	open(t, runtime)
	open(t, host)
}

func TestAResultOverTheLimitIsAnsweredWithAThrow(t *testing.T) {
	host, runtime := pair(t, nil, func(ctx context.Context, target, method string, args []any) (any, error) {
		switch method {
		case "big":
			return strings.Repeat("x", MaxMessage), nil
		case "bigError":
			return nil, errors.New(strings.Repeat("y", MaxMessage))
		}
		return "small", nil
	})
	ctx := context.Background()
	_, err := host.Call(ctx, "conformance", "big", nil, true)
	var remote *RemoteError
	if !errors.As(err, &remote) || !strings.HasPrefix(remote.Message, "the result of conformance.big: the message would be ") || !strings.HasSuffix(remote.Message, overLimitTail) {
		t.Fatalf("%v", err)
	}
	_, err = host.Call(ctx, "conformance", "bigError", nil, true)
	if !errors.As(err, &remote) || !strings.HasPrefix(remote.Message, "the error Error of conformance.bigError: the message would be ") {
		t.Fatalf("%v", err)
	}
	if value, err := host.Call(ctx, "conformance", "small", nil, true); err != nil || text(value) != "small" {
		t.Fatalf("%v %v", value, err)
	}
	open(t, runtime)
	open(t, host)
}

const receivedObject = "received an object with methods, which Go cannot take; pass plain data or a function instead"

func TestAnObjectReferenceFromTheFarEndFailsOnlyItsCall(t *testing.T) {
	runtime, far, next := rawHost(t, `[]`)
	object := `{"type":"reference","value":{"id":7,"kind":"object","home":false,"origin":[]}}`
	// In a call's arguments: the call is answered with a throw.
	_ = far.Send([]byte(`{"op":"invoke","id":"rust:1","path":[],"target":"x","method":"echo","args":{"type":"list","value":[` + object + `]}}`))
	// Its grant goes back, then the call is answered.
	if f := next(); f["op"] != "release" || f["reference"] != float64(7) || f["count"] != float64(1) {
		t.Fatalf("%v", f)
	}
	f := next()
	if f["op"] != "throw" || f["error"].(map[string]any)["message"] != receivedObject {
		t.Fatalf("%v", f)
	}
	// In a reply: the call waiting for it fails.
	done := make(chan error, 1)
	go func() {
		_, err := runtime.Call(context.Background(), "host:x", "m", nil, true)
		done <- err
	}()
	call := next()
	_ = far.Send([]byte(`{"op":"return","id":"` + call["id"].(string) + `","value":` + object + `}`))
	if f := next(); f["op"] != "release" || f["reference"] != float64(7) {
		t.Fatalf("%v", f)
	}
	err := testwait.Within(t, "the reply", done)
	var refused *ValueError
	if !errors.As(err, &refused) || err.Error() != receivedObject {
		t.Fatalf("%T %v", err, err)
	}
	// The session goes on.
	_ = far.Send([]byte(`{"op":"invoke","id":"rust:2","path":[],"target":"x","method":"echo","args":{"type":"list","value":[{"type":"data","value":"ok"}]}}`))
	if f := next(); f["op"] != "return" {
		t.Fatalf("%v", f)
	}
	open(t, runtime)
}

// counter is an object with methods.
type counter struct{ n int }

func (c *counter) Call(ctx context.Context, method string, args []any) (any, error) {
	c.n++
	return c.n, nil
}
func (c *counter) Get(ctx context.Context, property string) (any, error) { return c.n, nil }

func TestAnObjectGoesOnlyToAFarEndThatTakesObjects(t *testing.T) {
	a, b := endpointPair(t, ValueCapabilities, func(ctx context.Context, target, method string, args []any) (any, error) {
		if method == "object" {
			return &counter{}, nil
		}
		return "fine", nil
	})
	ctx := context.Background()
	_, err := a.Call(ctx, "x", "take", []any{1, &counter{}}, true)
	var refused *ValueError
	want := "argument 2 of x.take: an object with methods (counter) cannot cross to b; pass plain data or a function instead"
	if !errors.As(err, &refused) || err.Error() != want {
		t.Fatalf("%v", err)
	}
	_, err = b.Call(ctx, "x", "object", nil, true)
	var remote *RemoteError
	want = "the result of x.object: an object with methods (counter) cannot cross to b; pass plain data or a function instead"
	if !errors.As(err, &remote) || remote.Message != want {
		t.Fatalf("%v", err)
	}
	if value, err := a.Call(ctx, "x", "after", nil, true); err != nil || text(value) != "fine" {
		t.Fatalf("%v %v", value, err)
	}
	open(t, a)
	open(t, b)
}

// A value a JavaScript plugin threw that is no Error (`throw 'a string'`)
// crosses as the graph of that value, with no nodes: relayed by Go
// (`return err`), it keeps its name, message and graph, its nodes an
// array (null made Node end the session: `invalid error graph`).
func TestAThrownValueRelayedKeepsItsGraph(t *testing.T) {
	thrown := json.RawMessage(`{"root":{"type":"data","value":"a string"},"nodes":[]}`)
	wire, err := json.Marshal(encodeError(&RemoteError{Name: "ThrownValue", Message: "a string", Graph: thrown}))
	if err != nil || string(wire) != `{"graph":{"nodes":[],"root":{"type":"data","value":"a string"}},"message":"a string","name":"ThrownValue"}` {
		t.Fatalf("%s %v", wire, err)
	}
	// Through two sessions: the host's throw, relayed by the runtime.
	var runtime *Peer
	host, r, received := recordedPair(t, func(ctx context.Context, target, method string, args []any) (any, error) {
		return nil, &RemoteError{Name: "ThrownValue", Message: "a string", Graph: thrown}
	}, func(ctx context.Context, target, method string, args []any) (any, error) {
		return runtime.Call(ctx, "host:js", "fail", nil, true)
	})
	runtime = r
	_, err = host.Call(context.Background(), "go", "relay", nil, true)
	var remote *RemoteError
	if !errors.As(err, &remote) || remote.Name != "ThrownValue" || remote.Message != "a string" {
		t.Fatalf("%v", err)
	}
	var relayed []byte
	for _, f := range received.frames() {
		if f.Op == "throw" {
			relayed = f.Error
		}
	}
	var failure struct {
		Graph struct {
			Root  map[string]any
			Nodes json.RawMessage
		}
	}
	if json.Unmarshal(relayed, &failure) != nil || string(failure.Graph.Nodes) != "[]" || failure.Graph.Root["value"] != "a string" {
		t.Fatalf("%s", relayed)
	}
	// A graph without nodes (from an older Go) is not spliced in.
	wire, _ = json.Marshal(encodeError(&RemoteError{Name: "ThrownValue", Message: "old", Graph: json.RawMessage(`{"root":{"type":"reference","value":0},"nodes":null}`)}))
	if !strings.Contains(string(wire), `"nodes":[{`) || !strings.Contains(string(wire), `"message":"old"`) {
		t.Fatalf("%s", wire)
	}
	open(t, host)
}

func TestAMalformedErrorGraphDoesNotEndTheSession(t *testing.T) {
	runtime, far, next := rawHost(t, `[]`)
	for i, graph := range []string{`{"root":{"type":"reference","value":3},"nodes":null}`, `"not a graph"`, `{"nodes":[1,2]}`} {
		done := make(chan error, 1)
		go func() {
			_, err := runtime.Call(context.Background(), "host:x", "m", nil, true)
			done <- err
		}()
		call := next()
		_ = far.Send([]byte(`{"op":"throw","id":"` + call["id"].(string) + `","error":{"name":"TypeError","message":"bad graph","graph":` + graph + `}}`))
		err := testwait.Within(t, "the reply", done)
		var remote *RemoteError
		if !errors.As(err, &remote) || remote.Name != "TypeError" || remote.Message != "bad graph" {
			t.Fatalf("%d: %v", i, err)
		}
		// Thrown on, it still crosses.
		if _, err := json.Marshal(encodeError(remote)); err != nil {
			t.Fatalf("%d: %v", i, err)
		}
	}
	open(t, runtime)
}
