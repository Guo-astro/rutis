package rutis

import (
	"io"
	"net"
	"sync"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// recordConn keeps what each Write was given: one frame per write, as a
// connection built by hand needs it.
type recordConn struct {
	mu     sync.Mutex
	writes [][]byte
	closed bool
}

func (c *recordConn) Write(p []byte) (int, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return 0, net.ErrClosed
	}
	c.writes = append(c.writes, append([]byte(nil), p...))
	return len(p), nil
}

func (c *recordConn) Read([]byte) (int, error) { return 0, io.EOF }

func (c *recordConn) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.closed = true
	return nil
}

func (c *recordConn) frames() [][]byte {
	c.mu.Lock()
	defer c.mu.Unlock()
	return append([][]byte(nil), c.writes...)
}

type recordAddr struct{}

func (recordAddr) Network() string { return "test" }
func (recordAddr) String() string  { return "test" }

func (c *recordConn) LocalAddr() net.Addr              { return recordAddr{} }
func (c *recordConn) RemoteAddr() net.Addr             { return recordAddr{} }
func (c *recordConn) SetDeadline(time.Time) error      { return nil }
func (c *recordConn) SetReadDeadline(time.Time) error  { return nil }
func (c *recordConn) SetWriteDeadline(time.Time) error { return nil }

// A Send that passed its check before this side's close frame waits for the
// writer; the close frame must be the last frame written (#630): no text frame
// follows it.
func TestNoFrameIsWrittenAfterTheCloseFrame(t *testing.T) {
	wire := &recordConn{}
	ws := &wsConn{conn: wire, done: make(chan struct{})}
	inside := make(chan struct{})
	release := make(chan struct{})
	ws.sendGate = func() {
		close(inside)
		<-release
	}
	sent := make(chan error, 1)
	go func() { sent <- ws.Send([]byte("late")) }()
	testwait.Within(t, "Send between its check and its write", inside)
	// The close frame goes out while that Send is between the two.
	ws.closeWith(1000, "bye")
	close(release)
	if err := testwait.Within(t, "the Send that lost the race", sent); err != errClosed {
		t.Fatalf("Send after the close frame: %v, want %v", err, errClosed)
	}
	frames := wire.frames()
	if len(frames) != 1 {
		t.Fatalf("wrote %d frames after the close frame was sent: %q", len(frames), frames)
	}
	if opcode := frames[0][0] & 0x0F; opcode != 0x8 {
		t.Fatalf("the frame written is %d, not a close frame", opcode)
	}
}
