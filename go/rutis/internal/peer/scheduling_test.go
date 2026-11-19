package peer

import (
	"context"
	"encoding/json"
	"io"
	"sync"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// gateChannel is a channel the test drives: Recv returns the frame the test
// fed, and Send records what the session writes. The test hands over every
// incoming frame itself, so it decides when a call is answered (#630, "Go:
// explicit gates"). No test below waits on a timer to learn that something
// did or did not happen.
type gateChannel struct {
	incoming chan []byte
	written  chan []byte
	once     sync.Once
}

func newGateChannel() *gateChannel {
	return &gateChannel{incoming: make(chan []byte, 8), written: make(chan []byte, 8)}
}

// feed hands `message` to the session's reader, as the far end would.
func (c *gateChannel) feed(t *testing.T, message string) {
	t.Helper()
	c.incoming <- []byte(message)
}

func (c *gateChannel) Recv() ([]byte, error) {
	message, ok := <-c.incoming
	if !ok {
		return nil, io.EOF
	}
	return message, nil
}

func (c *gateChannel) Send(message []byte) error {
	c.written <- append([]byte(nil), message...)
	return nil
}

func (c *gateChannel) Close(string) {
	c.once.Do(func() { close(c.incoming) })
}

// openSession starts a session on a scripted channel and completes the
// handshake; `written` is left empty for the test.
func openSession(t *testing.T, opts Options) (*Peer, *gateChannel) {
	t.Helper()
	channel := newGateChannel()
	opts.Endpoint = &Endpoint{Local: "go", Expected: "main"}
	opts.Capabilities = TestCapabilities
	session, err := New(channel, opts)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		session.Close(nil)
		channel.Close("")
	})
	if err := session.Start(); err != nil {
		t.Fatal(err)
	}
	greeting := testwait.Within(t, "the greeting", channel.written)
	var hello map[string]any
	_ = json.Unmarshal(greeting, &hello)
	if hello["op"] != "hello" {
		t.Fatalf("the first frame is not the greeting: %s", greeting)
	}
	channel.feed(t, `{"op":"hello","version":5,"endpoint":"main","capabilities":["reentrant-sync"]}`)
	testwait.Within(t, "the handshake", session.Ready())
	return session, channel
}

// rule: REM-6.2-3 (a call's identity is written with the call).
// A synchronous call returns only when the test hands over its reply: the
// wait is the channel's, not the scheduler's.
func TestASynchronousCallReturnsOnlyWhenItsReplyIsHandedOver(t *testing.T) {
	session, channel := openSession(t, Options{
		Dispatch: func(context.Context, string, string, []any) (any, error) { return nil, nil },
	})
	type outcome struct {
		value any
		err   error
	}
	done := make(chan outcome, 1)
	go func() {
		value, err := session.Call(context.Background(), "x", "m", nil, true)
		done <- outcome{value, err}
	}()
	frame := testwait.Within(t, "the call frame", channel.written)
	var call map[string]any
	if err := json.Unmarshal(frame, &call); err != nil {
		t.Fatal(err)
	}
	if call["op"] != "invoke" || call["id"] != "go:1" || call["sync"] != true {
		t.Fatalf("the call frame: %s", frame)
	}
	// No reply was handed over: the call cannot have returned.
	select {
	case got := <-done:
		t.Fatalf("the call returned without its reply: %+v", got)
	default:
	}
	channel.feed(t, `{"op":"return","id":"go:1","value":{"type":"data","value":"answer"}}`)
	got := testwait.Within(t, "the call to return", done)
	if got.err != nil || text(got.value) != "answer" {
		t.Fatalf("the call returned %v, %v", got.value, got.err)
	}
}

// rule: SES-10-4 (reading a frame and running plugin code are separate).
// A control call runs on the reading goroutine before the next frame is
// read: while the gate holds it, the next frame's method cannot start.
func TestAControlFrameHoldsTheReaderBeforeTheNextFrame(t *testing.T) {
	arrived := make(chan string, 1)
	release := make(chan struct{})
	var mu sync.Mutex
	var dispatched []string
	_, channel := openSession(t, Options{
		Arrive: func(_ context.Context, method string, _ []any) func() (any, error) {
			arrived <- method
			<-release
			return nil // Dispatch serves the call.
		},
		Dispatch: func(_ context.Context, _ string, method string, _ []any) (any, error) {
			mu.Lock()
			dispatched = append(dispatched, method)
			mu.Unlock()
			return "ok", nil
		},
	})
	channel.feed(t, `{"op":"invoke","id":"main:1","path":[],"target":"","method":"events.on","args":{"type":"list","value":[]}}`)
	channel.feed(t, `{"op":"invoke","id":"main:2","path":[],"target":"x","method":"next","args":{"type":"list","value":[]}}`)
	if got := testwait.Within(t, "the control call", arrived); got != "events.on" {
		t.Fatalf("the control call: %s", got)
	}
	// The gate is held: the reader is inside Arrive and cannot have read the
	// frame behind it.
	mu.Lock()
	ran := append([]string(nil), dispatched...)
	mu.Unlock()
	if len(ran) != 0 {
		t.Fatalf("a frame was read while the control call held the reader: %v", ran)
	}
	close(release)
	// The rest of each call runs on a goroutine of its own, so the two
	// answers may come in either order; both must come, once.
	answered := map[string]string{}
	for range 2 {
		frame := testwait.Within(t, "an answer", channel.written)
		var reply map[string]any
		if err := json.Unmarshal(frame, &reply); err != nil {
			t.Fatal(err)
		}
		if reply["op"] != "return" {
			t.Fatalf("not a reply: %s", frame)
		}
		answered[reply["id"].(string)] = reply["op"].(string)
	}
	if len(answered) != 2 || answered["main:1"] != "return" || answered["main:2"] != "return" {
		t.Fatalf("the answers: %v", answered)
	}
	mu.Lock()
	ran = append([]string(nil), dispatched...)
	mu.Unlock()
	if len(ran) != 2 {
		t.Fatalf("dispatched: %v", ran)
	}
}
