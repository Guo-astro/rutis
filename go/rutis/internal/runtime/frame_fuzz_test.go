package rutis

// The WebSocket channel's hand-written RFC 6455 frame parser (#631): every
// byte string is read without a panic, and what it accepts stays within the
// limits the channel promises. Seeds cover each frame shape and the
// truncated forms of it; the parser is exercised as the accepting side
// (client frames must be masked) and as the dialing side (server frames must
// not be).

import (
	"bufio"
	"bytes"
	"encoding/binary"
	"io"
	"net"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// fuzzPayloadLimit is the largest payload an input is allowed to claim when
// the input is shorter than that: `readFrame` allocates the claimed length
// before `io.ReadFull` finds the input short, and a large allocation per
// input would only slow the fuzzer down. A claim past `peer.MaxMessage` is
// still read: it is refused before anything is allocated.
const fuzzPayloadLimit = 64 * 1024

// fuzzConn accepts every write: the close and pong frames a read may write
// go nowhere.
type fuzzConn struct{}

func (fuzzConn) Read([]byte) (int, error)         { return 0, io.EOF }
func (fuzzConn) Write(p []byte) (int, error)      { return len(p), nil }
func (fuzzConn) Close() error                     { return nil }
func (fuzzConn) LocalAddr() net.Addr              { return fuzzAddr{} }
func (fuzzConn) RemoteAddr() net.Addr             { return fuzzAddr{} }
func (fuzzConn) SetDeadline(time.Time) error      { return nil }
func (fuzzConn) SetReadDeadline(time.Time) error  { return nil }
func (fuzzConn) SetWriteDeadline(time.Time) error { return nil }

type fuzzAddr struct{}

func (fuzzAddr) Network() string { return "fuzz" }
func (fuzzAddr) String() string  { return "fuzz" }

// fuzzWSConn is a connection reading `data`, with its close path already
// taken: a close frame would otherwise write to fuzzConn and start a timer
// goroutine for every input that fails to parse.
func fuzzWSConn(data []byte, client bool) *wsConn {
	ws := &wsConn{
		conn:   fuzzConn{},
		reader: bufio.NewReader(bytes.NewReader(data)),
		client: client,
		done:   make(chan struct{}),
	}
	ws.once.Do(func() {})
	return ws
}

// fuzzFrame builds one frame, as a test writes it by hand; lengthForm 2 or 8
// forces the extended length encoding even when the payload is shorter.
func fuzzFrame(fin bool, opcode byte, masked bool, payload []byte, lengthForm int) []byte {
	first := opcode
	if fin {
		first |= 0x80
	}
	mask := byte(0)
	if masked {
		mask = 0x80
	}
	frame := []byte{first}
	switch n := len(payload); {
	case lengthForm == 8 || n > 0xffff:
		frame = append(frame, mask|127)
		frame = binary.BigEndian.AppendUint64(frame, uint64(n))
	case lengthForm == 2 || n >= 126:
		frame = append(frame, mask|126, byte(n>>8), byte(n))
	default:
		frame = append(frame, mask|byte(n))
	}
	if !masked {
		return append(frame, payload...)
	}
	key := [4]byte{0x12, 0x34, 0x56, 0x78}
	frame = append(frame, key[:]...)
	for i, b := range payload {
		frame = append(frame, b^key[i%4])
	}
	return frame
}

// fuzzDeclaredLength is the payload length a frame header claims, when the
// header is long enough to say.
func fuzzDeclaredLength(data []byte) (uint64, bool) {
	if len(data) < 2 {
		return 0, false
	}
	switch data[1] & 0x7f {
	case 126:
		if len(data) < 4 {
			return 0, false
		}
		return uint64(binary.BigEndian.Uint16(data[2:4])), true
	case 127:
		if len(data) < 10 {
			return 0, false
		}
		return binary.BigEndian.Uint64(data[2:10]), true
	default:
		return uint64(data[1] & 0x7f), true
	}
}

func FuzzFrameParsing(f *testing.F) {
	f.Add(fuzzFrame(true, 0x1, true, []byte("hello"), 0))
	f.Add(fuzzFrame(true, 0x1, false, []byte("hello"), 0))
	f.Add(fuzzFrame(true, 0x1, true, nil, 0))
	f.Add(fuzzFrame(true, 0x1, true, []byte("hi"), 2))
	f.Add(fuzzFrame(true, 0x1, true, []byte("hi"), 8))
	f.Add(fuzzFrame(true, 0x8, true, []byte{0x03, 0xe8}, 0))
	f.Add(fuzzFrame(true, 0x9, true, []byte("ping"), 0))
	f.Add(fuzzFrame(true, 0xa, true, nil, 0))
	f.Add(fuzzFrame(true, 0x2, true, []byte{0, 1, 2}, 0))
	f.Add(fuzzFrame(false, 0x0, true, []byte("fragment"), 0))
	f.Add(append(fuzzFrame(false, 0x1, true, []byte("hel"), 0), fuzzFrame(true, 0x0, true, []byte("lo"), 0)...))
	f.Add(append(fuzzFrame(false, 0x1, true, []byte("a"), 0), fuzzFrame(true, 0x9, true, []byte("p"), 0)...))
	f.Add(fuzzFrame(true, 0x3, true, nil, 0))
	f.Add(fuzzFrame(true, 0x1, true, []byte{0xff, 0xfe}, 0))
	f.Add([]byte{0x81})
	f.Add([]byte{0x81, 0xfe, 0x00})
	f.Add([]byte{0x81, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05})
	f.Add([]byte{0x81, 0xff, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x01})
	f.Add([]byte{0x81, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff})
	f.Add([]byte{})

	f.Fuzz(func(t *testing.T, data []byte) {
		if len(data) > 1<<20 {
			t.Skip()
		}
		if length, ok := fuzzDeclaredLength(data); ok && length > fuzzPayloadLimit && length <= uint64(peer.MaxMessage) {
			t.Skip()
		}
		for _, client := range []bool{false, true} {
			ws := fuzzWSConn(data, client)
			_, _, payload, err := ws.readFrame()
			if err == nil && len(payload) > peer.MaxMessage {
				t.Fatalf("readFrame accepted a payload of %d bytes", len(payload))
			}
			ws = fuzzWSConn(data, client)
			message, err := ws.Recv()
			if err != nil {
				continue
			}
			if len(message) > peer.MaxMessage {
				t.Fatalf("Recv accepted a message of %d bytes", len(message))
			}
			if !utf8.Valid(message) {
				t.Fatalf("Recv accepted invalid UTF-8: %x", message)
			}
		}
	})
}
