package rutis

import (
	"bufio"
	"crypto/rand"
	"crypto/sha1"
	"crypto/subtle"
	"crypto/tls"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"slices"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unicode/utf8"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// protocolRefusal says why a dialer that offered none of this endpoint's
// session protocol is refused, and which end to upgrade (standard Q6.2.4)
// when the versions it offered (`rutis.<n>`) are all newer (this endpoint)
// or all older (the dialer) than this endpoint's. The wording is
// rutis-bridge's (`transport::websocket::listen::protocol_refusal`).
func protocolRefusal(ours string, offered []string) string {
	version := func(protocol string) (int, bool) {
		number, ok := strings.CutPrefix(protocol, "rutis.")
		if !ok || number == "" || strings.Trim(number, "0123456789") != "" {
			return 0, false
		}
		n, err := strconv.Atoi(number)
		return n, err == nil
	}
	reason := "this endpoint speaks " + ours
	n, ok := version(ours)
	var theirs []int
	var names []string
	for _, protocol := range offered {
		if v, ok := version(protocol); ok {
			theirs = append(theirs, v)
			names = append(names, protocol)
		}
	}
	if !ok || len(theirs) == 0 {
		return reason
	}
	reason += ", the dialer offered " + strings.Join(names, ", ")
	newer, older := true, true
	for _, v := range theirs {
		newer = newer && v > n
		older = older && v < n
	}
	switch {
	case newer:
		return reason + ": upgrade this endpoint (Go module github.com/arcships/rutis/go/rutis)"
	case older:
		return reason + ": upgrade the dialer"
	}
	return reason
}

const (
	goingAway   = 1001
	unsupported = 1003
	tooBig      = 1009
	replaced    = 4002
	// rejected is an access point rejecting the id a dialer greeted with.
	rejected = 4003
)

// errClosed is what a write after the connection's close frame says.
var errClosed = errors.New("connection closed")

// heartbeat is the ping interval and how long a silent far end is kept
// (RUTIS_HEARTBEAT=<ping ms>,<timeout ms> shortens them for tests).
func heartbeat() (time.Duration, time.Duration) {
	if configured := os.Getenv("RUTIS_HEARTBEAT"); configured != "" {
		ping, timeout, ok := strings.Cut(configured, ",")
		p, err1 := strconv.Atoi(ping)
		t, err2 := strconv.Atoi(timeout)
		if ok && err1 == nil && err2 == nil {
			return time.Duration(p) * time.Millisecond, time.Duration(t) * time.Millisecond
		}
	}
	return 10 * time.Second, 30 * time.Second
}

// announce reports the address a listener is bound to (stderr; tests
// replace it).
var announce = func(address string) {
	fmt.Fprintf(os.Stderr, "rutis: listening on %s\n", address)
}

// listen serves one controller at a time on a WebSocket address.
func listen(spec string, endpoint *peer.Endpoint, defs []*Definition) error {
	listener, err := listenWebSocket(spec, fmt.Sprintf("rutis.%d", peer.EndpointProtocol), os.Getenv("RUTIS_TOKEN"), os.Getenv("RUTIS_CERT"), os.Getenv("RUTIS_KEY"))
	if err != nil {
		return err
	}
	return listener.serve(endpoint, defs)
}

// wsListener is a listening endpoint: the connections it accepts, the
// address it is bound to, and Stop. A runtime listens until the process
// ends; a test stops it.
type wsListener struct {
	accepted chan *wsConn
	address  string
	server   *http.Server
	// stop is closed by Stop: the handler stops handing out the connection
	// it upgraded, and serve returns.
	stop chan struct{}
	// served is closed once Serve returned, after Stop closed the server.
	served chan struct{}
	once   sync.Once
	// mu guards conns: the accepted connections not closed yet.
	mu    sync.Mutex
	conns map[*wsConn]struct{}
}

// serve serves one controller at a time: a newer connection takes over, and
// is greeted only once the old lease is gone. Its heartbeat starts at that
// hello, not at the accept: a cleanup longer than the heartbeat timeout
// does not close it (#1182).
func (l *wsListener) serve(endpoint *peer.Endpoint, defs []*Definition) error {
	defer l.Stop()
	var current *session
	var currentConn *wsConn
	for {
		var closed <-chan struct{}
		if current != nil {
			closed = current.peer.Closed()
		}
		select {
		case conn := <-l.accepted:
			if current != nil {
				currentConn.closeWith(replaced, "replaced by a new connection")
				current.end(errors.New("replaced by a new connection"))
			}
			var err error
			current, err = startSession(conn, endpoint, defs)
			currentConn = conn
			if err != nil {
				conn.closeWith(goingAway, err.Error())
				current, currentConn = nil, nil
			}
		case <-closed:
			current.end(nil)
			if refused := refusal(current.peer.Err()); refused != nil {
				return refused
			}
			current, currentConn = nil, nil
		case <-l.stop:
			if current != nil {
				currentConn.closeWith(goingAway, "the listener stopped")
				current.end(nil)
			}
			return nil
		}
	}
}

// Stop stops the listener: the server closes (idle and in-flight
// connections included), the connections it accepted close, their
// heartbeats end, and serve returns. It is a test's: a runtime listens
// until the process ends.
func (l *wsListener) Stop() {
	l.once.Do(func() {
		l.mu.Lock()
		conns := make([]*wsConn, 0, len(l.conns))
		for conn := range l.conns {
			conns = append(conns, conn)
		}
		close(l.stop)
		l.mu.Unlock()
		_ = l.server.Close()
		for _, conn := range conns {
			conn.closeWith(goingAway, "the listener stopped")
		}
		<-l.served
	})
}

// forget drops `conn` from the listener's set, once it closed.
func (l *wsListener) forget(conn *wsConn) {
	l.mu.Lock()
	delete(l.conns, conn)
	l.mu.Unlock()
}

// listenWebSocket listens on `spec` for connections presenting `token` and
// speaking `protocol`; it returns the listener, with the address it is bound
// to.
func listenWebSocket(spec, protocol, token, cert, key string) (*wsListener, error) {
	address, err := url.Parse(spec)
	if err != nil || (address.Scheme != "ws" && address.Scheme != "wss") {
		return nil, fmt.Errorf("%s is not a ws:// or wss:// address", spec)
	}
	secure := address.Scheme == "wss"
	host := address.Hostname()
	if host == "" {
		host = "127.0.0.1"
	}
	if !secure && host != "127.0.0.1" && host != "::1" && host != "localhost" {
		return nil, fmt.Errorf("%s: only a loopback listener may go without TLS", spec)
	}
	port := address.Port()
	if port == "" {
		port = map[bool]string{true: "443", false: "80"}[secure]
	}
	path := address.Path
	if path == "" {
		path = "/"
	}
	listener, err := net.Listen("tcp", net.JoinHostPort(host, port))
	if err != nil {
		return nil, err
	}
	if secure {
		if cert == "" || key == "" {
			listener.Close()
			return nil, fmt.Errorf("%s: wss needs a certificate and key (RUTIS_CERT, RUTIS_KEY)", spec)
		}
		pair, err := tls.LoadX509KeyPair(cert, key)
		if err != nil {
			listener.Close()
			return nil, err
		}
		listener = tls.NewListener(listener, &tls.Config{Certificates: []tls.Certificate{pair}, MinVersion: tls.VersionTLS12})
	}
	bound := listener.Addr().(*net.TCPAddr)
	shown := bound.IP.String()
	if strings.Contains(shown, ":") {
		shown = "[" + shown + "]"
	}
	boundAddress := fmt.Sprintf("%s://%s:%d%s", address.Scheme, shown, bound.Port, path)
	announce(boundAddress)
	l := &wsListener{
		accepted: make(chan *wsConn),
		address:  boundAddress,
		stop:     make(chan struct{}),
		served:   make(chan struct{}),
		conns:    map[*wsConn]struct{}{},
	}
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != path {
			http.Error(w, "no rutis endpoint here", http.StatusNotFound)
			return
		}
		header := r.Header.Get("Authorization")
		if !strings.HasPrefix(header, "Bearer ") {
			http.Error(w, "credentials required", http.StatusUnauthorized)
			return
		}
		presented := []byte(strings.TrimPrefix(header, "Bearer "))
		if token == "" || subtle.ConstantTimeCompare(presented, []byte(token)) != 1 {
			http.Error(w, "not accepted here", http.StatusForbidden)
			return
		}
		var offered []string
		for _, value := range r.Header.Values("Sec-WebSocket-Protocol") {
			for _, item := range strings.Split(value, ",") {
				offered = append(offered, strings.TrimSpace(item))
			}
		}
		if !slices.Contains(offered, protocol) {
			http.Error(w, protocolRefusal(protocol, offered), http.StatusBadRequest)
			return
		}
		challenge := r.Header.Get("Sec-WebSocket-Key")
		if !strings.EqualFold(r.Header.Get("Upgrade"), "websocket") || challenge == "" {
			http.Error(w, "a WebSocket upgrade is required", http.StatusBadRequest)
			return
		}
		hijacker, ok := w.(http.Hijacker)
		if !ok {
			http.Error(w, "cannot upgrade", http.StatusInternalServerError)
			return
		}
		conn, buffered, err := hijacker.Hijack()
		if err != nil {
			return
		}
		sum := sha1.Sum([]byte(challenge + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))
		response := "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
			"Sec-WebSocket-Accept: " + base64.StdEncoding.EncodeToString(sum[:]) + "\r\n" +
			"Sec-WebSocket-Protocol: " + protocol + "\r\n\r\n"
		if _, err := conn.Write([]byte(response)); err != nil {
			conn.Close()
			return
		}
		ws := newWSConn(conn, buffered.Reader)
		ws.onClose = func() { l.forget(ws) }
		l.mu.Lock()
		select {
		case <-l.stop:
			l.mu.Unlock()
			ws.shut()
			return
		default:
		}
		l.conns[ws] = struct{}{}
		l.mu.Unlock()
		select {
		case l.accepted <- ws:
		case <-l.stop:
			ws.shut()
		}
	})
	l.server = &http.Server{Handler: handler, ReadHeaderTimeout: 10 * time.Second}
	go func() {
		_ = l.server.Serve(listener)
		close(l.served)
	}()
	return l, nil
}

// wsConn is a WebSocket connection carrying text messages: accepted by a
// listener, or dialed (`client`: it masks what it sends).
type wsConn struct {
	conn    net.Conn
	reader  *bufio.Reader
	client  bool
	writeMu sync.Mutex
	once    sync.Once
	done    chan struct{}
	// lastSeen is when the far end last said something, in
	// time.Now's units; it counts once beating is set, at the session's
	// hello.
	lastSeen atomic.Int64
	// beating says the session's hello went out: until then the silence of
	// a connection waiting for the old lease to be cleaned up is not a lost
	// far end (#1182).
	beating atomic.Bool
	// farClose is the code of the far end's close frame, once it closed
	// with one.
	farClose atomic.Int32
	// closing says this side has sent (or is sending) its close frame: from
	// then on no other frame goes out (RFC 6455 §5.5.1).
	closing atomic.Bool
	// sendGate, when a test sets it, runs in Send between the check that the
	// connection is open and the write: it makes the interleaving where a
	// close lands in between happen every time. Nil in production.
	sendGate func()
	// onClose, when set, runs once as the connection closes: its listener
	// forgets it.
	onClose func()
}

func newWSConn(conn net.Conn, reader *bufio.Reader) *wsConn {
	return startWSConn(&wsConn{conn: conn, reader: reader, done: make(chan struct{})})
}

func startWSConn(ws *wsConn) *wsConn {
	go ws.heartbeat()
	return ws
}

// startHeartbeat starts counting silence, at the session's hello: a
// connection waiting for the old lease to be cleaned up is not closed with
// 1001 before it (#1182).
func (ws *wsConn) startHeartbeat() {
	ws.lastSeen.Store(time.Now().UnixNano())
	ws.beating.Store(true)
}

func (ws *wsConn) heartbeat() {
	interval, timeout := heartbeat()
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		select {
		case <-ws.done:
			return
		case <-ticker.C:
			// Waiting for its hello to go out, a connection is not silent.
			if ws.beating.Load() && time.Since(time.Unix(0, ws.lastSeen.Load())) > timeout {
				ws.closeWith(goingAway, "the far end stopped answering")
				return
			}
			_ = ws.writeFrame(0x9, nil)
		}
	}
}

func (ws *wsConn) writeFrame(opcode byte, payload []byte) error {
	header := []byte{0x80 | opcode}
	var mask byte
	if ws.client {
		mask = 0x80
	}
	switch n := len(payload); {
	case n < 126:
		header = append(header, mask|byte(n))
	case n <= 0xFFFF:
		header = append(header, mask|126, byte(n>>8), byte(n))
	default:
		header = append(header, mask|127)
		header = binary.BigEndian.AppendUint64(header, uint64(n))
	}
	if ws.client {
		var key [4]byte
		_, _ = rand.Read(key[:])
		header = append(header, key[:]...)
		masked := make([]byte, len(payload))
		for i, b := range payload {
			masked[i] = b ^ key[i%4]
		}
		payload = masked
	}
	ws.writeMu.Lock()
	defer ws.writeMu.Unlock()
	// A Send that passed its check before closeWith set closing waits here and
	// gives up, so no frame is written after the close frame.
	if opcode != 0x8 && ws.closing.Load() {
		return errClosed
	}
	_ = ws.conn.SetWriteDeadline(time.Now().Add(30 * time.Second))
	if _, err := ws.conn.Write(append(header, payload...)); err != nil {
		return err
	}
	return nil
}

func (ws *wsConn) Send(message []byte) error {
	if len(message) > peer.MaxMessage {
		reason := fmt.Sprintf("message of %d bytes exceeds the limit of %d", len(message), peer.MaxMessage)
		ws.closeWith(tooBig, reason)
		return errors.New(reason)
	}
	select {
	case <-ws.done:
		return errClosed
	default:
	}
	if ws.sendGate != nil {
		ws.sendGate()
	}
	return ws.writeFrame(0x1, message)
}

func (ws *wsConn) Recv() ([]byte, error) {
	var message []byte
	for {
		fin, opcode, payload, err := ws.readFrame()
		if err != nil {
			ws.shut()
			return nil, err
		}
		ws.lastSeen.Store(time.Now().UnixNano())
		switch opcode {
		case 0x8:
			code := 1005
			if len(payload) >= 2 {
				code = int(binary.BigEndian.Uint16(payload))
			}
			ws.farClose.Store(int32(code))
			ws.closeWith(code, "")
			if code == 1000 || code == goingAway || code == 1005 {
				return nil, io.EOF
			}
			if code == replaced {
				return nil, errors.New("replaced by a new connection")
			}
			if code == rejected {
				return nil, fmt.Errorf("rejected: %s", string(payload[min(2, len(payload)):]))
			}
			return nil, fmt.Errorf("closed (%d): %s", code, string(payload[min(2, len(payload)):]))
		case 0x9:
			_ = ws.writeFrame(0xA, payload)
			continue
		case 0xA:
			continue
		case 0x2:
			ws.closeWith(unsupported, "binary messages are reserved for a binary encoding")
			return nil, errors.New("received a binary message")
		case 0x1, 0x0:
			message = append(message, payload...)
			if len(message) > peer.MaxMessage {
				ws.closeWith(tooBig, "message too big")
				return nil, fmt.Errorf("a message exceeds the limit of %d bytes", peer.MaxMessage)
			}
			if fin {
				if !utf8.Valid(message) {
					ws.closeWith(1007, "invalid UTF-8")
					return nil, errors.New("a message is not UTF-8")
				}
				return message, nil
			}
		default:
			ws.closeWith(1002, "unknown opcode")
			return nil, errors.New("protocol error")
		}
	}
}

func (ws *wsConn) readFrame() (bool, byte, []byte, error) {
	var head [2]byte
	if _, err := io.ReadFull(ws.reader, head[:]); err != nil {
		return false, 0, nil, err
	}
	fin, opcode, masked := head[0]&0x80 != 0, head[0]&0x0F, head[1]&0x80 != 0
	length := uint64(head[1] & 0x7F)
	switch length {
	case 126:
		var extended [2]byte
		if _, err := io.ReadFull(ws.reader, extended[:]); err != nil {
			return false, 0, nil, err
		}
		length = uint64(binary.BigEndian.Uint16(extended[:]))
	case 127:
		var extended [8]byte
		if _, err := io.ReadFull(ws.reader, extended[:]); err != nil {
			return false, 0, nil, err
		}
		length = binary.BigEndian.Uint64(extended[:])
	}
	if masked == ws.client {
		if ws.client {
			ws.closeWith(1002, "server frames must not be masked")
			return false, 0, nil, errors.New("a masked server frame")
		}
		ws.closeWith(1002, "client frames must be masked")
		return false, 0, nil, errors.New("an unmasked client frame")
	}
	if length > peer.MaxMessage {
		ws.closeWith(tooBig, "message too big")
		return false, 0, nil, fmt.Errorf("a message exceeds the limit of %d bytes", peer.MaxMessage)
	}
	var mask [4]byte
	if masked {
		if _, err := io.ReadFull(ws.reader, mask[:]); err != nil {
			return false, 0, nil, err
		}
	}
	payload := make([]byte, length)
	if _, err := io.ReadFull(ws.reader, payload); err != nil {
		return false, 0, nil, err
	}
	if masked {
		for i := range payload {
			payload[i] ^= mask[i%4]
		}
	}
	return fin, opcode, payload, nil
}

// closeWith sends a close frame with `code` and closes the connection.
func (ws *wsConn) closeWith(code int, reason string) {
	ws.once.Do(func() {
		// Set before the close frame is written: a frame writer waiting for
		// writeMu gives up instead of writing after it.
		ws.closing.Store(true)
		if len(reason) > 123 {
			reason = reason[:123]
		}
		payload := binary.BigEndian.AppendUint16(nil, uint16(code))
		_ = ws.writeFrame(0x8, append(payload, reason...))
		close(ws.done)
		if ws.onClose != nil {
			ws.onClose()
		}
		go func() {
			time.Sleep(200 * time.Millisecond)
			ws.conn.Close()
		}()
	})
}

func (ws *wsConn) shut() {
	ws.once.Do(func() {
		ws.closing.Store(true)
		close(ws.done)
		ws.conn.Close()
		if ws.onClose != nil {
			ws.onClose()
		}
	})
}

func (ws *wsConn) Close(reason string) { ws.closeWith(goingAway, reason) }
