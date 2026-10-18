package rutis

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"net"
	"os"
	goruntime "runtime"
	"strconv"
	"strings"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// channelToken is the environment variable holding the token a process
// presents first on a `tcp:` channel.
const channelToken = "RUTIS_CHANNEL_TOKEN"

// deadlineVariable sets how long cleanups may take, in total, once the
// session ended and nobody waits for them (the host went away), in
// seconds; rutis-host passes its own shutdown deadline on.
const deadlineVariable = "RUTIS_SHUTDOWN_TIMEOUT"

// defaultDeadline is that time when deadlineVariable does not say.
const defaultDeadline = 10 * time.Second

// endedBudget is how long cleanups may take once the session ended.
func endedBudget() time.Duration {
	seconds, err := strconv.ParseFloat(strings.TrimSpace(os.Getenv(deadlineVariable)), 64)
	if err != nil || !(seconds > 0) || math.IsInf(seconds, 1) {
		return defaultDeadline
	}
	return time.Duration(seconds * float64(time.Second))
}

const usage = "usage: <binary> --rutis-manifest | <channel> [--id <endpoint>] [--peer <endpoint>] <project>"

var project string

// Project is the project directory the host gave this runtime.
func Project() string { return project }

// Serve runs the plugins `defs` as a rutis runtime, as the command line
// says, and exits the process: with --rutis-manifest it prints what the
// plugins declare; otherwise it serves the session on the channel it is
// given until the session ends.
func Serve(defs ...*Definition) {
	if err := ServeArgs(os.Args[1:], defs...); err != nil {
		fmt.Fprintln(os.Stderr, "rutis:", err)
		os.Exit(1)
	}
	os.Exit(0)
}

// ServeArgs is Serve with the arguments given, returning instead of
// exiting: for binaries with commands of their own.
func ServeArgs(args []string, defs ...*Definition) error {
	if err := checkDefinitions(defs); err != nil {
		return err
	}
	if len(args) == 1 && args[0] == "--rutis-manifest" {
		data, err := Manifest(defs...)
		if err != nil {
			return err
		}
		_, err = os.Stdout.Write(append(data, '\n'))
		return err
	}
	channel, endpoint, dir, err := parseArgs(args)
	if err != nil {
		return err
	}
	project = dir
	if strings.HasPrefix(channel, "listen:") {
		return listen(strings.TrimPrefix(channel, "listen:"), endpoint, defs)
	}
	if strings.HasPrefix(channel, "dial:") {
		return dialOut(strings.TrimPrefix(channel, "dial:"), endpoint, defs)
	}
	if strings.HasPrefix(channel, "ws:") || strings.HasPrefix(channel, "wss:") {
		return fmt.Errorf("a WebSocket channel is listen:<address> or dial:<address>: %s", channel)
	}
	conn, err := openChannel(channel)
	if err != nil {
		return err
	}
	return serveChannel(peer.NewLines(conn), nil, defs)
}

// ServeConn serves the plugins `defs` on `conn` (the compat protocol, one
// message per line) until the session ends, then unloads them.
func ServeConn(conn net.Conn, defs ...*Definition) error {
	if err := checkDefinitions(defs); err != nil {
		return err
	}
	return serveChannel(peer.NewLines(conn), nil, defs)
}

func checkDefinitions(defs []*Definition) error {
	seen := map[string]*Definition{}
	for _, def := range defs {
		if def == nil {
			return errors.New("a nil plugin definition")
		}
		if other := seen[def.name]; other != nil {
			return fmt.Errorf("two plugins are named %s: in %s and in %s", def.name, other.pkg, def.pkg)
		}
		seen[def.name] = def
	}
	if len(defs) == 0 {
		return errors.New("no plugins to serve")
	}
	return nil
}

// Manifest is what the plugins `defs` declare, as --rutis-manifest prints
// it.
func Manifest(defs ...*Definition) ([]byte, error) {
	plugins := map[string]any{}
	for _, def := range defs {
		plugins[def.name] = def.describe()
	}
	return json.Marshal(map[string]any{
		"manifest":  1,
		"sdk":       Version,
		"pluginApi": PluginAPI,
		"runtime":   marker,
		"platform":  goruntime.GOOS + "/" + goruntime.GOARCH,
		"plugins":   plugins,
	})
}

func parseArgs(args []string) (channel string, endpoint *peer.Endpoint, dir string, err error) {
	if len(args) == 0 {
		return "", nil, "", errors.New(usage)
	}
	channel = args[0]
	rest := args[1:]
	flags := map[string]string{}
	for len(rest) > 0 && strings.HasPrefix(rest[0], "--") {
		flag := strings.TrimPrefix(rest[0], "--")
		if len(rest) < 2 {
			return "", nil, "", fmt.Errorf("--%s needs a value", flag)
		}
		if flag != "id" && flag != "peer" {
			return "", nil, "", fmt.Errorf("unknown flag --%s; %s", flag, usage)
		}
		flags[flag] = rest[1]
		rest = rest[2:]
	}
	if len(rest) != 1 {
		return "", nil, "", errors.New(usage)
	}
	if strings.HasPrefix(channel, "listen:") || strings.HasPrefix(channel, "dial:") {
		if flags["id"] == "" {
			return "", nil, "", fmt.Errorf("a network channel needs --id <endpoint>: %s", channel)
		}
		// A runner is a runtime: the controller manages its rows.
		endpoint = &peer.Endpoint{Local: flags["id"], Expected: flags["peer"], Declare: []string{"runtime"}}
	}
	if strings.HasPrefix(channel, "dial:") {
		// What retrying cannot fix fails here, before any dial.
		if _, err := dialHost(strings.TrimPrefix(channel, "dial:")); err != nil {
			return "", nil, "", err
		}
		if !peer.ValidID(flags["id"]) {
			return "", nil, "", fmt.Errorf("invalid endpoint id %s: use lower-case letters, digits and -", flags["id"])
		}
	}
	return channel, endpoint, rest[0], nil
}

func openChannel(spec string) (net.Conn, error) {
	switch {
	case strings.HasPrefix(spec, "fd:"):
		fd, err := strconv.Atoi(strings.TrimPrefix(spec, "fd:"))
		if err != nil {
			return nil, fmt.Errorf("invalid channel %s", spec)
		}
		file := os.NewFile(uintptr(fd), "rutis-channel")
		if file == nil {
			return nil, fmt.Errorf("no file descriptor %d", fd)
		}
		defer file.Close()
		return net.FileConn(file)
	case strings.HasPrefix(spec, "tcp:"):
		address := strings.TrimPrefix(spec, "tcp:")
		token := os.Getenv(channelToken)
		// Spent once connected: what this process starts does not inherit it.
		os.Unsetenv(channelToken)
		if token == "" {
			return nil, fmt.Errorf("%s is not set for %s", channelToken, spec)
		}
		conn, err := net.Dial("tcp", address)
		if err != nil {
			return nil, err
		}
		if tcp, ok := conn.(*net.TCPConn); ok {
			_ = tcp.SetNoDelay(true)
		}
		if _, err := conn.Write([]byte(token + "\n")); err != nil {
			conn.Close()
			return nil, err
		}
		return conn, nil
	}
	return net.Dial("unix", strings.TrimPrefix(spec, "unix:"))
}

// session is one controller's session and the rows it loaded.
type session struct {
	peer    *peer.Peer
	runtime *runtimeState
}

func startSession(channel peer.Channel, endpoint *peer.Endpoint, defs []*Definition) (*session, error) {
	rt := newRuntime(defs)
	p, err := peer.New(channel, peer.Options{
		Dispatch:       rt.dispatch,
		Arrive:         rt.arrive,
		Endpoint:       endpoint,
		Implementation: map[string]string{"name": Implementation, "version": Version},
		Capabilities:   append([]string{"signals", "reentrant-sync", "sync-wait", peer.EventsCapability}, peer.ValueCapabilities...),
		// A host older than 0.13 is refused right after its handshake (#464 §2.2).
		Require: []string{peer.EventsCapability},
		Refuse:  func(string) string { return oldEventsHost },
	})
	if err != nil {
		return nil, err
	}
	rt.session = p
	if err := p.Start(); err != nil {
		return nil, err
	}
	return &session{peer: p, runtime: rt}, nil
}

// end ends the lease: nothing more is read, then every row goes, within
// endedBudget. It fails when cleanups were given up.
func (s *session) end(reason error) error {
	s.peer.Close(reason)
	s.runtime.mu.Lock()
	s.runtime.closing = true
	s.runtime.mu.Unlock()
	budget := endedBudget()
	ctx, cancel := context.WithTimeout(context.Background(), budget)
	defer cancel()
	if s.runtime.dispose(ctx) != nil {
		return fmt.Errorf("cleanups did not finish within %v after the session ended", budget)
	}
	return nil
}

// serveChannel serves one session, then unloads its rows; Serve exits the
// process after, whatever its plugins left running.
func serveChannel(channel peer.Channel, endpoint *peer.Endpoint, defs []*Definition) error {
	s, err := startSession(channel, endpoint, defs)
	if err != nil {
		// Refused during the handshake Start already finished.
		if refused := refusal(err); refused != nil {
			return refused
		}
		return err
	}
	<-s.peer.Closed()
	ended := s.end(nil)
	if refused := refusal(s.peer.Err()); refused != nil {
		return refused
	}
	return ended
}

// oldEventsHost is why a host that does not declare events-v2 is refused.
const oldEventsHost = "this host does not declare events-v2 (the event protocol of rutis 0.13): upgrade rutis-host to 0.13 or later"

// refusal is the *refusedError a session ended with when this side refused
// the far end at its handshake, else nil.
func refusal(err error) error {
	var refused *peer.RefusedError
	if errors.As(err, &refused) {
		return &refusedError{refused.Reason}
	}
	return nil
}
