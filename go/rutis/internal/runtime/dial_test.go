package rutis

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"math/big"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

func TestRetryDefaultsAndTheTestOverride(t *testing.T) {
	if r := retryFrom(""); r != defaultRetry() {
		t.Fatalf("%+v", r)
	}
	r := retryFrom("100,400,60000,300")
	if r.initial != 100*time.Millisecond || r.max != 400*time.Millisecond || r.stable != time.Minute || r.rejected != 300*time.Millisecond {
		t.Fatalf("%+v", r)
	}
	if r.jitter != 0.2 || r.handshake != 10*time.Second {
		t.Fatalf("the override keeps the rest: %+v", r)
	}
	for _, malformed := range []string{"100,400", "a,b,c,d", "1,2,3,-4"} {
		if r := retryFrom(malformed); r != defaultRetry() {
			t.Fatalf("%s: %+v", malformed, r)
		}
	}
}

func TestBackoffDoublesUpToTheMaximumWithJitter(t *testing.T) {
	b := newBackoff(defaultRetry())
	b.random = func() float64 { return 0.5 }
	var waits []string
	for range 9 {
		waits = append(waits, seconds(b.wait()))
	}
	if got := strings.Join(waits, " "); got != "0.5s 1s 2s 4s 8s 16s 30s 30s 30s" {
		t.Fatal(got)
	}
	b.reset()
	if wait := b.wait(); wait != 500*time.Millisecond {
		t.Fatalf("reset: %v", wait)
	}
	// ± 20 %.
	b.reset()
	b.random = func() float64 { return 0 }
	if wait := b.wait(); wait != 400*time.Millisecond {
		t.Fatalf("low: %v", wait)
	}
	b.reset()
	b.random = func() float64 { return 0.999999 }
	if wait := b.wait(); wait < 599*time.Millisecond || wait > 600*time.Millisecond {
		t.Fatalf("high: %v", wait)
	}
}

func TestStateLinesSayWaitsToATenth(t *testing.T) {
	for wait, said := range map[time.Duration]string{
		500 * time.Millisecond:  "0.5s",
		time.Second:             "1s",
		30 * time.Second:        "30s",
		340 * time.Millisecond:  "0.3s",
		1260 * time.Millisecond: "1.3s",
	} {
		if got := seconds(wait); got != said {
			t.Fatalf("%v: %s, not %s", wait, got, said)
		}
	}
}

func TestASessionsEndIsClassified(t *testing.T) {
	for _, c := range []struct {
		err      error
		farClose int
		want     failure
	}{
		{errors.New("session failed: rejected: pixel is not known"), rejected, authRejected},
		{fmt.Errorf("%w: the far end speaks protocol 9, this side 3", peer.ErrIncompatible), 0, incompatible},
		{fmt.Errorf("%w: the far end greeted as other", peer.ErrEndpointMismatch), 0, authRejected},
		{errors.New("session failed: replaced by a new connection"), replaced, retryable},
		{peer.HostGone(nil), goingAway, retryable},
		// Classified by type, never by text.
		{errors.New("incompatible session: said by a plugin"), 0, retryable},
	} {
		if got := sessionFailure(c.err, c.farClose); got != c.want {
			t.Fatalf("%v (%d): %v, not %v", c.err, c.farClose, got, c.want)
		}
	}
}

func TestOnlyALoopbackAccessPointIsDialedWithoutTLS(t *testing.T) {
	for _, spec := range []string{"ws://127.0.0.1:1/rutis", "ws://[::1]:1/rutis", "ws://localhost/rutis", "wss://example.com/rutis"} {
		if _, err := dialHost(spec); err != nil {
			t.Fatalf("%s: %v", spec, err)
		}
	}
	for _, spec := range []string{"ws://example.com/rutis", "ws://10.0.0.1/rutis", "http://127.0.0.1/", "wss:///rutis"} {
		if _, err := dialHost(spec); err == nil {
			t.Fatalf("%s is dialed", spec)
		}
	}
}

func TestWhatRetryingCannotFixFailsBeforeDialing(t *testing.T) {
	for _, c := range []struct {
		args []string
		says string
	}{
		{[]string{"dial:ws://127.0.0.1:1/rutis", "dir"}, "needs --id"},
		{[]string{"dial:ws://127.0.0.1:1/rutis", "--id", "Pixel 7", "dir"}, "invalid endpoint id"},
		{[]string{"dial:ws://example.com/rutis", "--id", "pixel", "dir"}, "needs wss://"},
		{[]string{"dial:tcp://127.0.0.1:1", "--id", "pixel", "dir"}, "not a ws:// or wss:// address"},
	} {
		if _, _, _, err := parseArgs(c.args); err == nil || !strings.Contains(err.Error(), c.says) {
			t.Fatalf("%v: %v", c.args, err)
		}
	}
	channel, endpoint, dir, err := parseArgs([]string{"dial:wss://example.com/rutis", "--id", "pixel", "--peer", "main", "dir"})
	if err != nil || channel != "dial:wss://example.com/rutis" || dir != "dir" ||
		endpoint.Local != "pixel" || endpoint.Expected != "main" || endpoint.Declare[0] != "runtime" {
		t.Fatalf("%s %+v %s %v", channel, endpoint, dir, err)
	}
}

// dialFailure dials `address` and wants a *dialError.
func dialFailure(t *testing.T, address, token string) *dialError {
	t.Helper()
	conn, err := dialWebSocket(address, testProtocol, token, nil, 5*time.Second)
	var failed *dialError
	if !errors.As(err, &failed) {
		if conn != nil {
			conn.Close("")
		}
		t.Fatalf("%s: %v", address, err)
	}
	return failed
}

func TestADialIsClassifiedByItsStatus(t *testing.T) {
	_, address := startListener(t)
	for _, c := range []struct {
		address, token string
		want           failure
		says           string
	}{
		{address, "", authRejected, "upgrade refused with 401: credentials required"},
		{address, "guess", authRejected, "upgrade refused with 403"},
		{strings.TrimSuffix(address, "rutis") + "other", "secret", incompatible, "upgrade refused with 404"},
	} {
		failed := dialFailure(t, c.address, c.token)
		if failed.failure != c.want || !strings.Contains(failed.reason, c.says) || strings.Contains(failed.reason, "guess") {
			t.Fatalf("%+v: %v %s", c, failed.failure, failed.reason)
		}
	}
	// Another session protocol: the access point says which end to upgrade.
	accepted, other, err := listenWebSocket("ws://127.0.0.1:0/rutis", "rutis.9", "secret", "", "")
	if err != nil {
		t.Fatal(err)
	}
	_ = accepted
	failed := dialFailure(t, other, "secret")
	if failed.failure != incompatible || !strings.Contains(failed.reason, "400: this endpoint speaks rutis.9, the dialer offered rutis.3: upgrade the dialer") {
		t.Fatalf("%v %s", failed.failure, failed.reason)
	}
	// Nothing listening: retryable.
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	closed := fmt.Sprintf("ws://%s/rutis", listener.Addr())
	listener.Close()
	if failed := dialFailure(t, closed, "secret"); failed.failure != retryable {
		t.Fatalf("%v %s", failed.failure, failed.reason)
	}
}

func TestADialedConnectionCarriesMessagesBothWays(t *testing.T) {
	accepted, address := startListener(t)
	conn, err := dialWebSocket(address, testProtocol, "secret", nil, 5*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	server := accept(t, accepted)
	for _, size := range []int{2, 200, 70_000} {
		message := []byte(strings.Repeat("x", size))
		if err := conn.Send(message); err != nil {
			t.Fatal(err)
		}
		if got, err := recv(t, server); err != nil || string(got) != string(message) {
			t.Fatalf("to the access point, %d bytes: %d %v", size, len(got), err)
		}
		if err := server.Send(message); err != nil {
			t.Fatal(err)
		}
		if got, err := conn.Recv(); err != nil || string(got) != string(message) {
			t.Fatalf("from the access point, %d bytes: %d %v", size, len(got), err)
		}
	}
	// The access point rejecting the id: the code is kept, the reason said.
	server.closeWith(rejected, "pixel is not known")
	if _, err := conn.Recv(); err == nil || err.Error() != "rejected: pixel is not known" {
		t.Fatal(err)
	}
	if conn.farClose.Load() != rejected {
		t.Fatal(conn.farClose.Load())
	}
}

func TestADialerClosesWithGoingAway(t *testing.T) {
	accepted, address := startListener(t)
	conn, err := dialWebSocket(address, testProtocol, "secret", nil, 5*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	server := accept(t, accepted)
	conn.Close("the runtime stopped")
	if _, err := recv(t, server); !errors.Is(err, io.EOF) {
		t.Fatal(err)
	}
	if server.farClose.Load() != goingAway {
		t.Fatal(server.farClose.Load())
	}
}

// selfSigned writes a self-signed certificate for 127.0.0.1 and its key.
func selfSigned(t *testing.T) (cert, key string) {
	t.Helper()
	private, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	template := &x509.Certificate{
		SerialNumber:          big.NewInt(1),
		Subject:               pkix.Name{CommonName: "rutis test"},
		NotBefore:             time.Now().Add(-time.Hour),
		NotAfter:              time.Now().Add(time.Hour),
		IPAddresses:           []net.IP{net.ParseIP("127.0.0.1")},
		KeyUsage:              x509.KeyUsageDigitalSignature | x509.KeyUsageCertSign,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		BasicConstraintsValid: true,
		IsCA:                  true,
	}
	der, err := x509.CreateCertificate(rand.Reader, template, template, &private.PublicKey, private)
	if err != nil {
		t.Fatal(err)
	}
	encoded, err := x509.MarshalECPrivateKey(private)
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	cert, key = filepath.Join(dir, "cert.pem"), filepath.Join(dir, "key.pem")
	if err := os.WriteFile(cert, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(key, pem.EncodeToMemory(&pem.Block{Type: "EC PRIVATE KEY", Bytes: encoded}), 0o600); err != nil {
		t.Fatal(err)
	}
	return cert, key
}

func TestAnUntrustedCertificateIsRejectedAndRUTISCATrustsIt(t *testing.T) {
	cert, key := selfSigned(t)
	previous := announce
	announce = func(string) {}
	accepted, address, err := listenWebSocket("wss://127.0.0.1:0/rutis", testProtocol, "secret", cert, key)
	announce = previous
	if err != nil {
		t.Fatal(err)
	}
	system, err := clientTLS("")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := dialWebSocket(address, testProtocol, "secret", system, 5*time.Second); err == nil {
		t.Fatal("an untrusted certificate is accepted")
	} else if failed := (*dialError)(nil); !errors.As(err, &failed) || failed.failure != authRejected {
		t.Fatalf("%v", err)
	}
	trusting, err := clientTLS(cert)
	if err != nil {
		t.Fatal(err)
	}
	conn, err := dialWebSocket(address, testProtocol, "secret", trusting, 5*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	server := accept(t, accepted)
	if err := conn.Send([]byte("over TLS")); err != nil {
		t.Fatal(err)
	}
	if got, err := recv(t, server); err != nil || string(got) != "over TLS" {
		t.Fatalf("%q %v", got, err)
	}
	conn.Close("")
	if _, err := clientTLS(key); err == nil {
		t.Fatal("a file without a certificate is trusted")
	}
	if _, err := clientTLS(filepath.Join(t.TempDir(), "missing.pem")); err == nil {
		t.Fatal("a missing file is trusted")
	}
}

func TestKeepDialingFollowsTheRules(t *testing.T) {
	r := retry{initial: time.Millisecond, max: 2 * time.Millisecond, jitter: 0.2, stable: time.Hour, rejected: 3 * time.Millisecond, handshake: time.Second}
	failures := []*dialError{
		{retryable, "nothing there"},
		{retryable, "nothing there"},
		{authRejected, "upgrade refused with 403"},
		{retryable, "nothing there"},
		{incompatible, "upgrade refused with 400"},
	}
	var said []string
	err := keepDialing("ws://127.0.0.1:1/rutis as pixel",
		func() (*wsConn, error) {
			next := failures[0]
			failures = failures[1:]
			return nil, next
		},
		func(*wsConn) (*session, error) { t.Fatal("nothing connected"); return nil, nil },
		nil, r,
		func(line string) { said = append(said, line) },
	)
	want := []string{
		"cannot connect (nothing there); retrying in 0s",
		"cannot connect (nothing there); retrying in 0s",
		"refused: auth-rejected (upgrade refused with 403); retrying in 0s",
		// A rejection does not reset the backoff.
		"cannot connect (nothing there); retrying in 0s",
	}
	if strings.Join(said, "\n") != strings.Join(want, "\n") {
		t.Fatalf("said:\n%s", strings.Join(said, "\n"))
	}
	var refused *refusedError
	if !errors.As(err, &refused) || err.Error() != "refused: incompatible (upgrade refused with 400); stopping" {
		t.Fatal(err)
	}
}

func TestStoppingEndsTheWaitWithoutDialingAgain(t *testing.T) {
	r := defaultRetry()
	stopping := make(chan struct{})
	dials := 0
	done := make(chan error)
	go func() {
		done <- keepDialing("ws://127.0.0.1:1/rutis as pixel",
			func() (*wsConn, error) {
				dials++
				return nil, &dialError{authRejected, "upgrade refused with 403"}
			},
			func(*wsConn) (*session, error) { return nil, errors.New("unreachable") },
			stopping, r,
			func(line string) {
				if line == "refused: auth-rejected (upgrade refused with 403); retrying in 30s" {
					close(stopping)
				}
			},
		)
	}()
	select {
	case err := <-done:
		if err != nil || dials != 1 {
			t.Fatalf("%v after %d dials", err, dials)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("still waiting after the stop")
	}
}
