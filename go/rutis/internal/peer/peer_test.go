package peer

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"math/big"
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
	h, err := New(hostChannel, Options{Endpoint: &Endpoint{Local: "main"}, Capabilities: TestCapabilities, Dispatch: host})
	if err != nil {
		t.Fatal(err)
	}
	r, err := New(NewLines(b), Options{Endpoint: &Endpoint{Local: "go"}, Capabilities: TestCapabilities, Dispatch: runtime})
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
	if len(seen) != 2 || seen[0] != "main:1" || !strings.HasPrefix(seen[1], "go:") {
		t.Fatalf("the callback carries the chain main:1 -> go:N: %v", seen)
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

// An announcement the session's end leaves unanswered is not a refusal:
// `failed` hears only of the far end's.
func TestAnAnnouncementTheSessionEndsIsNotRefused(t *testing.T) {
	held := make(chan struct{})
	defer close(held)
	_, runtime := pair(t,
		func(ctx context.Context, target, method string, args []any) (any, error) {
			if method == "refused" {
				return nil, Errorf("BindingError", "no")
			}
			<-held
			return nil, nil
		}, nil)
	var mu sync.Mutex
	var failures []string
	failed := func(err error) {
		mu.Lock()
		failures = append(failures, err.Error())
		mu.Unlock()
	}
	testwait.Within(t, "the refusal is answered", runtime.AnnounceReporting("", "refused", nil, failed))
	unanswered := runtime.AnnounceReporting("", "held", nil, failed)
	runtime.Close(nil)
	testwait.Within(t, "the session's end settles it", unanswered)
	mu.Lock()
	defer mu.Unlock()
	if fmt.Sprint(failures) != "[BindingError: no]" {
		t.Fatalf("failures %v", failures)
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
	runtime, _ := New(NewLines(a), Options{Endpoint: &Endpoint{Local: "go"}})
	go func() {
		reader := NewLines(b)
		_, _ = reader.Recv() // its hello
		_ = reader.Send([]byte(`{"op":"invoke","id":"main:1","path":[],"target":"x","method":"m","args":{"type":"list","value":[]}}`))
	}()
	_ = runtime.Start()
	testwait.Within(t, "the session to end", runtime.Closed())
	if !strings.Contains(runtime.Err().Error(), "before protocol handshake") {
		t.Fatal(runtime.Err())
	}
}

func TestSynchronousCallsSayTheyWait(t *testing.T) {
	// A frame as the far end reads it, `sync` included.
	type seen struct {
		frame
		Sync json.RawMessage `json:"sync"`
	}
	a, b := net.Pipe()
	defer b.Close()
	defer a.Close()
	runtime, _ := New(NewLines(a), Options{Endpoint: &Endpoint{Local: "go", Expected: "main"}, Capabilities: []string{"reentrant-sync"}})
	far := NewLines(b)
	calls := make(chan seen, 2)
	go func() {
		_, _ = far.Recv() // its hello
		_ = far.Send([]byte(`{"op":"hello","version":4,"endpoint":"main","capabilities":[]}`))
		for {
			message, err := far.Recv()
			if err != nil {
				return
			}
			var f seen
			_ = json.Unmarshal(message, &f)
			calls <- f
			_ = far.Send([]byte(`{"op":"return","id":` + string(f.ID) + `,"value":{"type":"undefined"}}`))
		}
	}()
	_ = runtime.Start()
	defer runtime.Close(nil)
	testwait.Within(t, "the handshake", runtime.Ready())
	for _, sync := range []bool{true, false} {
		if _, err := runtime.Call(context.Background(), "host:x", "m", nil, sync); err != nil {
			t.Fatal(err)
		}
		want := ""
		if sync {
			want = "true"
		}
		if got := (<-calls).Sync; string(got) != want {
			t.Fatalf("sync %v: the call says sync %q", sync, got)
		}
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

// A reference's origin holds this session's ids and other sessions'
// tagged with them: refusing those ended the session of a nested call
// (#225).
func TestOriginsTakeOtherSessionsTaggedIds(t *testing.T) {
	for _, id := range []string{"go:1", "main:12", "s1/main:7", "s1/node:2", "s12/mac-2:3"} {
		if !originID.MatchString(id) {
			t.Errorf("%s: refused", id)
		}
	}
	for _, id := range []string{"go:0", "s1/s2/node:1", "s1/node", "S1/node:1", "node:01", "Go:1"} {
		if originID.MatchString(id) {
			t.Errorf("%s: taken", id)
		}
	}
}

// Risk P8: a far end that greets with a protocol version this side does
// not speak is refused at the handshake, and the refusal says which end to
// upgrade (standard Q6.2.4): this side, naming its module, when the far end
// is newer; the far end, naming its implementation when it said, when it
// is older. The wording is rutis-bridge's (`session::rpc::version_mismatch`).
// guarantee: bridge/version-refusal
func TestAFarEndOfAnotherProtocolVersionIsRefusedSayingWhichEndToUpgrade(t *testing.T) {
	const module = "upgrade this side (Go module github.com/arcships/rutis/go/rutis)"
	for _, c := range []struct {
		far     string
		version int
		hint    string
	}{
		{`{"op":"hello","version":99,"endpoint":"main","capabilities":[]}`, 99, module},
		// rutis 0.18 speaks protocol 3.
		{`{"op":"hello","version":3,"endpoint":"main","implementation":{"name":"rutis-bridge","version":"0.18.0"},"capabilities":[]}`, 3, "upgrade the far end (rutis-bridge 0.18.0)"},
		{`{"op":"hello","version":1,"endpoint":"main","implementation":{"name":"rutis","version":"0.1.0"},"capabilities":[]}`, 1, "upgrade the far end (rutis 0.1.0)"},
		{`{"op":"hello","version":1,"endpoint":"main","capabilities":[]}`, 1, "upgrade the far end"},
	} {
		var f frame
		if err := json.Unmarshal([]byte(c.far), &f); err != nil {
			t.Fatal(err)
		}
		p := &Peer{opts: Options{Endpoint: &Endpoint{Local: "go"}}}
		want := fmt.Sprintf("incompatible session: the far end speaks protocol %d, this side %d: %s", c.version, EndpointProtocol, c.hint)
		if err := p.greet(&f); err == nil || err.Error() != want {
			t.Errorf("%s: got %v, want %q", c.far, err, want)
		}
	}
}

// guarantee: bridge/error-code-and-data
// A Rust service's error decodes with its code and data (#440), a big
// integer in the data as a *big.Int.
func TestARustServiceErrorHasItsCodeAndData(t *testing.T) {
	err := decodeError(json.RawMessage(`{"name":"CalendarError","message":"no entry 7","graph":{
		"root":{"type":"reference","value":0},
		"nodes":[{"type":"error","name":"CalendarError","message":"no entry 7","code":"NotFound","data":{"type":"reference","value":1}},
		         {"type":"object","values":[["NotFound",{"type":"bigint","value":"18446744073709551615"}],["at",{"type":"data","value":[1,"x"]}]]}]}}`))
	var remote *RemoteError
	if !errors.As(err, &remote) || remote.Name != "CalendarError" || remote.Code != "NotFound" {
		t.Fatalf("decoded as %#v", err)
	}
	data := remote.Data.(map[string]any)
	if data["NotFound"].(*big.Int).String() != "18446744073709551615" || fmt.Sprint(data["at"]) != "[1 x]" {
		t.Fatalf("data: %#v", remote.Data)
	}
	plain := decodeError(json.RawMessage(`{"name":"Error","message":"plain"}`)).(*RemoteError)
	if plain.Code != "" || plain.Data != nil {
		t.Fatalf("a plain error: %#v", plain)
	}
}

type stormy struct{}

func (stormy) Error() string { return "too windy" }
func (stormy) Code() string  { return "Stormy" }
func (stormy) Data() any     { return map[string]any{"wind": uint64(1) << 60, "gusts": []int{1, 2}} }

// An error with Code() and Data() crosses with them; data that cannot be
// read (or sent) leaves the code; numbers and tagged values decode.
func TestAnErrorWithACodeAndDataCrossesWithThem(t *testing.T) {
	wire, err := json.Marshal(encodeError(stormy{}))
	if err != nil {
		t.Fatal(err)
	}
	remote := decodeError(wire).(*RemoteError)
	data := remote.Data.(map[string]any)
	if remote.Code != "Stormy" || data["wind"].(*big.Int).String() != "1152921504606846976" || fmt.Sprint(data["gusts"]) != "[1 2]" {
		t.Fatalf("decoded as %#v", remote)
	}
	withData := func(data string) *RemoteError {
		return decodeError(json.RawMessage(`{"name":"E","message":"m","graph":{"root":{"type":"reference","value":0},
			"nodes":[{"type":"error","name":"E","message":"m","code":"C","data":` + data + `}]}}`)).(*RemoteError)
	}
	if broken := withData(`{"type":"nonsense"}`); broken.Code != "C" || broken.Data != nil {
		t.Fatalf("broken data: %#v", broken)
	}
	if inf := withData(`{"type":"number","value":"Infinity"}`); !math.IsInf(inf.Data.(float64), 1) {
		t.Fatalf("number: %#v", inf.Data)
	}
	if raw := withData(`{"type":"bytes","value":"aGk="}`); fmt.Sprint(raw.Data) != "[104 105]" {
		t.Fatalf("bytes: %#v", raw.Data)
	}
}

// A RemoteError thrown on crosses with its whole graph, alone as inside an
// aggregate: as Node's decoded Error re-encodes.
func TestARemoteErrorThrownOnCrossesWithItsWholeGraph(t *testing.T) {
	far := func() *RemoteError {
		return &RemoteError{Name: "TypeError", Message: "far", Graph: json.RawMessage(`{
		"root":{"type":"reference","value":0},
		"nodes":[{"type":"error","name":"TypeError","message":"far","cause":{"type":"reference","value":1}},
		         {"type":"error","name":"Error","message":"root cause"}]
	}`)}
	}
	wire := encodeError(far())
	nodes := wire["graph"].(map[string]any)["nodes"].([]any)
	if wire["name"] != "TypeError" || wire["message"] != "far" || len(nodes) != 2 || nodes[0].(map[string]any)["cause"].(map[string]any)["value"] != float64(1) || nodes[1].(map[string]any)["message"] != "root cause" {
		t.Fatalf("the graph of a remote error thrown on: %v", wire)
	}
	wire = encodeError(&AggregateError{Errors: []error{errors.New("near"), far()}})
	nodes = wire["graph"].(map[string]any)["nodes"].([]any)
	names := make([]any, len(nodes))
	for i, node := range nodes {
		names[i] = node.(map[string]any)["name"]
	}
	if fmt.Sprint(names) != "[AggregateError Error TypeError Error]" || nodes[2].(map[string]any)["cause"].(map[string]any)["value"] != float64(3) {
		t.Fatalf("the graph of a remote error inside an aggregate: %v", wire)
	}
}

func TestAggregateErrorGraphsKeepRepeatedAndCyclicChildren(t *testing.T) {
	remote := &RemoteError{Name: "AggregateError", Graph: json.RawMessage(`{
		"root":{"type":"reference","value":0},
		"nodes":[{"type":"error","name":"AggregateError","message":"","errors":[{"type":"reference","value":1}]},
		         {"type":"error","name":"TypeError","message":"remote child"}]
	}`)}
	cyclic := &AggregateError{}
	cyclic.Errors = []error{cyclic, nil, errors.New("child"), remote, cyclic}
	wire := encodeError(cyclic)
	graph := wire["graph"].(map[string]any)
	nodes := graph["nodes"].([]any)
	children := nodes[0].(map[string]any)["errors"].([]any)
	if len(nodes) != 4 || len(children) != 4 || children[0].(map[string]any)["value"] != 0 || children[3].(map[string]any)["value"] != 0 || nodes[1].(map[string]any)["message"] != "child" {
		t.Fatalf("invalid aggregate graph: %v", graph)
	}
	imported := nodes[2].(map[string]any)["errors"].([]any)
	if imported[0].(map[string]any)["value"] != float64(3) || nodes[3].(map[string]any)["name"] != "TypeError" || nodes[3].(map[string]any)["message"] != "remote child" {
		t.Fatalf("nested remote aggregate lost: %v", graph)
	}
}

func TestAggregateErrorGraphsRepeatRemoteChildrenAtTheirOwnNodes(t *testing.T) {
	remote := func() *RemoteError {
		return &RemoteError{Name: "AggregateError", Graph: json.RawMessage(`{
			"root":{"type":"reference","value":0},
			"nodes":[{"type":"error","name":"AggregateError","message":"","errors":[{"type":"reference","value":1}]},
			         {"type":"error","name":"TypeError","message":"remote child"}]
		}`)}
	}
	// checkRemote asserts a reference lands on an imported remote root whose
	// child reference lands on the imported TypeError.
	checkRemote := func(t *testing.T, nodes []any, ref any) {
		t.Helper()
		id := int(toFloat(ref.(map[string]any)["value"]))
		root := nodes[id].(map[string]any)
		if root["name"] != "AggregateError" {
			t.Fatalf("reference %d lands on %v", id, root)
		}
		child := int(toFloat(root["errors"].([]any)[0].(map[string]any)["value"]))
		if leaf := nodes[child].(map[string]any); leaf["name"] != "TypeError" || leaf["message"] != "remote child" {
			t.Fatalf("remote child reference %d lands on %v", child, leaf)
		}
	}

	t.Run("flat", func(t *testing.T) {
		shared := remote()
		wire := encodeError(&AggregateError{Errors: []error{shared, errors.New("between"), shared}})
		nodes := wire["graph"].(map[string]any)["nodes"].([]any)
		children := nodes[0].(map[string]any)["errors"].([]any)
		t.Logf("nodes=%v children=%v", nodes, children)
		checkRemote(t, nodes, children[0])
		checkRemote(t, nodes, children[2])
		if nodes[toInt(children[1])].(map[string]any)["message"] != "between" {
			t.Fatalf("middle child misplaced: %v", children)
		}
	})

	t.Run("nested", func(t *testing.T) {
		shared := remote()
		inner := &AggregateError{Errors: []error{shared, shared}}
		outer := &AggregateError{Errors: []error{shared, inner, shared, inner}}
		wire := encodeError(outer)
		nodes := wire["graph"].(map[string]any)["nodes"].([]any)
		children := nodes[0].(map[string]any)["errors"].([]any)
		t.Logf("nodes=%v children=%v", nodes, children)
		checkRemote(t, nodes, children[0])
		checkRemote(t, nodes, children[2])
		if children[1].(map[string]any)["value"] != children[3].(map[string]any)["value"] {
			t.Fatalf("repeated inner aggregate split: %v", children)
		}
		innerNode := nodes[toInt(children[1])].(map[string]any)
		innerChildren := innerNode["errors"].([]any)
		checkRemote(t, nodes, innerChildren[0])
		checkRemote(t, nodes, innerChildren[1])
		// The graph must survive a JSON round trip (what a receiver parses).
		raw, err := json.Marshal(wire)
		if err != nil {
			t.Fatal(err)
		}
		decoded := decodeError(raw).(*RemoteError)
		if decoded.Name != "AggregateError" || decoded.Message != "" || len(decoded.Graph) == 0 {
			t.Fatalf("decoded %#v", decoded)
		}
	})
}

func toFloat(value any) float64 {
	switch value := value.(type) {
	case int:
		return float64(value)
	case float64:
		return value
	}
	panic(fmt.Sprintf("not a number: %#v", value))
}

func toInt(ref any) int { return int(toFloat(ref.(map[string]any)["value"])) }

// guarantee: sdk/error-codes, sdk/error-texts (a call in flight when the host goes fails with
// `the host disconnected`, code peer-gone: #677)
func TestACallFailsSayingTheHostDisconnected(t *testing.T) {
	entered := make(chan struct{})
	host, runtime := pair(t, func(ctx context.Context, target, method string, args []any) (any, error) {
		close(entered)
		<-ctx.Done()
		return nil, ctx.Err()
	}, nil)
	failed := make(chan error, 1)
	go func() {
		_, err := runtime.Call(context.Background(), "", "wait", nil, false)
		failed <- err
	}()
	testwait.Within(t, "the call to reach the host", entered)
	host.channel.Close("gone")
	err := testwait.Within(t, "the call to fail", failed)
	if err == nil || err.Error() != "the host disconnected" {
		t.Fatalf("%v", err)
	}
	gone, ok := err.(*Coded)
	if !ok || gone.Code() != "peer-gone" || gone.Name() != "BindingError" {
		t.Fatalf("%#v", err)
	}
	if reason := gone.Data().(map[string]any)["reason"]; reason != "disconnected" {
		t.Fatal(reason)
	}
	// Later calls fail the same way.
	if _, err := runtime.Call(context.Background(), "", "again", nil, false); err == nil || err.Error() != "the host disconnected" {
		t.Fatalf("%v", err)
	}
}
