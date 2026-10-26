// The session conformance target (rutis_bridge::session::testing::session),
// served by the Go session: connects to the channel given as the first
// argument (a Unix socket path, or `tcp:<host>:<port>`, presenting the token
// in RUTIS_CHANNEL_TOKEN first) as endpoint `go`, expecting `main`, and serves `conformance`
// until the session ends.
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

type named struct{ name, message string }

func (e *named) Error() string { return e.message }
func (e *named) Name() string  { return e.name }

type conformance struct {
	mu      sync.Mutex
	held    *peer.RemoteFunction
	started bool
	aborted bool
	session *peer.Peer
}

func arg(args []any, i int) any {
	if i < len(args) {
		return args[i]
	}
	return peer.Undefined
}

func text(value any) string {
	var s string
	if raw, ok := value.(json.RawMessage); ok {
		_ = json.Unmarshal(raw, &s)
	}
	return s
}

func number(value any) int {
	var n int
	if raw, ok := value.(json.RawMessage); ok {
		_ = json.Unmarshal(raw, &n)
	}
	return n
}

// counter is an object with methods: it crosses only to a far end that
// takes objects (#675).
type counter struct{ n int }

func (c *counter) Call(ctx context.Context, method string, args []any) (any, error) {
	c.n++
	return c.n, nil
}

func (c *counter) Get(ctx context.Context, property string) (any, error) { return c.n, nil }

func (c *conformance) dispatch(ctx context.Context, target, method string, args []any) (any, error) {
	if target != "conformance" {
		return nil, fmt.Errorf("no target %s", target)
	}
	switch method {
	case "echo":
		return arg(args, 0), nil
	case "apply":
		fn, ok := arg(args, 0).(*peer.RemoteFunction)
		if !ok {
			return nil, fmt.Errorf("apply needs a function")
		}
		return fn.Call(ctx, []any{arg(args, 1)}, true)
	case "later":
		time.Sleep(5 * time.Millisecond)
		return arg(args, 0), nil
	case "fail":
		return nil, &named{text(arg(args, 0)), text(arg(args, 1))}
	case "hold":
		fn, _ := arg(args, 0).(*peer.RemoteFunction)
		c.mu.Lock()
		c.held = fn
		c.mu.Unlock()
		return peer.Undefined, nil
	case "fire":
		c.mu.Lock()
		held := c.held
		c.mu.Unlock()
		if held == nil {
			return nil, &named{"LookupError", "nothing held"}
		}
		return held.Call(ctx, []any{arg(args, 0)}, true)
	case "drop":
		c.mu.Lock()
		held := c.held
		c.held = nil
		c.mu.Unlock()
		if held != nil {
			c.session.Release(held)
		}
		return peer.Undefined, nil
	case "abortable":
		signal, _ := arg(args, 0).(*peer.Signal)
		if signal == nil {
			return nil, fmt.Errorf("abortable needs a signal")
		}
		c.mu.Lock()
		c.started = true
		c.mu.Unlock()
		select {
		case <-signal.Done():
		case <-ctx.Done():
		}
		c.mu.Lock()
		c.aborted = true
		c.mu.Unlock()
		return peer.Undefined, nil
	case "started":
		c.mu.Lock()
		defer c.mu.Unlock()
		return c.started, nil
	case "aborted":
		c.mu.Lock()
		defer c.mu.Unlock()
		return c.aborted, nil
	// The value one level deeper, returned or passed to a callback: past the
	// depth limit, this side's call or reply fails (#279).
	case "wrap":
		return []any{arg(args, 0)}, nil
	case "applyWrapped":
		fn, ok := arg(args, 0).(*peer.RemoteFunction)
		if !ok {
			return nil, fmt.Errorf("applyWrapped needs a function")
		}
		return fn.Call(ctx, []any{[]any{arg(args, 1)}}, true)
	// What cannot be sent fails only its call (#675): a result over the
	// message limit, an argument over it (this side's call of `fn`), an
	// object to a far end that does not take objects.
	case "big":
		return strings.Repeat("x", number(arg(args, 0))), nil
	case "applyBig":
		fn, ok := arg(args, 0).(*peer.RemoteFunction)
		if !ok {
			return nil, fmt.Errorf("applyBig needs a function")
		}
		return fn.Call(ctx, []any{strings.Repeat("x", number(arg(args, 1)))}, true)
	case "object":
		return &counter{}, nil
	case "reenter":
		fn, _ := arg(args, 0).(*peer.RemoteFunction)
		if fn == nil {
			return nil, fmt.Errorf("reenter needs a function")
		}
		return fn.Call(ctx, nil, true)
	}
	return nil, fmt.Errorf("no method %s", method)
}

func dial(spec string) (net.Conn, error) {
	address, ok := strings.CutPrefix(spec, "tcp:")
	if !ok {
		return net.Dial("unix", spec)
	}
	conn, err := net.Dial("tcp", address)
	if err != nil {
		return nil, err
	}
	if _, err := conn.Write([]byte(os.Getenv("RUTIS_CHANNEL_TOKEN") + "\n")); err != nil {
		conn.Close()
		return nil, err
	}
	return conn, nil
}

func main() {
	connection, err := dial(os.Args[1])
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	service := &conformance{}
	session, err := peer.New(peer.NewLines(connection), peer.Options{
		Dispatch:       service.dispatch,
		Endpoint:       &peer.Endpoint{Local: "go", Expected: "main"},
		Implementation: map[string]string{"name": "conformance", "version": "0"},
		Capabilities:   append([]string{"signals", "reentrant-sync", "sync-wait", "events-v2"}, peer.ValueCapabilities...),
	})
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	service.session = session
	if err := session.Start(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	<-session.Closed()
}
