package rutis

import (
	"bufio"
	"io"
	"net"
	"sync"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// gateWire is a connection a test feeds frames to and reads writes from.
// Close does not end the incoming side: a frame can be fed after
// closeWith without racing the delayed conn.Close it schedules.
type gateWire struct {
	in     *io.PipeReader
	mu     sync.Mutex
	writes [][]byte
}

func (c *gateWire) Read(p []byte) (int, error) { return c.in.Read(p) }

func (c *gateWire) Write(p []byte) (int, error) {
	c.mu.Lock()
	c.writes = append(c.writes, append([]byte(nil), p...))
	c.mu.Unlock()
	return len(p), nil
}

func (c *gateWire) Close() error                     { return nil }
func (c *gateWire) LocalAddr() net.Addr              { return nil }
func (c *gateWire) RemoteAddr() net.Addr             { return nil }
func (c *gateWire) SetDeadline(time.Time) error      { return nil }
func (c *gateWire) SetReadDeadline(time.Time) error  { return nil }
func (c *gateWire) SetWriteDeadline(time.Time) error { return nil }

func (c *gateWire) frames() [][]byte {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([][]byte(nil), c.writes...)
}

// clientFrame frames one masked frame, as a client sends it.
func clientFrame(opcode byte, payload []byte) []byte {
	mask := [4]byte{1, 2, 3, 4}
	frame := []byte{0x80 | opcode, 0x80 | byte(len(payload))}
	frame = append(frame, mask[:]...)
	for i, b := range payload {
		frame = append(frame, b^mask[i%4])
	}
	return frame
}

// #630: the close frame is the last frame written (RFC 6455 §5.5.1).
// A ping read after this side's close frame starts no pong. The close frame
// goes out first; the ping is fed afterwards, so the order is the test's,
// not the scheduler's.
func TestAPingReadAfterTheCloseFrameIsNotAnswered(t *testing.T) {
	// The fix is in #1191; remove this skip when it merges.
	t.Skip("blocked on #1191: the fix (no frame after the close frame) is not in main yet")
	in, out := io.Pipe()
	wire := &gateWire{in: in}
	ws := &wsConn{conn: wire, reader: bufio.NewReader(in), done: make(chan struct{})}
	left := make(chan error, 1)
	go func() {
		_, err := ws.Recv()
		left <- err
	}()
	ws.closeWith(1000, "bye")
	if _, err := out.Write(clientFrame(0x9, []byte("late"))); err != nil {
		t.Fatal(err)
	}
	_ = out.Close()
	testwait.Within(t, "the reader to stop", left)
	frames := wire.frames()
	if len(frames) != 1 {
		t.Fatalf("%d frames written: %q", len(frames), frames)
	}
	if opcode := frames[0][0] & 0x0F; opcode != 0x8 {
		t.Fatalf("the frame written is %d, not the close frame", opcode)
	}
}

// #630: the close frame is the last frame written (RFC 6455 §5.5.1).
// A Send that passed its open check just before the close frame reaches
// writeFrame after it; that text frame must not be written.
func TestATextFrameAfterTheCloseFrameIsNotWritten(t *testing.T) {
	// The fix is in #1191; remove this skip when it merges.
	t.Skip("blocked on #1191: the fix (no frame after the close frame) is not in main yet")
	in, _ := io.Pipe()
	wire := &gateWire{in: in}
	ws := &wsConn{conn: wire, done: make(chan struct{})}
	ws.closeWith(1000, "bye")
	if err := ws.writeFrame(0x1, []byte("late")); err == nil {
		t.Fatal("a text frame was accepted after the close frame")
	}
	frames := wire.frames()
	if len(frames) != 1 {
		t.Fatalf("%d frames written: %q", len(frames), frames)
	}
	if opcode := frames[0][0] & 0x0F; opcode != 0x8 {
		t.Fatalf("the frame written is %d, not the close frame", opcode)
	}
}
