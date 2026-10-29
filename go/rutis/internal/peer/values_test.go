package peer

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"math/big"
	"net"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/testwait"
	"github.com/arcships/rutis/go/rutis/internal/values"
)

func echo(ctx context.Context, target, method string, args []any) (any, error) {
	if method == "echo" {
		return args[0], nil
	}
	return nil, errors.New("no method " + method)
}

// sameJSON reports whether two JSON texts hold the same value.
func sameJSON(t *testing.T, got json.RawMessage, want string) bool {
	t.Helper()
	var a, b any
	if err := json.Unmarshal(got, &a); err != nil {
		t.Fatalf("%s: %v", got, err)
	}
	if err := json.Unmarshal([]byte(want), &b); err != nil {
		t.Fatalf("%s: %v", want, err)
	}
	return reflect.DeepEqual(a, b)
}

func TestTaggedValuesCrossAsTheirTags(t *testing.T) {
	host, _, recorded := recordedPair(t, nil, echo)
	oslo := time.FixedZone("Oslo", 2*3600)
	big1, _ := new(big.Int).SetString("-123456789012345678901", 10)
	for _, c := range []struct {
		sent any
		wire string
	}{
		{time.Date(2026, 10, 15, 5, 22, 15, 123456789, oslo), `{"type":"instant","value":"2026-10-15T03:22:15.123456789Z"}`},
		{values.DateTime{Date: values.Date{Year: 2026, Month: 10, Day: 15}, Time: values.TimeOfDay{Hour: 14, Minute: 30, Second: 5, Nanosecond: 250_000_000}}, `{"type":"datetime","value":"2026-10-15T14:30:05.25"}`},
		{values.Date{Year: 2026, Month: 10, Day: 15}, `{"type":"date","value":"2026-10-15"}`},
		{values.TimeOfDay{Hour: 14, Minute: 30}, `{"type":"time","value":"14:30:00"}`},
		{[]byte("hello"), `{"type":"bytes","value":"aGVsbG8="}`},
		{big1, `{"type":"bigint","value":"-123456789012345678901"}`},
		{values.Decimal("1.10"), `{"type":"decimal","value":"1.10"}`},
		{values.UUID("0f8fad5b-d9cb-469f-a165-70867728950e"), `{"type":"uuid","value":"0f8fad5b-d9cb-469f-a165-70867728950e"}`},
		{
			Map{{int64(1), "a"}, {"1", "b"}, {true, Undefined}},
			`{"type":"map","value":[[{"type":"data","value":1},{"type":"data","value":"a"}],[{"type":"data","value":"1"},{"type":"data","value":"b"}],[{"type":"data","value":true},{"type":"data","value":null}]]}`,
		},
		{Set{"a", int64(1)}, `{"type":"set","value":[{"type":"data","value":"a"},{"type":"data","value":1}]}`},
		{
			map[string]any{"raw": []byte("hi"), "n": int64(1)},
			`{"type":"record","value":{"raw":{"type":"bytes","value":"aGk="},"n":{"type":"data","value":1}}}`,
		},
		{[]any{values.Date{Year: 1, Month: 1, Day: 1}}, `{"type":"list","value":[{"type":"date","value":"0001-01-01"}]}`},
		// Undefined in data is null (design V10).
		{map[string]any{"a": Undefined}, `{"type":"data","value":{"a":null}}`},
	} {
		got, err := host.Call(context.Background(), "x", "echo", []any{c.sent}, true)
		if err != nil {
			t.Fatalf("%v: %v", c.sent, err)
		}
		frames := recorded.frames()
		reply := frames[len(frames)-1]
		if !sameJSON(t, reply.Value, c.wire) {
			t.Errorf("%v: sent as %s, want %s", c.sent, reply.Value, c.wire)
		}
		if sent, ok := c.sent.(time.Time); ok {
			if got := got.(time.Time); !got.Equal(sent) || got.Location() != time.UTC {
				t.Errorf("an instant comes back in UTC to the nanosecond: %v", got)
			}
		}
	}
}

func TestMapKeysAndSetElementsAreCheckedBeforeSending(t *testing.T) {
	host, _ := pair(t, nil, echo)
	for _, c := range []struct {
		sent any
		says string
	}{
		{Map{{[]byte("x"), int64(1)}}, "a map key cannot be bytes"},
		{Map{{time.Unix(0, 0), int64(1)}}, "a map key cannot be an instant"},
		{Map{{nil, int64(1)}}, "a map key cannot be null"},
		{Map{{json.RawMessage(`[1]`), int64(1)}}, "a map key cannot be an array"},
		{Map{{int64(1), "a"}, {float64(1), "b"}}, "duplicate map key 1"},
		{Map{{json.RawMessage(`0`), "a"}, {json.RawMessage(`-0.0`), "b"}}, "duplicate map key -0.0"},
		{Map{{int64(1) << 53, "a"}}, "a map key cannot be the integer 9007199254740992, beyond ±(2^53-1): send it as a bigint"},
		{Map{{json.RawMessage(`-9007199254740992`), "a"}}, "beyond ±(2^53-1)"},
		{Set{"x", "x"}, `duplicate set element "x"`},
		{Set{Set{}}, "a set element cannot be a set"},
		{values.Decimal("1e3"), `invalid decimal "1e3"`},
		{values.UUID("0F8FAD5B-D9CB-469F-A165-70867728950E"), "invalid uuid"},
		{values.Date{Year: 2026, Month: 2, Day: 29}, "invalid date 2026-02-29"},
		{time.Date(10000, 1, 1, 0, 0, 0, 0, time.UTC), "outside years 0001–9999"},
	} {
		_, err := host.Call(context.Background(), "x", "echo", []any{c.sent}, true)
		if err == nil || !strings.Contains(err.Error(), c.says) {
			t.Errorf("%v: %v, want %q", c.sent, err, c.says)
		}
	}
	// Keys of different kinds, or different text, are different keys.
	keys := Map{
		{int64(1), 1}, {"1", 1}, {big.NewInt(1), 1}, {values.Decimal("1"), 1}, {values.Decimal("1.0"), 1},
		{int64(MaxSafe), 1}, {int64(-MaxSafe), 1}, {float64(1.5), 1}, {true, 1},
	}
	if _, err := host.Call(context.Background(), "x", "echo", []any{keys}, true); err != nil {
		t.Fatal(err)
	}
}

// endpointPair is two endpoint-format sessions, a and b; `bDeclares` are
// b's capabilities.
func endpointPair(t *testing.T, bDeclares []string, dispatch func(ctx context.Context, target, method string, args []any) (any, error)) (*Peer, *Peer) {
	t.Helper()
	x, y := net.Pipe()
	a, err := New(NewLines(x), Options{Endpoint: &Endpoint{Local: "a", Expected: "b"}, Capabilities: ValueCapabilities, Dispatch: dispatch})
	if err != nil {
		t.Fatal(err)
	}
	b, err := New(NewLines(y), Options{Endpoint: &Endpoint{Local: "b", Expected: "a"}, Capabilities: bDeclares, Dispatch: dispatch})
	if err != nil {
		t.Fatal(err)
	}
	started := make(chan error, 2)
	for _, p := range []*Peer{a, b} {
		go func(p *Peer) { started <- p.Start() }(p)
	}
	for range 2 {
		if err := testwait.Within(t, "the session to start", started); err != nil {
			t.Fatal(err)
		}
	}
	for _, p := range []*Peer{a, b} {
		testwait.Within(t, "the handshake", p.Ready())
	}
	t.Cleanup(func() {
		a.Close(nil)
		b.Close(nil)
		x.Close()
		y.Close()
	})
	return a, b
}

func TestATaggedValueFailsOnlyItsCallToAFarEndThatDoesNotTakeIt(t *testing.T) {
	a, b := endpointPair(t, []string{"value-date"}, func(ctx context.Context, target, method string, args []any) (any, error) {
		switch method {
		case "bytes":
			return []byte("x"), nil
		case "echo":
			return args[0], nil
		}
		return nil, errors.New("no method")
	})
	ctx := context.Background()
	_, err := a.Call(ctx, "x", "echo", []any{[]any{[]byte("x")}}, true)
	if err == nil || err.Error() != "x.echo: argument 1 cannot be sent: cannot send bytes: the far end does not take bytes values (it did not declare the capability value-bytes)" {
		t.Fatalf("%v", err)
	}
	var refused *ValueError
	if !errors.As(err, &refused) {
		t.Fatalf("%T", err)
	}
	// A declared one crosses; the session goes on.
	if _, err := a.Call(ctx, "x", "echo", []any{values.Date{Year: 2026, Month: 10, Day: 15}}, true); err != nil {
		t.Fatal(err)
	}
	// In a result: the caller receives a throw.
	if _, err := a.Call(ctx, "x", "bytes", nil, true); err != nil {
		t.Fatal(err)
	}
	_, err = b.Call(ctx, "x", "bytes", nil, true)
	if err == nil || !strings.Contains(err.Error(), "did not declare the capability value-bytes") {
		t.Fatalf("%v", err)
	}
	if value, err := a.Call(ctx, "x", "echo", []any{"still"}, true); err != nil || text(value) != "still" {
		t.Fatalf("%v %v", value, err)
	}
}

// rawHost is a far end written frame by frame: it greets the Go session
// (as the host `main`) with `capabilities` and returns what it
// receives, one frame at a time.
func rawHost(t *testing.T, capabilities string) (*Peer, Channel, func() map[string]any) {
	t.Helper()
	x, y := net.Pipe()
	runtime, err := New(NewLines(x), Options{Endpoint: &Endpoint{Local: "go"}, Capabilities: TestCapabilities, Dispatch: echo})
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
	if err := far.Send([]byte(`{"op":"hello","version":4,"endpoint":"main","capabilities":` + capabilities + `}`)); err != nil {
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
	t.Cleanup(func() {
		runtime.Close(nil)
		x.Close()
		y.Close()
	})
	return runtime, far, next
}

func TestARefusedCallReleasesTheReferencesItCarries(t *testing.T) {
	runtime, far, next := rawHost(t, `[]`)
	reference := func(id int) string {
		return `{"type":"reference","value":{"id":` + string(rune('0'+id)) + `,"kind":"function","home":false,"origin":[]}}`
	}
	for i, c := range []struct {
		value string
		says  string
	}{
		{`{"type":"bytes","value":"Zh=="}`, "invalid bytes (4 characters)"},
		{`{"type":"instant","value":"2026-10-15T03:22:15+00:00"}`, `invalid instant "2026-10-15T03:22:15+00:00"`},
		{`{"type":"map","value":[[{"type":"data","value":1},` + reference(2) + `],[{"type":"data","value":1.0},{"type":"data","value":0}]]}`, "duplicate map key 1.0"},
		{`{"type":"set","value":[{"type":"data","value":9007199254740992},` + `{"type":"list","value":[` + reference(3) + `]}]}`, "a set element cannot be the integer 9007199254740992"},
		{`{"type":"uuid","value":"0F8FAD5B-D9CB-469F-A165-70867728950E"}`, "invalid uuid"},
		{`{"type":"bigint","value":"-0"}`, `invalid bigint "-0"`},
	} {
		id := `"main:` + string(rune('1'+i)) + `"`
		args := `{"type":"list","value":[` + c.value + `,` + reference(1) + `]}`
		frame := `{"op":"invoke","id":` + id + `,"path":[],"target":"x","method":"echo","args":` + args + `}`
		if err := far.Send([]byte(frame)); err != nil {
			t.Fatal(err)
		}
		released := map[float64]bool{}
		for {
			f := next()
			if f["op"] == "release" {
				if f["count"] != float64(1) {
					t.Errorf("%v", f)
				}
				released[f["reference"].(float64)] = true
				continue
			}
			if f["op"] != "throw" || `"`+f["id"].(string)+`"` != id {
				t.Fatalf("%s: %v", c.value, f)
			}
			if message := f["error"].(map[string]any)["message"].(string); !strings.Contains(message, c.says) {
				t.Errorf("%s: %s, want %q", c.value, message, c.says)
			}
			break
		}
		if !released[1] {
			t.Errorf("%s: the reference it carried was not released before the throw", c.value)
		}
		if strings.Contains(c.value, `"id":2`) && !released[2] || strings.Contains(c.value, `"id":3`) && !released[3] {
			t.Errorf("%s: a reference inside the refused value was not released", c.value)
		}
	}
	runtime.mu.Lock()
	imports := len(runtime.imports)
	runtime.mu.Unlock()
	if imports != 0 {
		t.Errorf("%d imports left", imports)
	}
	// The session goes on.
	_ = far.Send([]byte(`{"op":"invoke","id":"main:9","path":[],"target":"x","method":"echo","args":{"type":"list","value":[{"type":"data","value":"ok"}]}}`))
	if f := next(); f["op"] != "return" {
		t.Fatalf("%v", f)
	}
}

func TestAValueTheCallerCannotTakeFailsItsCall(t *testing.T) {
	runtime, far, next := rawHost(t, `[]`)
	done := make(chan error, 1)
	go func() {
		_, err := runtime.Call(context.Background(), "host:x", "m", nil, true)
		done <- err
	}()
	call := next()
	_ = far.Send([]byte(`{"op":"return","id":"` + call["id"].(string) + `","value":{"type":"date","value":"2026-02-29"}}`))
	err := testwait.Within(t, "the reply", done)
	if err == nil || !strings.Contains(err.Error(), `invalid date "2026-02-29"`) {
		t.Fatalf("%v", err)
	}
	// The far end declared no values: this side sends it none.
	_, err = runtime.Call(context.Background(), "host:x", "m", []any{Set{}}, true)
	if err == nil || !strings.Contains(err.Error(), "value-set") {
		t.Fatalf("%v", err)
	}
	select {
	case <-runtime.Closed():
		t.Fatal(runtime.Err())
	default:
	}
}

func TestAValueOfTheWrongShapeIsAMalformedFrame(t *testing.T) {
	for _, value := range []string{
		`{"type":"instant","value":5}`,
		`{"type":"map","value":[[{"type":"data","value":1}]]}`,
		`{"type":"set","value":{}}`,
	} {
		runtime, far, _ := rawHost(t, `[]`)
		_ = far.Send([]byte(`{"op":"invoke","id":"main:1","path":[],"target":"x","method":"echo","args":{"type":"list","value":[` + value + `]}}`))
		testwait.Within(t, "the session to end", runtime.Closed())
		if !strings.Contains(runtime.Err().Error(), "invalid") {
			t.Errorf("%s: %v", value, runtime.Err())
		}
	}
}

func TestAnUndeclaredTagIsRefusedEvenInsideAMap(t *testing.T) {
	x, y := net.Pipe()
	defer x.Close()
	defer y.Close()
	older, err := New(NewLines(x), Options{Endpoint: &Endpoint{Local: "b", Expected: "a"}, Dispatch: echo})
	if err != nil {
		t.Fatal(err)
	}
	far := NewLines(y)
	go func() { _ = older.Start() }()
	if _, err := far.Recv(); err != nil {
		t.Fatal(err)
	}
	_ = far.Send([]byte(`{"op":"hello","version":4,"endpoint":"a","capabilities":[]}`))
	testwait.Within(t, "the handshake", older.Ready())
	_ = far.Send([]byte(`{"op":"invoke","id":"a:1","path":[],"target":"x","method":"echo","args":{"type":"list","value":[{"type":"map","value":[[{"type":"data","value":"k"},{"type":"reference","value":{"id":4,"kind":"function","home":false,"origin":[]}}]]}]}}`))
	var frames []string
	for len(frames) < 2 {
		message, err := far.Recv()
		if err != nil {
			t.Fatal(err)
		}
		frames = append(frames, string(message))
	}
	if !strings.Contains(frames[0], `"op":"release"`) || !strings.Contains(frames[1], "received a map value, which this side does not take (it did not declare the capability value-map)") {
		t.Fatalf("%s", frames)
	}
	if !bytes.Contains([]byte(frames[1]), []byte(`"op":"throw"`)) {
		t.Fatalf("%s", frames[1])
	}
}
