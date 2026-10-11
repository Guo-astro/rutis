package rutis

import (
	"bufio"
	"crypto/sha1"
	"encoding/base64"
	"encoding/binary"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

const testProtocol = "rutis.3"

func startListener(t *testing.T) (<-chan *wsConn, string) {
	t.Helper()
	var announced string
	previous := announce
	announce = func(address string) { announced = address }
	defer func() { announce = previous }()
	accepted, address, err := listenWebSocket("ws://127.0.0.1:0/rutis", testProtocol, "secret", "", "")
	if err != nil {
		t.Fatal(err)
	}
	if announced != address || !strings.HasPrefix(address, "ws://127.0.0.1:") {
		t.Fatalf("announced %q, bound %q", announced, address)
	}
	return accepted, address
}

func TestARefusedConnectionSaysWhy(t *testing.T) {
	_, address := startListener(t)
	base := "http" + strings.TrimPrefix(address, "ws")
	for _, c := range []struct {
		path, auth, protocol string
		status               int
	}{
		{"/other", "Bearer secret", testProtocol, http.StatusNotFound},
		{"/rutis", "", testProtocol, http.StatusUnauthorized},
		{"/rutis", "Bearer wrong", testProtocol, http.StatusForbidden},
		{"/rutis", "Bearer secret", "", http.StatusBadRequest},
	} {
		u, _ := url.Parse(base)
		u.Path = c.path
		request, _ := http.NewRequest("GET", u.String(), nil)
		if c.auth != "" {
			request.Header.Set("Authorization", c.auth)
		}
		if c.protocol != "" {
			request.Header.Set("Sec-WebSocket-Protocol", c.protocol)
		}
		response, err := http.DefaultClient.Do(request)
		if err != nil {
			t.Fatal(err)
		}
		response.Body.Close()
		if response.StatusCode != c.status {
			t.Errorf("%+v: got %d", c, response.StatusCode)
		}
	}
}

func TestOnlyALoopbackListenerGoesWithoutTLS(t *testing.T) {
	if _, _, err := listenWebSocket("ws://0.0.0.0:0/rutis", testProtocol, "t", "", ""); err == nil || !strings.Contains(err.Error(), "only a loopback listener") {
		t.Fatalf("%v", err)
	}
	if _, _, err := listenWebSocket("wss://127.0.0.1:0/rutis", testProtocol, "t", "", ""); err == nil || !strings.Contains(err.Error(), "certificate and key") {
		t.Fatalf("%v", err)
	}
}

// client is the controller's end of a connection, framed by hand.
type client struct {
	t      *testing.T
	conn   net.Conn
	reader *bufio.Reader
}

func dial(t *testing.T, address string) *client {
	t.Helper()
	u, _ := url.Parse(address)
	conn, err := net.Dial("tcp", u.Host)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { conn.Close() })
	key := "dGhlIHNhbXBsZSBub25jZQ=="
	fmt.Fprintf(conn, "GET %s HTTP/1.1\r\nHost: %s\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"+
		"Sec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: %s\r\n"+
		"Authorization: Bearer secret\r\n\r\n", u.Path, u.Host, key, testProtocol)
	reader := bufio.NewReader(conn)
	response, err := http.ReadResponse(reader, nil)
	if err != nil {
		t.Fatal(err)
	}
	sum := sha1.Sum([]byte(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))
	if response.StatusCode != http.StatusSwitchingProtocols ||
		response.Header.Get("Sec-WebSocket-Accept") != base64.StdEncoding.EncodeToString(sum[:]) ||
		response.Header.Get("Sec-WebSocket-Protocol") != testProtocol {
		t.Fatalf("handshake: %d %v", response.StatusCode, response.Header)
	}
	return &client{t: t, conn: conn, reader: reader}
}

func (c *client) send(fin bool, opcode byte, payload []byte, masked bool) {
	head := byte(opcode)
	if fin {
		head |= 0x80
	}
	frame := []byte{head}
	maskBit := byte(0)
	if masked {
		maskBit = 0x80
	}
	switch n := len(payload); {
	case n < 126:
		frame = append(frame, maskBit|byte(n))
	case n <= 0xFFFF:
		frame = append(frame, maskBit|126, byte(n>>8), byte(n))
	default:
		frame = append(frame, maskBit|127)
		frame = binary.BigEndian.AppendUint64(frame, uint64(n))
	}
	body := append([]byte{}, payload...)
	if masked {
		mask := []byte{1, 2, 3, 4}
		frame = append(frame, mask...)
		for i := range body {
			body[i] ^= mask[i%4]
		}
	}
	if _, err := c.conn.Write(append(frame, body...)); err != nil {
		c.t.Fatal(err)
	}
}

// next reads the next frame the listener sends.
func (c *client) next() (byte, []byte) {
	_ = c.conn.SetReadDeadline(time.Now().Add(testwait.HangGuard))
	var head [2]byte
	if _, err := io.ReadFull(c.reader, head[:]); err != nil {
		c.t.Fatal(err)
	}
	length := uint64(head[1] & 0x7F)
	switch length {
	case 126:
		var n [2]byte
		_, _ = io.ReadFull(c.reader, n[:])
		length = uint64(binary.BigEndian.Uint16(n[:]))
	case 127:
		var n [8]byte
		_, _ = io.ReadFull(c.reader, n[:])
		length = binary.BigEndian.Uint64(n[:])
	}
	payload := make([]byte, length)
	if _, err := io.ReadFull(c.reader, payload); err != nil {
		c.t.Fatal(err)
	}
	return head[0] & 0x0F, payload
}

// closed waits for the listener's close frame and returns its code.
func (c *client) closed() int {
	for {
		opcode, payload := c.next()
		if opcode == 0x8 {
			return int(binary.BigEndian.Uint16(payload))
		}
	}
}

func accept(t *testing.T, accepted <-chan *wsConn) *wsConn {
	t.Helper()
	return testwait.Within(t, "the accepted connection", accepted)
}

type received struct {
	message []byte
	err     error
}

// recv is conn.Recv() within the hang guard.
func recv(t *testing.T, conn *wsConn) ([]byte, error) {
	t.Helper()
	got := testwait.Do(t, "a message", func() received {
		message, err := conn.Recv()
		return received{message, err}
	})
	return got.message, got.err
}

func TestMessagesCrossInBothDirections(t *testing.T) {
	accepted, address := startListener(t)
	c := dial(t, address)
	conn := accept(t, accepted)
	// A message in two fragments.
	c.send(false, 0x1, []byte("hel"), true)
	c.send(true, 0x0, []byte("lo"), true)
	message, err := recv(t, conn)
	if err != nil || string(message) != "hello" {
		t.Fatalf("%q %v", message, err)
	}
	if err := conn.Send([]byte("hi")); err != nil {
		t.Fatal(err)
	}
	if opcode, payload := c.next(); opcode != 0x1 || string(payload) != "hi" {
		t.Fatalf("%d %q", opcode, payload)
	}
	// A ping is answered while the listener reads.
	c.send(true, 0x9, []byte("are you there"), true)
	c.send(true, 0x1, []byte("after"), true)
	if message, err := recv(t, conn); err != nil || string(message) != "after" {
		t.Fatalf("%q %v", message, err)
	}
	if opcode, payload := c.next(); opcode != 0xA || string(payload) != "are you there" {
		t.Fatalf("%d %q", opcode, payload)
	}
}

func TestBrokenFramesCloseWithTheirCode(t *testing.T) {
	accepted, address := startListener(t)
	for _, c := range []struct {
		name string
		send func(*client)
		code int
	}{
		{"unmasked", func(c *client) { c.send(true, 0x1, []byte("x"), false) }, 1002},
		{"binary", func(c *client) { c.send(true, 0x2, []byte("x"), true) }, unsupported},
		{"too big", func(c *client) {
			// The header alone announces more than the limit.
			frame := []byte{0x81, 0x80 | 127}
			frame = binary.BigEndian.AppendUint64(frame, uint64(peer.MaxMessage+1))
			_, _ = c.conn.Write(append(frame, 1, 2, 3, 4))
		}, tooBig},
	} {
		client := dial(t, address)
		conn := accept(t, accepted)
		c.send(client)
		if _, err := recv(t, conn); err == nil {
			t.Errorf("%s: the message should be refused", c.name)
		}
		if code := client.closed(); code != c.code {
			t.Errorf("%s: closed with %d, not %d", c.name, code, c.code)
		}
	}
}

// Timing is what this tests: by tolerance
// (docs/design-test-clocks-2026-10-11.md §3.3), ping and timeout ten times
// apart, so that a stalled machine does not drop the controller before the
// first ping; no bound on how fast it is dropped (only a hang guard).
func TestASilentControllerIsDropped(t *testing.T) {
	t.Setenv("RUTIS_HEARTBEAT", "100,1000")
	accepted, address := startListener(t)
	c := dial(t, address)
	accept(t, accepted)
	pinged := false
	// Pings keep coming until it is dropped: bound the whole wait.
	deadline := time.Now().Add(testwait.HangGuard)
	for {
		if time.Now().After(deadline) {
			t.Fatalf("timed out (%v): the silent controller to be dropped", testwait.HangGuard)
		}
		opcode, payload := c.next()
		if opcode == 0x9 {
			pinged = true
			continue
		}
		if opcode == 0x8 {
			if code := int(binary.BigEndian.Uint16(payload)); code != goingAway || !pinged {
				t.Fatalf("closed with %d, pinged %v", code, pinged)
			}
			return
		}
	}
}

func TestANewerControllerTakesOver(t *testing.T) {
	addresses := make(chan string, 1)
	previous := announce
	announce = func(address string) { addresses <- address }
	t.Cleanup(func() { announce = previous })
	t.Setenv("RUTIS_TOKEN", "secret")
	plugin := Define(Plugin[NoConfig]{Name: "noop", Apply: func(*Ctx, NoConfig) error { return nil }})
	endpoint := &peer.Endpoint{Local: "runtime", Declare: []string{"runtime"}}
	go func() { _ = listen("ws://127.0.0.1:0/rutis", endpoint, []*Definition{plugin}) }()
	address := testwait.Within(t, "the listener's address", addresses)
	first := dial(t, address)
	if opcode, payload := first.next(); opcode != 0x1 || !strings.Contains(string(payload), `"op":"hello"`) {
		t.Fatalf("the session greets: %d %q", opcode, payload)
	}
	dial(t, address)
	if code := first.closed(); code != replaced {
		t.Fatalf("the older connection closed with %d, not %d", code, replaced)
	}
}
