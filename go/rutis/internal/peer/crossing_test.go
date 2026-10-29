// guarantee: sdk/error-codes
// guarantee: bridge/bad-values
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
	if err == nil || !errors.As(err, &refused) || !strings.HasPrefix(err.Error(), "svc.take: argument 2 cannot be sent: the message would be ") || !strings.HasSuffix(err.Error(), overLimitTail) {
		t.Fatalf("%T %v", err, err)
	}
	// guarantee: sdk/error-texts (a value that cannot be sent: code
	// bad-value, data {argument, part, limit}: #677)
	if data, _ := json.Marshal(refused.Data()); refused.Code() != "bad-value" || string(data) != `{"argument":2,"limit":16777216,"part":"argument"}` {
		t.Fatalf("%s %s", refused.Code(), data)
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
	if !errors.As(err, &remote) || !strings.HasPrefix(remote.Message, "conformance.big: the result cannot be sent: the message would be ") || !strings.HasSuffix(remote.Message, overLimitTail) {
		t.Fatalf("%v", err)
	}
	if data, _ := json.Marshal(remote.Data); remote.Code != "bad-value" || string(data) != `{"limit":16777216,"part":"result"}` {
		t.Fatalf("%s %s", remote.Code, data)
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

// objects-case(go): send-call-state
// objects-case(go): same-proxy-home
// An object of the far end is a *RemoteObject: calls of its methods are
// call frames naming the method (blocking ones marked synchronous, for the
// host's wait-cycle check); handed back, it is the far end's own again; the
// same object granted twice is one proxy (#687).
func TestAnObjectFromTheFarEndIsCalledAndGoesHome(t *testing.T) {
	runtime, far, next := rawHost(t, `[]`)
	object := `{"type":"reference","value":{"id":7,"kind":"object","home":false,"origin":[]}}`
	// echo hands it back: home.
	_ = far.Send([]byte(`{"op":"invoke","id":"main:1","path":[],"target":"x","method":"echo","args":{"type":"list","value":[` + object + `]}}`))
	f := next()
	value, _ := f["value"].(map[string]any)
	if f["op"] != "return" || value["type"] != "reference" || value["value"].(map[string]any)["home"] != true {
		t.Fatalf("%v", f)
	}
	// Received as a result: a *RemoteObject, one per object.
	got := make(chan any, 2)
	for range 2 {
		go func() {
			value, err := runtime.Call(context.Background(), "host:x", "m", nil, false)
			if err != nil {
				got <- err
				return
			}
			got <- value
		}()
		call := next()
		_ = far.Send([]byte(`{"op":"return","id":"` + call["id"].(string) + `","value":` + object + `}`))
	}
	first, second := testwait.Within(t, "a result", got), testwait.Within(t, "a result", got)
	remote, ok := first.(*RemoteObject)
	if !ok || first != second {
		t.Fatalf("%T %v / %v", first, first, second)
	}
	for _, sync := range []bool{true, false} {
		done := make(chan any, 1)
		go func() {
			value, err := remote.Call(context.Background(), "inc", []any{1}, sync)
			if err != nil {
				done <- err
				return
			}
			done <- value
		}()
		call := next()
		if call["op"] != "call" || call["method"] != "inc" || call["reference"] != float64(7) || (call["sync"] == true) != sync {
			t.Fatalf("%v", call)
		}
		_ = far.Send([]byte(`{"op":"return","id":"` + call["id"].(string) + `","value":{"type":"data","value":2}}`))
		if value := testwait.Within(t, "the result", done); string(value.(json.RawMessage)) != "2" {
			t.Fatalf("%v", value)
		}
	}
	open(t, runtime)
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
