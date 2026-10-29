// Package events holds what the runtime's side of the unified event model
// (#464, docs/design-events-protocol-2026-10-18.md) needs: descriptors,
// modes, the reserved names, the lines of §5.6 and the order of a
// listener's `emit` invocations. The host dispatches every emission.
package events

import (
	"context"
	"encoding/json"
	"fmt"
	"math"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// Scope is one loader instance of a scope chain.
type Scope struct {
	ID    string `json:"id"`
	Group string `json:"group"`
}

// Listener is what the bus says of a listener: what a filter sees. Empty
// strings are the wire's null.
type Listener struct {
	Name    string
	Prefix  bool
	Plugin  string
	Row     string
	Runtime string
	Node    string
	Scope   []Scope
	Global  bool
}

type wireListener struct {
	Name    string  `json:"name"`
	Prefix  bool    `json:"prefix"`
	Plugin  string  `json:"plugin"`
	Row     *string `json:"row"`
	Runtime *string `json:"runtime"`
	Node    *string `json:"node"`
	Scope   []Scope `json:"scope"`
	Global  bool    `json:"global"`
}

func orNull(s string) *string {
	if s == "" {
		return nil
	}
	return &s
}

func (l Listener) MarshalJSON() ([]byte, error) {
	scope := l.Scope
	if scope == nil {
		scope = []Scope{}
	}
	return json.Marshal(wireListener{l.Name, l.Prefix, l.Plugin, orNull(l.Row), orNull(l.Runtime), orNull(l.Node), scope, l.Global})
}

func (l *Listener) UnmarshalJSON(data []byte) error {
	var w wireListener
	if err := json.Unmarshal(data, &w); err != nil {
		return err
	}
	deref := func(s *string) string {
		if s == nil {
			return ""
		}
		return *s
	}
	*l = Listener{w.Name, w.Prefix, w.Plugin, deref(w.Row), deref(w.Runtime), deref(w.Node), w.Scope, w.Global}
	return nil
}

// ListenOptions are a listener's registration options (§4.2).
type ListenOptions struct{ Prepend, Global bool }

// Next is the rest of a waterfall.
type Next = func(context.Context) (any, error)

// Terminal is a waterfall's innermost step: `next` of the last listener
// calls it with the event's arguments (§5.4).
type Terminal = func(ctx context.Context, args ...any) (any, error)

// Mode is how a dispatch runs its listeners.
type Mode string

const (
	ModeEmit      Mode = "emit"
	ModeParallel  Mode = "parallel"
	ModeSerial    Mode = "serial"
	ModeWaterfall Mode = "waterfall"
	ModeBail      Mode = "bail"
)

// Valid reports whether m is a mode of the protocol.
func (m Mode) Valid() bool {
	switch m {
	case ModeEmit, ModeParallel, ModeSerial, ModeWaterfall, ModeBail:
		return true
	}
	return false
}

// Call runs one reserved invocation of an entry: with the event's arguments
// and, in a waterfall, `next`.
type Call func(ctx context.Context, mode Mode, args []any, next Next) (any, error)

// The names of the event model's errors (§11).
var (
	ErrReservedEvent = peer.NewSentinel("ReservedEvent")
	ErrWaterfallNext = peer.NewSentinel("WaterfallNext")
	ErrAsyncListener = peer.NewSentinel("AsyncListener")
	ErrBinding       = peer.NewSentinel("BindingError")
)

// CheckEmit refuses what no plugin may emit: `internal/*` (the runtime's
// own) and `rutis/*` (rutis's own, ReservedEvent).
func CheckEmit(name string) error {
	if strings.HasPrefix(name, "internal/") {
		return fmt.Errorf("event %s is internal to the runtime", name)
	}
	if strings.HasPrefix(name, "rutis/") {
		return peer.Errorf("ReservedEvent", "rutis/* events are emitted by rutis only: %s", name)
	}
	return nil
}

// CheckListen refuses what no leaf plugin may listen to: `internal/*`.
func CheckListen(name string) error {
	if strings.HasPrefix(name, "internal/") {
		return fmt.Errorf("event %s is internal to the runtime", name)
	}
	return nil
}

// ShutdownVariable sets, in seconds, how long an unload waits for its
// row's running listeners (§7.1) and how long cleanups may take once the
// session ended; rutis-host passes its own shutdown deadline on.
const ShutdownVariable = "RUTIS_SHUTDOWN_TIMEOUT"

// DefaultShutdown is that time when ShutdownVariable does not say.
const DefaultShutdown = 10 * time.Second

// ShutdownTimeout is the deadline ShutdownVariable sets.
func ShutdownTimeout() time.Duration {
	seconds, err := strconv.ParseFloat(strings.TrimSpace(os.Getenv(ShutdownVariable)), 64)
	if err != nil || !(seconds > 0) || math.IsInf(seconds, 1) {
		return DefaultShutdown
	}
	return time.Duration(seconds * float64(time.Second))
}

// report writes the lines of §5.6; tests replace it.
var (
	reportMu sync.Mutex
	report   = func(line string) { fmt.Fprintln(os.Stderr, line) }
)

// Report writes one line of §5.6.
func Report(line string) {
	reportMu.Lock()
	write := report
	reportMu.Unlock()
	write(line)
}

// SetReport replaces where the lines of §5.6 go, returning the previous.
func SetReport(write func(string)) func(string) {
	reportMu.Lock()
	defer reportMu.Unlock()
	previous := report
	report = write
	return previous
}

// Failure is an error as the lines of §5.6 show it: `<ErrorName>: <message>`.
func Failure(err error) string {
	return peer.ErrorName(err) + ": " + peer.Message(err)
}

// Queue runs functions one after another, in the order given (§5.5: the
// `emit` invocations of one listener).
type Queue struct {
	mu      sync.Mutex
	pending []func()
	running bool
}

// Push runs `f` after every function pushed before it.
func (q *Queue) Push(f func()) {
	q.mu.Lock()
	q.pending = append(q.pending, f)
	if q.running {
		q.mu.Unlock()
		return
	}
	q.running = true
	q.mu.Unlock()
	go func() {
		for {
			q.mu.Lock()
			if len(q.pending) == 0 {
				q.running = false
				q.mu.Unlock()
				return
			}
			f := q.pending[0]
			q.pending[0] = nil
			q.pending = q.pending[1:]
			q.mu.Unlock()
			f()
		}
	}()
}
