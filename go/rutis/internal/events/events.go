// Package events is the dispatch of the unified event model (#464,
// docs/design-events-protocol-2026-10-18.md §5): the five modes over a
// snapshot of entries, wherever each entry runs. The runtime dispatches its
// fast path with it (every entry a listener of its own process); rutistest
// dispatches as a host with it (entries in the runtime it tests, reached by
// `events.call`, and the test's own).
package events

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"reflect"
	"strings"
	"sync"
	"sync/atomic"

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

// Next is the rest of a waterfall, and a waterfall's terminal.
type Next = func(context.Context) (any, error)

// Mode is how a dispatch runs its listeners.
type Mode string

const (
	ModeEmit          Mode = "emit"
	ModeParallel      Mode = "parallel"
	ModeSerial        Mode = "serial"
	ModeWaterfall     Mode = "waterfall"
	ModeBail          Mode = "bail"
	ModeWaterfallSync Mode = "waterfall-sync"
)

// Valid reports whether m is a mode of the protocol.
func (m Mode) Valid() bool {
	switch m {
	case ModeEmit, ModeParallel, ModeSerial, ModeWaterfall, ModeBail, ModeWaterfallSync:
		return true
	}
	return false
}

// Waterfall reports whether listeners get `next` in this mode.
func (m Mode) Waterfall() bool { return m == ModeWaterfall || m == ModeWaterfallSync }

// Call runs one reserved invocation of an entry: with the event's arguments
// and, in a waterfall, `next`.
type Call func(ctx context.Context, mode Mode, args []any, next Next) (any, error)

// Entry is one listener of a dispatch's snapshot.
type Entry interface {
	Info() Listener
	// Begin reserves an invocation, unless the entry was withdrawn: then
	// it is skipped, as if it had never been in the snapshot.
	Begin() (Call, bool)
	// Emit starts an `emit` invocation after the earlier ones of this
	// entry (§5.5), unless the entry was withdrawn. Its failure is
	// reported by the entry, never returned.
	Emit(ctx context.Context, args []any) bool
}

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

// Within reports whether a listener in the scope `listener` hears what is
// emitted in `emitter`: the listener's chain is a prefix of the emitter's.
func Within(listener, emitter []Scope) bool {
	if len(listener) > len(emitter) {
		return false
	}
	for i := range listener {
		if listener[i].ID != emitter[i].ID {
			return false
		}
	}
	return true
}

// HasResult reports whether v is a result: not nil, undefined, null or
// false (§1), as a Go value or as a value received from a session.
func HasResult(v any) bool {
	switch x := v.(type) {
	case nil, peer.UndefinedType, *peer.Signal:
		return false
	case bool:
		return x
	case json.RawMessage:
		text := string(bytes.TrimSpace(x))
		return text != "null" && text != "false" && text != ""
	}
	value := reflect.ValueOf(v)
	switch value.Kind() {
	case reflect.Pointer, reflect.Map, reflect.Slice, reflect.Interface, reflect.Func, reflect.Chan:
		return !value.IsNil()
	}
	return true
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

// Dispatch is one dispatch: the snapshot, in registration order, already
// limited to the emitter's scope (§5.1 step 1).
type Dispatch struct {
	Name    string
	Entries []Entry
	// Keep is the filter (§6), nil for none: asked once per non-global
	// entry, in order, before any invocation starts.
	Keep func(Listener) (bool, error)
}

// selected applies the filter (§5.1 step 2).
func (d *Dispatch) selected() ([]Entry, error) {
	if d.Keep == nil {
		return d.Entries, nil
	}
	kept := make([]Entry, 0, len(d.Entries))
	for _, entry := range d.Entries {
		info := entry.Info()
		if !info.Global {
			keep, err := d.Keep(info)
			if err != nil {
				return nil, err
			}
			if !keep {
				continue
			}
		}
		kept = append(kept, entry)
	}
	return kept, nil
}

// Emit starts every invocation in order and waits for none. A filter's
// failure is returned, for the dispatcher to report (§5.6).
func (d *Dispatch) Emit(ctx context.Context, args []any) error {
	entries, err := d.selected()
	if err != nil {
		return err
	}
	for _, entry := range entries {
		entry.Emit(ctx, args)
	}
	return nil
}

// Parallel starts every invocation in order and waits for all; failures
// are an *AggregateError in registration order, even a single one. The
// emitter giving up (ctx) stops starting invocations and returns at once;
// those running see ctx cancelled.
func (d *Dispatch) Parallel(ctx context.Context, args []any) error {
	entries, err := d.selected()
	if err != nil {
		return err
	}
	errs := make([]error, len(entries))
	var running sync.WaitGroup
	for i, entry := range entries {
		if ctx.Err() != nil {
			break
		}
		call, ok := entry.Begin()
		if !ok {
			continue
		}
		running.Add(1)
		go func() {
			defer running.Done()
			_, errs[i] = call(ctx, ModeParallel, args, nil)
		}()
	}
	done := make(chan struct{})
	go func() { running.Wait(); close(done) }()
	select {
	case <-done:
	case <-ctx.Done():
	}
	if ctx.Err() != nil {
		return ctx.Err()
	}
	var failed []error
	for _, err := range errs {
		if err != nil {
			failed = append(failed, err)
		}
	}
	if len(failed) != 0 {
		return &peer.AggregateError{Message: fmt.Sprintf("%d listeners of %s failed", len(failed), d.Name), Errors: failed}
	}
	return nil
}

// Serial runs one invocation at a time and returns the first result; a
// failure stops it. `mode` is serial or bail.
func (d *Dispatch) Serial(ctx context.Context, mode Mode, args []any) (any, error) {
	entries, err := d.selected()
	if err != nil {
		return nil, err
	}
	for _, entry := range entries {
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		call, ok := entry.Begin()
		if !ok {
			continue
		}
		value, err := call(ctx, mode, args, nil)
		if err != nil {
			return nil, err
		}
		if HasResult(value) {
			return value, nil
		}
	}
	return nil, nil
}

// Waterfall calls the outermost entry with `next`, each `next` calling
// the entry after it and the last one `terminal` (nil: no result), and
// returns the outermost value as it is. `mode` is waterfall or
// waterfall-sync.
func (d *Dispatch) Waterfall(ctx context.Context, mode Mode, args []any, terminal Next) (any, error) {
	entries, err := d.selected()
	if err != nil {
		return nil, err
	}
	var ended atomic.Bool
	defer ended.Store(true)
	var run func(context.Context, int) (any, error)
	next := func(from int) Next {
		var called atomic.Bool
		return func(nextCtx context.Context) (any, error) {
			if nextCtx == nil {
				nextCtx = ctx
			}
			if called.Swap(true) {
				return nil, peer.Errorf("WaterfallNext", "next of %s was already called", d.Name)
			}
			if ended.Load() || ctx.Err() != nil {
				return nil, peer.Errorf("WaterfallNext", "next of %s was called after its waterfall ended", d.Name)
			}
			return run(nextCtx, from)
		}
	}
	run = func(ctx context.Context, from int) (any, error) {
		for i := from; i < len(entries); i++ {
			call, ok := entries[i].Begin()
			if !ok {
				continue
			}
			return call(ctx, mode, args, next(i+1))
		}
		if terminal != nil {
			return terminal(ctx)
		}
		return nil, nil
	}
	return run(ctx, 0)
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
