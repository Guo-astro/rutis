package rutis

// Dialing out to a rutis access point, and dialing again: the rules every
// runtime follows (docs/design-remote-runtime-dial-2026-10-14.md §六), as
// rutis-bridge's dialing link does (`link.rs`).
//
// A failure is retryable (backoff from 0.5 s, doubling up to 30 s, reset by
// a session that lasted 60 s), an authentication failure (again every 30 s:
// 401 / 403 at the upgrade, a server certificate this side refuses, or the
// access point rejecting this id with close code 4003 after the hello), or
// incompatible (stop: retrying cannot fix it). Every session is new: the
// rows of one go when it ends, and the next connection is mounted afresh.

import (
	"bufio"
	"context"
	"crypto/rand"
	"crypto/sha1"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
	"math"
	mathrand "math/rand/v2"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/signal"
	"strconv"
	"strings"
	"syscall"
	"time"
	"unicode"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// failure is what a failed dial or an ended session means for retrying.
type failure int

const (
	retryable failure = iota
	authRejected
	incompatible
)

func (f failure) String() string {
	switch f {
	case authRejected:
		return "auth-rejected"
	case incompatible:
		return "incompatible"
	}
	return "retryable"
}

// dialError is a dial that failed: `failure` says what it means for
// retrying.
type dialError struct {
	failure failure
	reason  string
}

func (e *dialError) Error() string { return e.reason }

// refusedError ends a runtime that dials: the access point is
// incompatible. Its text is the state line.
type refusedError struct{ reason string }

func (e *refusedError) Error() string {
	return fmt.Sprintf("refused: incompatible (%s); stopping", e.reason)
}

// retry is reconnection: rutis-bridge's `Retry` defaults.
type retry struct {
	// The first wait after a retryable failure; doubled each time.
	initial time.Duration
	max     time.Duration
	// ± this fraction of each wait, at random.
	jitter float64
	// A session lasting this long resets the backoff.
	stable time.Duration
	// The wait after an authentication failure.
	rejected time.Duration
	// How long the access point may take to greet.
	handshake time.Duration
}

func defaultRetry() retry {
	return retry{
		initial:   500 * time.Millisecond,
		max:       30 * time.Second,
		jitter:    0.2,
		stable:    60 * time.Second,
		rejected:  30 * time.Second,
		handshake: 10 * time.Second,
	}
}

// retryFrom is the defaults, unless tests shorten them with `configured`
// (RUTIS_RETRY=<initial ms>,<max ms>,<stable ms>,<rejected ms>).
func retryFrom(configured string) retry {
	r := defaultRetry()
	parts := strings.Split(configured, ",")
	if len(parts) != 4 {
		return r
	}
	var ms [4]time.Duration
	for i, part := range parts {
		n, err := strconv.Atoi(strings.TrimSpace(part))
		if err != nil || n < 0 {
			return r
		}
		ms[i] = time.Duration(n) * time.Millisecond
	}
	r.initial, r.max, r.stable, r.rejected = ms[0], ms[1], ms[2], ms[3]
	return r
}

// backoff is exponential backoff with jitter.
type backoff struct {
	retry retry
	next  time.Duration
	// random is in [0, 1).
	random func() float64
}

func newBackoff(r retry) *backoff {
	return &backoff{retry: r, next: r.initial, random: mathrand.Float64}
}

func (b *backoff) reset() { b.next = b.retry.initial }

// wait is the next wait: the current step ± jitter; the step doubles up to
// the maximum.
func (b *backoff) wait() time.Duration {
	base := b.next
	b.next = min(b.next*2, b.retry.max)
	return time.Duration(float64(base) * (1 + b.retry.jitter*(2*b.random()-1)))
}

// sessionFailure is what a session's end means for retrying: the access
// point rejecting this id (4003), or the far end greeting as another
// endpoint than expected, is an authentication failure; a greeting this
// side cannot speak is incompatible; anything else, being replaced (4002)
// too, is retryable.
func sessionFailure(err error, farClose int) failure {
	switch {
	case farClose == rejected:
		return authRejected
	case errors.Is(err, peer.ErrIncompatible):
		return incompatible
	case errors.Is(err, peer.ErrEndpointMismatch):
		return authRejected
	}
	return retryable
}

// seconds is `wait` for a state line: 0.5s, 1s, 30s.
func seconds(wait time.Duration) string {
	return strconv.FormatFloat(math.Round(wait.Seconds()*10)/10, 'f', -1, 64) + "s"
}

// report says a state line (stderr; tests replace it).
var report = func(line string) {
	fmt.Fprintf(os.Stderr, "rutis: %s\n", line)
}

// dialHost is the host of `spec`, an access point to dial: an error unless
// it is a ws:// or wss:// address; a non-loopback one needs wss:// (the
// rule of rutis-bridge's dialer).
func dialHost(spec string) (string, error) {
	address, err := url.Parse(spec)
	if err != nil || (address.Scheme != "ws" && address.Scheme != "wss") || address.Hostname() == "" {
		return "", fmt.Errorf("%s is not a ws:// or wss:// address", spec)
	}
	host := address.Hostname()
	if address.Scheme == "ws" && !loopback(host) {
		return "", fmt.Errorf("%s: a non-loopback address needs wss://", spec)
	}
	return host, nil
}

func loopback(host string) bool {
	if host == "localhost" {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}

// clientTLS is what a wss:// dial verifies the access point against: the
// system's roots, and the CA certificates in the PEM file `ca` (RUTIS_CA).
func clientTLS(ca string) (*tls.Config, error) {
	roots, err := x509.SystemCertPool()
	if err != nil {
		roots = x509.NewCertPool()
	}
	if ca != "" {
		data, err := os.ReadFile(ca)
		if err != nil {
			return nil, err
		}
		if !roots.AppendCertsFromPEM(data) {
			return nil, errors.New("no PEM certificate in it")
		}
	}
	return &tls.Config{RootCAs: roots, MinVersion: tls.VersionTLS12}, nil
}

// dialWebSocket dials the access point at `spec`, presenting `token` and
// speaking `protocol`; `config` verifies a wss:// one. A failure is a
// *dialError, classified from its type or status, never from its text, as
// rutis-bridge's dialer does: 401 / 403 and a certificate this side
// refuses are an authentication failure; 400, 404, 405, 426 and a session
// protocol refusal are incompatible; the rest is retryable.
func dialWebSocket(spec, protocol, token string, config *tls.Config, timeout time.Duration) (*wsConn, error) {
	host, err := dialHost(spec)
	if err != nil {
		return nil, &dialError{incompatible, err.Error()}
	}
	address, _ := url.Parse(spec)
	secure := address.Scheme == "wss"
	port := address.Port()
	if port == "" {
		port = map[bool]string{true: "443", false: "80"}[secure]
	}
	failed := func(kind failure, err error) (*wsConn, error) {
		return nil, &dialError{kind, fmt.Sprintf("%s: %v", host, err)}
	}
	deadline := time.Now().Add(timeout)
	dialer := net.Dialer{Deadline: deadline}
	conn, err := dialer.Dial("tcp", net.JoinHostPort(host, port))
	if err != nil {
		return failed(retryable, err)
	}
	if tcp, ok := conn.(*net.TCPConn); ok {
		_ = tcp.SetNoDelay(true)
	}
	_ = conn.SetDeadline(deadline)
	if secure {
		tlsConfig := config.Clone()
		if tlsConfig == nil {
			tlsConfig = &tls.Config{MinVersion: tls.VersionTLS12}
		}
		tlsConfig.ServerName = host
		client := tls.Client(conn, tlsConfig)
		if err := client.Handshake(); err != nil {
			conn.Close()
			var verification *tls.CertificateVerificationError
			if errors.As(err, &verification) {
				return failed(authRejected, fmt.Errorf("TLS: %w", err))
			}
			return failed(retryable, fmt.Errorf("TLS: %w", err))
		}
		conn = client
	}
	var key [16]byte
	_, _ = rand.Read(key[:])
	challenge := base64.StdEncoding.EncodeToString(key[:])
	request := &http.Request{
		Method:     http.MethodGet,
		URL:        &url.URL{Path: address.EscapedPath(), RawQuery: address.RawQuery},
		Proto:      "HTTP/1.1",
		ProtoMajor: 1,
		ProtoMinor: 1,
		Header: http.Header{
			"Upgrade":                {"websocket"},
			"Connection":             {"Upgrade"},
			"Sec-Websocket-Key":      {challenge},
			"Sec-Websocket-Version":  {"13"},
			"Sec-Websocket-Protocol": {protocol},
		},
		Host: address.Host,
	}
	if request.URL.Path == "" {
		request.URL.Path = "/"
	}
	if token != "" {
		request.Header.Set("Authorization", "Bearer "+token)
	}
	if err := request.Write(conn); err != nil {
		conn.Close()
		return failed(retryable, err)
	}
	reader := bufio.NewReader(conn)
	response, err := http.ReadResponse(reader, request)
	if err != nil {
		conn.Close()
		return failed(retryable, err)
	}
	if response.StatusCode != http.StatusSwitchingProtocols {
		defer conn.Close()
		reason := fmt.Sprintf("%s: upgrade refused with %d", host, response.StatusCode)
		// The access point's own words (for a session protocol refusal,
		// which end to upgrade): the first line, bounded.
		body, _ := io.ReadAll(io.LimitReader(response.Body, 4096))
		if said := firstLine(string(body)); said != "" {
			reason += ": " + said
		}
		switch response.StatusCode {
		case 401, 403:
			return nil, &dialError{authRejected, reason}
		case 400, 404, 405, 426:
			return nil, &dialError{incompatible, reason}
		}
		return nil, &dialError{retryable, reason}
	}
	sum := sha1.Sum([]byte(challenge + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))
	if !strings.EqualFold(response.Header.Get("Upgrade"), "websocket") ||
		response.Header.Get("Sec-WebSocket-Accept") != base64.StdEncoding.EncodeToString(sum[:]) {
		conn.Close()
		return failed(retryable, errors.New("not a WebSocket upgrade"))
	}
	if chosen := response.Header.Get("Sec-WebSocket-Protocol"); chosen != protocol {
		ws := startWSConn(&wsConn{conn: conn, reader: reader, client: true, done: make(chan struct{})})
		ws.closeWith(1002, "no session protocol")
		if chosen == "" {
			return failed(incompatible, errors.New("the access point chose no protocol: it speaks none this process speaks"))
		}
		return failed(incompatible, fmt.Errorf("the access point chose the protocol %q, which this process does not speak", chosen))
	}
	_ = conn.SetDeadline(time.Time{})
	return startWSConn(&wsConn{conn: conn, reader: reader, client: true, done: make(chan struct{})}), nil
}

// firstLine is the first line of `text`, printable and bounded.
func firstLine(text string) string {
	line, _, _ := strings.Cut(strings.TrimSpace(text), "\n")
	line = strings.TrimSpace(line)
	if runes := []rune(line); len(runes) > 200 {
		line = string(runes[:200])
	}
	return strings.Map(func(r rune) rune {
		if unicode.IsPrint(r) {
			return r
		}
		return -1
	}, line)
}

// dialOut dials a rutis access point and stays connected: each connection
// is a new session, whose rows go when it ends; it dials again by the
// rules until it is stopped (SIGTERM, Ctrl-C: the rows go, then the
// connection closes; nil) or the access point is incompatible (a
// *refusedError).
func dialOut(spec string, endpoint *peer.Endpoint, defs []*Definition) error {
	// Spent once read: what this process starts does not inherit it.
	token := os.Getenv("RUTIS_TOKEN")
	os.Unsetenv("RUTIS_TOKEN")
	var config *tls.Config
	if strings.HasPrefix(spec, "wss:") {
		ca := os.Getenv("RUTIS_CA")
		var err error
		if config, err = clientTLS(ca); err != nil {
			return fmt.Errorf("RUTIS_CA %s: %w", ca, err)
		}
	}
	stopping, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	r := retryFrom(os.Getenv("RUTIS_RETRY"))
	protocol := fmt.Sprintf("rutis.%d", peer.EndpointProtocol)
	return keepDialing(
		fmt.Sprintf("%s as %s", spec, endpoint.Local),
		func() (*wsConn, error) { return dialWebSocket(spec, protocol, token, config, r.handshake) },
		func(conn *wsConn) (*session, error) { return startSession(conn, endpoint, defs) },
		stopping.Done(),
		r,
		report,
	)
}

// keepDialing dials with `dial`, serves the session `start` starts on the
// connection, and dials again by the rules, until `stopping` is closed
// (nil) or the access point is incompatible (a *refusedError). `say` gets
// one line per state change.
func keepDialing(address string, dial func() (*wsConn, error), start func(*wsConn) (*session, error), stopping <-chan struct{}, r retry, say func(string)) error {
	b := newBackoff(r)
	type attempt struct {
		conn *wsConn
		err  error
	}
	for {
		attempted := make(chan attempt, 1)
		go func() {
			conn, err := dial()
			attempted <- attempt{conn, err}
		}()
		var a attempt
		select {
		case <-stopping:
			// A dial that connects after the runtime stopped is closed.
			go func() {
				if late := <-attempted; late.conn != nil {
					late.conn.Close("the runtime stopped")
				}
			}()
			return nil
		case a = <-attempted:
		}
		kind, reason, lost := retryable, "", "cannot connect"
		var failed *dialError
		switch {
		case errors.As(a.err, &failed):
			kind, reason = failed.failure, failed.reason
		case a.err != nil:
			reason = a.err.Error()
		default:
			s, err := start(a.conn)
			if err != nil {
				a.conn.Close(err.Error())
				reason = err.Error()
				break
			}
			greeting := time.NewTimer(r.handshake)
			select {
			case <-stopping:
				greeting.Stop()
				return s.stop(a.conn)
			case <-greeting.C:
				s.peer.Close(errors.New("the access point did not greet in time"))
				a.conn.Close("no greeting")
			case <-s.peer.Ready():
				greeting.Stop()
				if s.peer.Err() != nil {
					break
				}
				say("connected to " + address)
				since := time.Now()
				select {
				case <-stopping:
					return s.stop(a.conn)
				case <-s.peer.Closed():
				}
				if time.Since(since) >= r.stable {
					b.reset()
				}
			}
			ended := s.peer.Err()
			kind, reason, lost = sessionFailure(ended, int(a.conn.farClose.Load())), ended.Error(), "connection lost"
			_ = s.end(nil)
		}
		var wait time.Duration
		switch kind {
		case incompatible:
			return &refusedError{reason}
		case authRejected:
			wait = r.rejected
			say(fmt.Sprintf("refused: auth-rejected (%s); retrying in %s", reason, seconds(wait)))
		default:
			wait = b.wait()
			say(fmt.Sprintf("%s (%s); retrying in %s", lost, reason, seconds(wait)))
		}
		pause := time.NewTimer(wait)
		select {
		case <-stopping:
			pause.Stop()
			return nil
		case <-pause.C:
		}
	}
}

// stop stops for good: every row goes while the controller still hears
// from this end, then the connection closes. It fails when cleanups were
// given up.
func (s *session) stop(conn *wsConn) error {
	s.runtime.mu.Lock()
	s.runtime.closing = true
	s.runtime.mu.Unlock()
	budget := endedBudget()
	ctx, cancel := context.WithTimeout(context.Background(), budget)
	defer cancel()
	err := s.runtime.dispose(ctx)
	s.peer.Close(errors.New("the runtime stopped"))
	conn.Close("the runtime stopped")
	if err != nil {
		return fmt.Errorf("cleanups did not finish within %v after the runtime stopped", budget)
	}
	return nil
}
