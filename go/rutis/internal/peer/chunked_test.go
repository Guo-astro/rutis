// guarantee: bridge/large-and-deep-values
// A frame over the channel's message limit, carried by `chunk` frames and
// reassembled by the far end (#1363). A chunk is not a call: the method runs
// once, after the last chunk, and never for a transfer that does not finish.

package peer

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// chunkHost is a runtime session on a pipe with a far end that sends `hello`
// declaring `capabilities`. Its message limit and reassembly cap are lowered
// so a test need not carry 16 MiB.
func chunkHost(t *testing.T, capabilities string, maxMessage, maxTransfer int) (*Peer, Channel) {
	t.Helper()
	return chunkHostWith(t, capabilities, maxMessage, maxTransfer, func(ctx context.Context, target, method string, args []any) (any, error) {
		return arg(args, 0), nil
	})
}

func chunkHostWith(t *testing.T, capabilities string, maxMessage, maxTransfer int, dispatch func(ctx context.Context, target, method string, args []any) (any, error)) (*Peer, Channel) {
	t.Helper()
	x, y := net.Pipe()
	runtime, err := New(NewLines(x), Options{Endpoint: &Endpoint{Local: "go"}, Capabilities: TestCapabilities, Dispatch: dispatch})
	if err != nil {
		t.Fatal(err)
	}
	if maxMessage > 0 {
		runtime.maxMessage = maxMessage
	}
	if maxTransfer > 0 {
		runtime.maxTransfer = maxTransfer
	}
	far := NewLines(y)
	go func() { _ = runtime.Start() }()
	if _, err := far.Recv(); err != nil {
		t.Fatalf("the hello: %v", err)
	}
	if err := far.Send([]byte(`{"op":"hello","version":5,"endpoint":"main","capabilities":` + capabilities + `}`)); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the handshake", runtime.Ready())
	t.Cleanup(func() {
		runtime.Close(nil)
		x.Close()
		y.Close()
	})
	return runtime, far
}

// arg is a call's argument `i`, Undefined when absent.
func arg(args []any, i int) any {
	if i < len(args) {
		return args[i]
	}
	return Undefined
}

func chunkFrame(transfer string, seq int, last bool, part []byte) []byte {
	frame, _ := json.Marshal(map[string]any{
		"op": "chunk", "transfer": transfer, "seq": seq, "last": last,
		"data": base64.StdEncoding.EncodeToString(part),
	})
	return frame
}

// invokeBytes is an invoke frame for `value`, encoded without a call id.
func invokeBytes(id, value string) []byte {
	return []byte(fmt.Sprintf(`{"op":"invoke","id":%q,"path":[],"target":"test","method":"echo","args":{"type":"list","value":[{"type":"data","value":%q}]}}`, id, value))
}

func partsOf(data []byte, size int) [][]byte {
	var out [][]byte
	for at := 0; at < len(data); at += size {
		out = append(out, data[at:min(at+size, len(data))])
	}
	return out
}

func TestAChunkedCallIsDispatchedOnceAfterItsLastChunk(t *testing.T) {
	var calls atomic.Int32
	runtime, far := chunkHostWith(t, `["chunked"]`, 0, 0, func(ctx context.Context, target, method string, args []any) (any, error) {
		calls.Add(1)
		return arg(args, 0), nil
	})
	_ = runtime
	value := strings.Repeat("x", 4096)
	parts := partsOf(invokeBytes("main:1", value), 16)
	if len(parts) < 2 {
		t.Fatalf("%d parts", len(parts))
	}
	for seq, part := range parts[:len(parts)-1] {
		if err := far.Send(chunkFrame("main:1", seq, false, part)); err != nil {
			t.Fatal(err)
		}
	}
	if calls.Load() != 0 {
		t.Fatalf("the call ran before the last chunk")
	}
	if err := far.Send(chunkFrame("main:1", len(parts)-1, true, parts[len(parts)-1])); err != nil {
		t.Fatal(err)
	}
	message, err := far.Recv()
	if err != nil {
		t.Fatal(err)
	}
	var reply map[string]any
	if err := json.Unmarshal(message, &reply); err != nil || reply["op"] != "return" {
		t.Fatalf("%s %v", message, err)
	}
	encoded, _ := json.Marshal(reply["value"])
	if !strings.Contains(string(encoded), value) {
		t.Fatalf("the call did not receive the whole value: %s", encoded)
	}
	if calls.Load() != 1 {
		t.Fatalf("the call ran %d times", calls.Load())
	}
}

func TestATransferTheSessionNeverFinishesIsNeverDispatched(t *testing.T) {
	var calls atomic.Int32
	runtime, far := chunkHostWith(t, `["chunked"]`, 0, 0, func(ctx context.Context, target, method string, args []any) (any, error) {
		calls.Add(1)
		return nil, nil
	})
	parts := partsOf(invokeBytes("main:1", "value"), 8)
	for seq, part := range parts[:len(parts)-2] {
		if err := far.Send(chunkFrame("main:1", seq, false, part)); err != nil {
			t.Fatal(err)
		}
	}
	runtime.Close(nil)
	testwait.Within(t, "the session to end", runtime.Closed())
	if calls.Load() != 0 {
		t.Fatal("a call was dispatched from an incomplete transfer")
	}
}

func TestAChunkWithAnUnknownFieldEndsTheSession(t *testing.T) {
	runtime, far := chunkHost(t, `["chunked"]`, 0, 0)
	if err := far.Send([]byte(`{"op":"chunk","transfer":"main:1","seq":0,"last":true,"data":"","extra":1}`)); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the session to end", runtime.Closed())
	if err := runtime.Err(); err == nil || !strings.Contains(err.Error(), `unknown field "extra"`) {
		t.Fatalf("%v", err)
	}
}

func TestAChunkOutOfOrderEndsTheSession(t *testing.T) {
	runtime, far := chunkHost(t, `["chunked"]`, 0, 0)
	if err := far.Send(chunkFrame("main:1", 1, false, []byte("{"))); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the session to end", runtime.Closed())
	if err := runtime.Err(); err == nil || !strings.Contains(err.Error(), "chunked transfer main:1: expected chunk 0, got 1") {
		t.Fatalf("%v", err)
	}
}

func TestASecondTransferBeforeTheFirstEndedEndsTheSession(t *testing.T) {
	runtime, far := chunkHost(t, `["chunked"]`, 0, 0)
	if err := far.Send(chunkFrame("main:1", 0, false, []byte("{"))); err != nil {
		t.Fatal(err)
	}
	if err := far.Send(chunkFrame("main:2", 0, false, []byte("{"))); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the session to end", runtime.Closed())
	if err := runtime.Err(); err == nil || !strings.Contains(err.Error(), "transfer main:2 started before it ended") {
		t.Fatalf("%v", err)
	}
}

func TestAChunkBeforeTheHandshakeEndsTheSession(t *testing.T) {
	x, y := net.Pipe()
	runtime, err := New(NewLines(x), Options{Endpoint: &Endpoint{Local: "go"}, Capabilities: TestCapabilities})
	if err != nil {
		t.Fatal(err)
	}
	far := NewLines(y)
	go func() { _ = runtime.Start() }()
	if _, err := far.Recv(); err != nil {
		t.Fatal(err)
	}
	if err := far.Send(chunkFrame("main:1", 0, true, []byte("{}"))); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the session to end", runtime.Closed())
	if err := runtime.Err(); err == nil || !strings.Contains(err.Error(), "before protocol handshake") {
		t.Fatalf("%v", err)
	}
	x.Close()
	y.Close()
}

func TestAChunkFromAFarEndWithoutChunkedEndsTheSession(t *testing.T) {
	runtime, far := chunkHost(t, `[]`, 0, 0)
	if err := far.Send(chunkFrame("main:1", 0, true, []byte("{}"))); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the session to end", runtime.Closed())
	if err := runtime.Err(); err == nil || !strings.Contains(err.Error(), "did not declare chunked") {
		t.Fatalf("%v", err)
	}
}

func TestAChunkWithInvalidBase64EndsTheSession(t *testing.T) {
	runtime, far := chunkHost(t, `["chunked"]`, 0, 0)
	if err := far.Send([]byte(`{"op":"chunk","transfer":"main:1","seq":0,"last":true,"data":"not base64!"}`)); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the session to end", runtime.Closed())
	if err := runtime.Err(); err == nil || !strings.Contains(err.Error(), "chunked transfer main:1: invalid bytes (11 characters)") {
		t.Fatalf("%v", err)
	}
}

func TestATransferOverTheReassemblyLimitEndsTheSession(t *testing.T) {
	runtime, far := chunkHost(t, `["chunked"]`, 0, 1024)
	if err := far.Send(chunkFrame("main:1", 0, false, make([]byte, 1024))); err != nil {
		t.Fatal(err)
	}
	if err := far.Send(chunkFrame("main:1", 1, true, []byte("{"))); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the session to end", runtime.Closed())
	if err := runtime.Err(); err == nil || !strings.Contains(err.Error(), "chunked transfer main:1: over the limit of 1024 bytes") {
		t.Fatalf("%v", err)
	}
}

func TestAChunkInsideAChunkedTransferEndsTheSession(t *testing.T) {
	runtime, far := chunkHost(t, `["chunked"]`, 0, 0)
	nested := chunkFrame("main:2", 0, true, []byte("{}"))
	if err := far.Send(chunkFrame("main:1", 0, true, nested)); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the session to end", runtime.Closed())
	if err := runtime.Err(); err == nil || !strings.Contains(err.Error(), "inside a chunked transfer") {
		t.Fatalf("%v", err)
	}
}

func TestAFrameOverTheMessageLimitIsSentAsChunksThatFitIt(t *testing.T) {
	runtime, far := chunkHost(t, `["chunked"]`, 4096, 0)
	big := strings.Repeat("x", 8192)
	type outcome struct {
		value any
		err   error
	}
	done := make(chan outcome, 1)
	go func() {
		value, err := runtime.Call(context.Background(), "host:svc", "take", []any{big}, true)
		done <- outcome{value, err}
	}()
	var transferred []byte
	var transfer any
	seq := 0
	for {
		message, err := far.Recv()
		if err != nil {
			t.Fatal(err)
		}
		if len(message) > 4096 {
			t.Fatalf("a frame of %d bytes, over 4096", len(message))
		}
		var frame map[string]any
		_ = json.Unmarshal(message, &frame)
		if frame["op"] != "chunk" {
			t.Fatalf("message %d is not a chunk: %v", seq, frame["op"])
		}
		if transfer == nil {
			transfer = frame["transfer"]
		}
		if frame["transfer"] != transfer || int(frame["seq"].(float64)) != seq {
			t.Fatalf("chunk %d of another transfer or out of order: %v", seq, frame)
		}
		last, _ := frame["last"].(bool)
		part, err := base64.StdEncoding.DecodeString(frame["data"].(string))
		if err != nil {
			t.Fatal(err)
		}
		transferred = append(transferred, part...)
		if last {
			break
		}
		seq++
	}
	var call map[string]any
	if err := json.Unmarshal(transferred, &call); err != nil {
		t.Fatalf("the chunks do not join a frame: %v", err)
	}
	if call["op"] != "invoke" || call["method"] != "take" {
		t.Fatalf("%v", call)
	}
	if err := far.Send([]byte(fmt.Sprintf(`{"op":"return","id":%q,"value":{"type":"data","value":"ok"}}`, call["id"]))); err != nil {
		t.Fatal(err)
	}
	got := testwait.Within(t, "the result", done)
	if got.err != nil || text(got.value) != "ok" {
		t.Fatalf("%v %v", got.value, got.err)
	}
}

func TestAResultOverTheMessageLimitIsSentAsChunksToo(t *testing.T) {
	big := strings.Repeat("y", 8192)
	var calls atomic.Int32
	runtime, far := chunkHostWith(t, `["chunked"]`, 4096, 0, func(ctx context.Context, target, method string, args []any) (any, error) {
		calls.Add(1)
		return big, nil
	})
	_ = runtime
	if err := far.Send([]byte(`{"op":"invoke","id":"main:1","path":[],"target":"test","method":"big","args":{"type":"data","value":[]}}`)); err != nil {
		t.Fatal(err)
	}
	var transferred []byte
	seq := 0
	for {
		message, err := far.Recv()
		if err != nil {
			t.Fatal(err)
		}
		if len(message) > 4096 {
			t.Fatalf("a frame of %d bytes, over 4096", len(message))
		}
		var frame map[string]any
		_ = json.Unmarshal(message, &frame)
		if frame["op"] != "chunk" {
			t.Fatalf("message %d is not a chunk: %v", seq, frame["op"])
		}
		part, err := base64.StdEncoding.DecodeString(frame["data"].(string))
		if err != nil {
			t.Fatal(err)
		}
		transferred = append(transferred, part...)
		last, _ := frame["last"].(bool)
		if last {
			break
		}
		seq++
	}
	var reply map[string]any
	if err := json.Unmarshal(transferred, &reply); err != nil {
		t.Fatalf("the chunks do not join a frame: %v", err)
	}
	if reply["op"] != "return" || !strings.Contains(string(transferred), big) {
		t.Fatalf("%v", reply)
	}
	if calls.Load() != 1 {
		t.Fatalf("the call ran %d times", calls.Load())
	}
}

func TestAFarEndWithoutChunkedStillGetsTheOverLimitError(t *testing.T) {
	runtime, _ := chunkHost(t, `[]`, 1024, 0)
	_, err := runtime.Call(context.Background(), "host:svc", "take", []any{strings.Repeat("x", 8192)}, true)
	var refused *ValueError
	if err == nil || !errors.As(err, &refused) || !strings.HasSuffix(err.Error(), "over the limit of 1024 bytes") {
		t.Fatalf("%v", err)
	}
	if data, _ := json.Marshal(refused.Data()); string(data) != `{"argument":1,"limit":1024,"part":"argument"}` {
		t.Fatalf("%s", data)
	}
	select {
	case <-runtime.Closed():
		t.Fatal("the session ended")
	default:
	}
}

func TestAFrameOverTheReassemblyLimitFailsOnlyItsCall(t *testing.T) {
	runtime, _ := chunkHost(t, `["chunked"]`, 4096, 2048)
	_, err := runtime.Call(context.Background(), "host:svc", "take", []any{strings.Repeat("x", 8192)}, true)
	var refused *ValueError
	if err == nil || !errors.As(err, &refused) || !strings.HasSuffix(err.Error(), "over the limit of 2048 bytes") {
		t.Fatalf("%v", err)
	}
	if data, _ := json.Marshal(refused.Data()); string(data) != `{"argument":1,"limit":2048,"part":"argument"}` {
		t.Fatalf("%s", data)
	}
}

func TestMaxTransferIsReadFromTheEnvironment(t *testing.T) {
	x, y := net.Pipe()
	defer x.Close()
	defer y.Close()
	t.Setenv("RUTIS_MAX_TRANSFER", "1024")
	p, err := New(NewLines(x), Options{Endpoint: &Endpoint{Local: "go"}})
	if err != nil {
		t.Fatal(err)
	}
	if p.maxTransfer != 1024 {
		t.Fatalf("%d", p.maxTransfer)
	}
	t.Setenv("RUTIS_MAX_TRANSFER", "abc")
	if _, err := New(NewLines(x), Options{Endpoint: &Endpoint{Local: "go"}}); err == nil || !strings.Contains(err.Error(), `RUTIS_MAX_TRANSFER "abc": expected a positive number of bytes`) {
		t.Fatalf("%v", err)
	}
}

func TestChunkedSidesCarryAValueOverTheMessageLimitWhole(t *testing.T) {
	if testing.Short() {
		t.Skip("17 MiB")
	}
	var calls atomic.Int32
	host, runtime := pairWith(t, func(ctx context.Context, target, method string, args []any) (any, error) {
		calls.Add(1)
		return arg(args, 0), nil
	}, nil, TestCapabilities, TestCapabilities)
	big := strings.Repeat("x", 17*1024*1024)
	value, err := runtime.Call(context.Background(), "host:svc", "echo", []any{big}, true)
	if err != nil {
		t.Fatal(err)
	}
	if text(value) != big {
		t.Fatalf("the value came back %d bytes", len(text(value)))
	}
	if calls.Load() != 1 {
		t.Fatalf("the call ran %d times", calls.Load())
	}
	_ = host
}
