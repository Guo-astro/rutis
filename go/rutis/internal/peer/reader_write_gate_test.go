package peer

import (
	"context"
	"encoding/json"
	"io"
	"sync"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// blockedChannel is a channel whose writes a test can hold. While a write
// is held, it holds the peer's writeMu, as a far end that stopped reading
// does under backpressure.
type blockedChannel struct {
	incoming chan []byte
	written  chan []byte

	mu      sync.Mutex
	holding bool
	blocked chan struct{}
	release chan struct{}
	signal  sync.Once

	closeOnce sync.Once
}

func newBlockedChannel() *blockedChannel {
	return &blockedChannel{
		incoming: make(chan []byte, 8),
		written:  make(chan []byte, 32),
	}
}

// holdWrites makes the next Send wait in the channel.
func (c *blockedChannel) holdWrites() {
	c.mu.Lock()
	c.holding = true
	c.blocked = make(chan struct{})
	c.release = make(chan struct{})
	c.signal = sync.Once{}
	c.mu.Unlock()
}

// releaseWrites lets the held Send (and later ones) through.
func (c *blockedChannel) releaseWrites() {
	c.mu.Lock()
	c.holding = false
	release := c.release
	c.release = nil
	c.mu.Unlock()
	if release != nil {
		close(release)
	}
}

func (c *blockedChannel) Send(message []byte) error {
	c.mu.Lock()
	holding, blocked, release := c.holding, c.blocked, c.release
	c.mu.Unlock()
	if holding {
		c.signal.Do(func() { close(blocked) })
		<-release
	}
	c.written <- append([]byte(nil), message...)
	return nil
}

func (c *blockedChannel) Recv() ([]byte, error) {
	message, ok := <-c.incoming
	if !ok {
		return nil, io.EOF
	}
	return message, nil
}

func (c *blockedChannel) Close(string) {
	c.closeOnce.Do(func() { close(c.incoming) })
}

func (c *blockedChannel) feed(t *testing.T, message string) {
	t.Helper()
	c.incoming <- []byte(message)
}

// openBlockedSession starts a session declaring no value capabilities, so
// a tagged value in a reply is refused, and reports each control call that
// arrives on `arrived`.
func openBlockedSession(t *testing.T) (*Peer, *blockedChannel, chan string) {
	t.Helper()
	channel := newBlockedChannel()
	arrived := make(chan string, 1)
	session, err := New(channel, Options{
		Endpoint:     &Endpoint{Local: "go", Expected: "main"},
		Capabilities: []string{"reentrant-sync"},
		Arrive: func(_ context.Context, method string, _ []any) func() (any, error) {
			arrived <- method
			return func() (any, error) { return nil, nil }
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		channel.releaseWrites()
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
	channel.feed(t, `{"op":"hello","version":4,"endpoint":"main","capabilities":["reentrant-sync"]}`)
	testwait.Within(t, "the handshake", session.Ready())
	return session, channel, arrived
}

// refusedReply is a return carrying a reference and an instant: the
// instant is a value this side did not declare, so the call fails and the
// reference it carried goes back.
const refusedReply = `{"op":"return","id":"go:1","value":{"type":"list","value":[` +
	`{"type":"instant","value":"2026-10-15T03:22:15+00:00"},` +
	`{"type":"reference","value":{"id":7,"kind":"function","home":false,"origin":[]}}]}}`

// rule: SESS-47 (the references a refused reply carried go back), #630.
// A reply this side refuses releases the reference it carried before the
// reader goes on; the release is one frame, written by releaseAll.
func TestARefusedReplyReleasesItsReference(t *testing.T) {
	session, channel, arrived := openBlockedSession(t)
	wait, err := session.Send(context.Background(), "x", "m", nil, false)
	if err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the call frame", channel.written)
	channel.feed(t, refusedReply)
	type outcome struct {
		value any
		err   error
	}
	got := testwait.Do(t, "the refused call", func() outcome {
		value, err := wait()
		return outcome{value, err}
	})
	if got.err == nil {
		t.Fatal("the call did not fail on the value it could not take")
	}
	frame := testwait.Within(t, "the release frame", channel.written)
	var release map[string]any
	if err := json.Unmarshal(frame, &release); err != nil {
		t.Fatal(err)
	}
	if release["op"] != "release" || release["reference"] != float64(7) || release["count"] != float64(1) {
		t.Fatalf("the frame after the refused reply: %s", frame)
	}
	// The reader goes on: the release did not end it.
	channel.feed(t, `{"op":"invoke","id":"main:9","path":[],"target":"","method":"probe","args":{"type":"list","value":[]}}`)
	if got := testwait.Within(t, "the frame after the release", arrived); got != "probe" {
		t.Fatalf("the control call: %q", got)
	}
}

// rule: SESS-47, SES-10-4, specs/call-lifecycle (the reader write-back),
// #630, #1210. A refused reply's release is written by the session's writer,
// not by the reader: while another frame's write is held, that write holds
// writeMu, and the reader goes on reading the frames behind it.
func TestTheReaderDoesNotWaitForAReleaseWrite(t *testing.T) {
	session, channel, arrived := openBlockedSession(t)
	wait, err := session.Send(context.Background(), "x", "m", nil, false)
	if err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the first call frame", channel.written)

	// The next call's frame waits in the channel: its goroutine holds
	// writeMu while the far end does not read.
	channel.holdWrites()
	sent := make(chan struct{})
	go func() {
		defer close(sent)
		_, _ = session.Send(context.Background(), "x", "m2", nil, false)
	}()
	testwait.Within(t, "the held write", channel.blocked)

	channel.feed(t, refusedReply)
	channel.feed(t, `{"op":"invoke","id":"main:9","path":[],"target":"","method":"probe","args":{"type":"list","value":[]}}`)
	// The frame behind the release must be read while the write is held:
	// the reader hands its write-back to a writer, it does not wait here.
	if got := testwait.Within(t, "the frame behind the release", arrived); got != "probe" {
		t.Fatalf("the control call: %q", got)
	}
	channel.releaseWrites()
	_ = wait
	<-sent
}
