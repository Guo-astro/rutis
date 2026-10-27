package rutis

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"slices"
	"strconv"
	"strings"

	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/events"
	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// Events (#464, docs/design-events-2026-10-18.md): every listener on a
// host is on one logical bus. A listener this process registers is
// registered with the host too; an emission whose listeners are all in
// this process is dispatched here (the fast path), any other by the host.
// The result is the same either way.

// AggregateError reports every failed listener of a Parallel dispatch, in
// registration order, even when only one failed. Unwrap lets errors.Is/As
// inspect each failure.
type AggregateError = peer.AggregateError

// Listener is what the bus says of a listener, as a Filter predicate sees
// it. Empty strings are absent values: Row is empty for a listener of no
// row, Runtime for one of the host, Node for one of this node.
type Listener = events.Listener

// Scope is one loader instance (a group copy, such as a tenant or a
// session) of a scope chain, outermost first.
type Scope = events.Scope

// Next is the rest of a waterfall: a waterfall listener gets it as its last
// argument, and calls it at most once.
type Next = events.Next

// Terminal is a waterfall's innermost step: `next` of the last listener
// calls it with the event's arguments; its result is that `next`'s.
type Terminal = events.Terminal

// The names of the event model's errors, for errors.Is: they match an
// error of that name made here or received from another process.
var (
	// ErrReservedEvent: a plugin emitted a `rutis/*` event.
	ErrReservedEvent = events.ErrReservedEvent
	// ErrWaterfallNext: `next` called twice, or after its waterfall ended.
	ErrWaterfallNext = events.ErrWaterfallNext
	// ErrAsyncListener: a Bail reached an asynchronous listener of another
	// language.
	ErrAsyncListener = events.ErrAsyncListener
	// ErrBinding: a transport failure, a withdrawn listener, a refused
	// frame.
	ErrBinding = events.ErrBinding
)

// ListenOption adjusts On.
type ListenOption func(*events.ListenOptions)

// Prepend puts the listener before every listener registered so far,
// instead of after them.
func Prepend() ListenOption { return func(o *events.ListenOptions) { o.Prepend = true } }

// Global exempts the listener from emitters' filters (Filter).
func Global() ListenOption { return func(o *events.ListenOptions) { o.Global = true } }

type listener struct {
	rt      *runtimeState
	id      string
	name    string
	row     *row
	fn      func(context.Context, ...any) (any, error)
	prepend bool
	global  bool
	// Its position: the order of the listeners of one name (rt.mu).
	position int64
	// Withdrawn (rt.mu): no invocation starts any more.
	withdrawn bool
	queue     events.Queue
}

func (l *listener) Info() Listener {
	l.rt.mu.Lock()
	runtime := l.rt.runtimeName
	l.rt.mu.Unlock()
	// A copy of the scope: a filter changing it changes nothing else.
	return Listener{Name: l.name, Plugin: l.row.plugin(), Row: l.row.name, Runtime: runtime, Scope: slices.Clone(l.row.scope), Global: l.global}
}

// live reports whether invocations may start; rt.mu is held.
func (l *listener) live() bool {
	return !l.withdrawn && l.rt.rows[l.row.key] == l.row
}

// reserve counts an invocation in the row's listeners, which unload waits
// for, unless the listener was withdrawn.
func (l *listener) reserve() bool {
	l.rt.mu.Lock()
	defer l.rt.mu.Unlock()
	if !l.live() {
		return false
	}
	l.row.listening.Add(1)
	return true
}

func (l *listener) Begin() (events.Call, bool) {
	if !l.reserve() {
		return nil, false
	}
	return func(ctx context.Context, mode events.Mode, args []any, next events.Next) (any, error) {
		defer l.row.listening.Done()
		return l.run(ctx, mode, args, next)
	}, true
}

func (l *listener) Emit(ctx context.Context, args []any) bool {
	if !l.reserve() {
		return false
	}
	l.queue.Push(func() {
		defer l.row.listening.Done()
		l.rt.mu.Lock()
		live := l.live()
		l.rt.mu.Unlock()
		if !live {
			return
		}
		if _, err := l.run(ctx, events.ModeEmit, args, nil); err != nil {
			l.failed(err)
		}
	})
	return true
}

// failed reports an `emit` invocation's failure where the listener is
// (§5.6).
func (l *listener) failed(err error) {
	// Cancelled because its row unloads (§7.1): not a failure.
	if errors.Is(err, context.Canceled) && l.row.ctx.Err() != nil {
		return
	}
	events.Report(fmt.Sprintf("rutis: listener of %s in row %s failed: %s", l.name, l.row.name, events.Failure(err)))
}

// run calls the listener: its context is the caller's, cancelled too when
// the row unloads.
func (l *listener) run(caller context.Context, mode events.Mode, args []any, next events.Next) (any, error) {
	ctx, cancel := context.WithCancel(caller)
	stop := context.AfterFunc(l.row.ctx, cancel)
	defer stop()
	defer cancel()
	if l.row.ctx.Err() != nil {
		cancel()
	}
	if mode.Waterfall() {
		args = append(args[:len(args):len(args)], next)
	} else {
		args = append([]any(nil), args...)
	}
	return peer.Protect(func() (any, error) { return l.fn(ctx, args...) })
}

// On listens to the event `name` on the bus until this row unloads or the
// returned disposer is called; each registration is independent. The
// listener gets the event's arguments, and in a waterfall `next` (a Next)
// after them. Its result counts in Serial, Bail and Waterfall: nil and
// false are no result. Its context is the emitter's, cancelled when the
// emitter gives up or the row unloads; unloading waits for it (up to the
// shutdown deadline). `internal/*` names, a nil listener and an inactive
// row panic, as Provide does for invalid registrations (in Apply, loading
// fails). `rutis/*` events can be listened to.
func (c *Ctx) On(name string, fn func(ctx context.Context, args ...any) (any, error), options ...ListenOption) func() {
	if err := events.CheckListen(name); err != nil {
		panic(err)
	}
	if fn == nil {
		panic("rutis: On needs a listener")
	}
	var o events.ListenOptions
	for _, option := range options {
		option(&o)
	}
	rt := c.rt
	rt.mu.Lock()
	// An unloaded row's task listening: reported, not a panic (a plain
	// goroutine or timer calling it recovers nothing), as Provide.
	if rt.rows[c.row.key] != c.row {
		rt.mu.Unlock()
		fmt.Fprintf(rowOutput, "rutis: row %s is not active: listener of %s is not registered\n", c.row.name, name)
		return func() {}
	}
	rt.listenerSeq++
	l := &listener{rt: rt, id: strconv.FormatInt(rt.listenerSeq, 10), name: name, row: c.row, fn: fn, prepend: o.Prepend, global: o.Global}
	list := rt.listeners[name]
	if o.Prepend {
		rt.first--
		l.position = rt.first
		list = append([]*listener{l}, list...)
	} else {
		rt.last++
		l.position = rt.last
		list = append(list, l)
	}
	rt.listeners[name] = list
	rt.byID[l.id] = l
	// The host hears of it before any emission this process makes after
	// it (queued in order, sent before On returns); a row loading answers
	// once the host has it, and fails when the host refuses it (§4.4).
	// Refused outside a load, it is reported (§11).
	if rt.session != nil && !rt.closing {
		session, id, loading := rt.session, c.row.name, c.row.loading
		registration := []any{map[string]any{
			"id": l.id, "name": name, "row": c.row.key, "plugin": c.row.plugin(),
			"kind": "sync", "prepend": o.Prepend, "global": o.Global,
		}}
		answered := make(chan struct{})
		// Set before answered closes.
		var refused error
		rt.post(func() {
			relay(session.AnnounceReporting("", "events.on", registration, func(err error) {
				if loading {
					refused = err
					return
				}
				events.Report(fmt.Sprintf("rutis: registration of listener of %s in row %s failed: %s", name, id, events.Failure(err)))
			}), answered)
		})
		if loading {
			c.row.pending = append(c.row.pending, func() error { <-answered; return refused })
		}
	}
	rt.mu.Unlock()
	rt.flush()
	return func() {
		rt.mu.Lock()
		if !l.withdrawn {
			rt.withdraw(l)
			if rt.session != nil && !rt.closing {
				session := rt.session
				rt.post(func() { session.Notify("", "events.off", []any{[]any{l.id}}) })
			}
		}
		rt.mu.Unlock()
		rt.flush()
	}
}

// withdraw takes a listener off the table; rt.mu is held.
func (rt *runtimeState) withdraw(l *listener) {
	l.withdrawn = true
	delete(rt.byID, l.id)
	list := slices.DeleteFunc(rt.listeners[l.name], func(other *listener) bool { return other == l })
	if len(list) == 0 {
		delete(rt.listeners, l.name)
	} else {
		rt.listeners[l.name] = list
	}
}

// forgetListeners withdraws a row's listeners, before its context is
// cancelled; rt.mu is held, and events.off is queued (post). A dispatch in progress keeps its snapshot but
// starts none of them; those running see cancellation, and unload waits
// for them. The host, which withdraws them itself on rows.unload, hears of
// it for the other unloads (a failed load, an update).
func (rt *runtimeState) forgetListeners(r *row) {
	var ids []any
	for _, l := range rt.byID {
		if l.row == r {
			ids = append(ids, l.id)
		}
	}
	for _, id := range ids {
		rt.withdraw(rt.byID[id.(string)])
	}
	if len(ids) != 0 && rt.session != nil && !rt.closing {
		session := rt.session
		rt.post(func() { session.Notify("", "events.off", []any{ids}) })
	}
}

// Emitter emits events, filtered: Ctx.Filter makes one.
type Emitter struct {
	c    *Ctx
	keep func(Listener) (bool, error)
}

// Filter is an emitter whose dispatches reach only the listeners `keep`
// accepts (listeners registered with Global excepted). `keep` runs in
// this process, once per listener, in registration order, before any
// listener runs. An error `keep` returns (or a panic) fails the dispatch
// before any listener runs, with that error (§6).
func (c *Ctx) Filter(keep func(Listener) (bool, error)) *Emitter {
	if keep == nil {
		panic("rutis: Filter needs a predicate")
	}
	return &Emitter{c: c, keep: keep}
}

// Scope is the chain of loader instances this row runs in, outermost
// first; empty at the root. It is a descriptor for filters; event
// delivery is by name on the one bus.
func (c *Ctx) Scope() []Scope { return append([]Scope(nil), c.row.scope...) }

// Emit starts every listener of `name` and returns at once. A listener's
// failure never reaches the emitter: it is reported where the listener is.
// The error is only for an emission refused outright (a reserved name,
// arguments that cannot cross, an inactive row). Pass the context your
// caller gave you: listeners keep its call chain, not its cancellation.
func (c *Ctx) Emit(ctx context.Context, name string, args ...any) error {
	return (&Emitter{c: c}).Emit(ctx, name, args...)
}

// Parallel starts every listener of `name` and waits for all. When any
// failed, the error is an *AggregateError with the failures in
// registration order. Cancelling ctx gives up: listeners see their context
// cancelled.
func (c *Ctx) Parallel(ctx context.Context, name string, args ...any) error {
	return (&Emitter{c: c}).Parallel(ctx, name, args...)
}

// Serial runs the listeners of `name` one at a time and returns the first
// result (anything but nil and false); a failure stops it and is returned.
func (c *Ctx) Serial(ctx context.Context, name string, args ...any) (any, error) {
	return (&Emitter{c: c}).Serial(ctx, name, args...)
}

// Waterfall calls the outermost listener of `name`, which gets `next`
// (a Next) as its last argument: `next` runs the rest of the chain, the
// innermost `terminal`, called with `args` (nil: no result). It returns
// the outermost listener's value as it is. A listener not calling `next`
// vetoes the rest.
func (c *Ctx) Waterfall(ctx context.Context, name string, terminal Terminal, args ...any) (any, error) {
	return (&Emitter{c: c}).Waterfall(ctx, name, terminal, args...)
}

// Bail is Serial, as a synchronous call: listeners elsewhere run as
// synchronous calls (a wait cycle fails with SyncWaitCycle), and an
// asynchronous listener of another language fails it with AsyncListener.
func (c *Ctx) Bail(ctx context.Context, name string, args ...any) (any, error) {
	return (&Emitter{c: c}).Bail(ctx, name, args...)
}

// Emit is Ctx.Emit, filtered.
func (e *Emitter) Emit(ctx context.Context, name string, args ...any) error {
	_, err := e.dispatch(ctx, name, events.ModeEmit, args, nil)
	return err
}

// Parallel is Ctx.Parallel, filtered.
func (e *Emitter) Parallel(ctx context.Context, name string, args ...any) error {
	_, err := e.dispatch(ctx, name, events.ModeParallel, args, nil)
	return err
}

// Serial is Ctx.Serial, filtered.
func (e *Emitter) Serial(ctx context.Context, name string, args ...any) (any, error) {
	return e.dispatch(ctx, name, events.ModeSerial, args, nil)
}

// Waterfall is Ctx.Waterfall, filtered.
func (e *Emitter) Waterfall(ctx context.Context, name string, terminal Terminal, args ...any) (any, error) {
	return e.dispatch(ctx, name, events.ModeWaterfall, args, terminal)
}

// Bail is Ctx.Bail, filtered.
func (e *Emitter) Bail(ctx context.Context, name string, args ...any) (any, error) {
	return e.dispatch(ctx, name, events.ModeBail, args, nil)
}

// keepOf is the filter as the dispatcher asks it: the error it returns, or
// a panic, is its failure.
func keepOf(keep func(Listener) (bool, error)) func(Listener) (bool, error) {
	if keep == nil {
		return nil
	}
	return func(info Listener) (bool, error) {
		kept, err := peer.Protect(func() (any, error) { return keep(info) })
		if err != nil {
			return false, err
		}
		return kept.(bool), nil
	}
}

func (e *Emitter) dispatch(ctx context.Context, name string, mode events.Mode, args []any, terminal Terminal) (any, error) {
	if ctx == nil {
		panic("rutis: a nil context: pass the context.Context your caller gave you")
	}
	if err := events.CheckEmit(name); err != nil {
		return nil, err
	}
	c, rt := e.c, e.c.rt
	rt.mu.Lock()
	r := c.row
	if rt.rows[r.key] != r {
		rt.mu.Unlock()
		return nil, peer.Errorf("BindingError", "row %s is not loaded in this runtime", r.name)
	}
	if rt.viaHost(name, mode) {
		return rt.dispatchViaHost(ctx, r, name, mode, args, terminal, e.keep)
	}
	// The fast path (§5.2): the snapshot of this process's listeners.
	d := &events.Dispatch{Name: name, Keep: keepOf(e.keep)}
	for _, l := range rt.listeners[name] {
		d.Entries = append(d.Entries, l)
	}
	rt.mu.Unlock()
	switch mode {
	case events.ModeEmit:
		// Not cancelled with the emitter: it does not wait.
		if err := d.Emit(context.WithoutCancel(ctx), args); err != nil {
			events.Report(fmt.Sprintf("rutis: filter of %s from row %s failed: %s", name, r.name, events.Failure(err)))
		}
		return nil, nil
	case events.ModeParallel:
		return nil, d.Parallel(ctx, args)
	case events.ModeWaterfall:
		return d.Waterfall(ctx, mode, args, terminal)
	default:
		return d.Serial(ctx, mode, args)
	}
}

// viaHost reports whether an emission of `name` goes through the host
// (§5.2): a listener of it may be elsewhere, or an earlier emit of it
// through the host is unanswered. rt.mu is held.
func (rt *runtimeState) viaHost(name string, mode events.Mode) bool {
	// Once the session ended, nothing is elsewhere.
	if rt.session == nil || rt.closing || rt.session.Err() != nil {
		return false
	}
	if rt.elsewhere[name] {
		return true
	}
	for prefix := range rt.elsewherePrefixes {
		if strings.HasPrefix(name, prefix) {
			return true
		}
	}
	return mode == events.ModeEmit && rt.hostEmits[name] > 0
}

// dispatchViaHost sends `events.dispatch`; rt.mu is held, and released.
// The frame is queued under rt.mu (post), behind this process's earlier
// frames, such as the events.on of a listener registered before (§4.4).
func (rt *runtimeState) dispatchViaHost(ctx context.Context, r *row, name string, mode events.Mode, args []any, terminal Terminal, keep func(Listener) (bool, error)) (any, error) {
	session := rt.session
	converted, err := convert.ToPeerArgs(args)
	if err != nil {
		rt.mu.Unlock()
		return nil, err
	}
	record := map[string]any{"row": r.key, "name": name, "mode": string(mode), "args": converted}
	if keep != nil {
		record["filter"] = selector(keepOf(keep))
	}
	if terminal != nil && mode == events.ModeWaterfall {
		// The host calls it with the event's arguments.
		record["terminal"] = peer.Func(func(ctx context.Context, args []any) (any, error) {
			plain := make([]any, len(args))
			for i, arg := range args {
				var err error
				if plain[i], err = convert.Plain(arg); err != nil {
					return nil, err
				}
			}
			value, err := terminal(ctx, plain...)
			if err != nil {
				return nil, err
			}
			return convert.ToPeer(value)
		})
	}
	type sent struct {
		wait func() (any, error)
		err  error
	}
	result := make(chan sent, 1)
	if mode == events.ModeEmit {
		// Queued now, in order with this process's other frames; its later
		// emits of `name` follow it through the host until it is answered
		// (§5.5). The emitter does not wait, nor cancel it.
		rt.hostEmits[name]++
		answered := func(err error) {
			rt.mu.Lock()
			if rt.hostEmits[name]--; rt.hostEmits[name] <= 0 {
				delete(rt.hostEmits, name)
			}
			rt.mu.Unlock()
			if err != nil {
				events.Report(fmt.Sprintf("rutis: emit of %s from row %s failed: %s", name, r.name, events.Failure(err)))
			}
		}
		// The answer is taken in frame order (§5.5): an emit that a later
		// frame starts already sees it.
		failed := make(chan error, 1)
		rt.post(func() {
			failed <- session.SendAnswered(ctx, "", "events.dispatch", []any{record}, answered)
		})
		rt.mu.Unlock()
		rt.flush()
		if err := <-failed; err != nil {
			answered(nil)
			return nil, err
		}
		return nil, nil
	}
	rt.post(func() {
		wait, err := session.Send(ctx, "", "events.dispatch", []any{record}, mode == events.ModeBail)
		result <- sent{wait, err}
	})
	rt.mu.Unlock()
	rt.flush()
	s := <-result
	wait, err := s.wait, s.err
	if err != nil {
		return nil, err
	}
	rt.warnChainless(ctx)
	value, err := wait()
	if err != nil {
		return nil, peer.Aggregated(err)
	}
	if value, err = peer.Settle(ctx, value); err != nil {
		// A host answers a dispatch it does not finish at once with a
		// future: its failure is the dispatch's (#677).
		return nil, peer.Aggregated(err)
	}
	if mode == events.ModeParallel {
		return nil, nil
	}
	return convert.Plain(value)
}

// selector is the function the host calls with the descriptors of a
// dispatch's candidates, answering one boolean each (§6).
func selector(keep func(Listener) (bool, error)) peer.Func {
	return func(_ context.Context, args []any) (any, error) {
		var infos []Listener
		if len(args) > 0 {
			if err := convert.Decode(args[0], &infos); err != nil {
				return nil, fmt.Errorf("filter: %w", err)
			}
		}
		answers := make([]any, len(infos))
		for i, info := range infos {
			kept, err := keep(info)
			if err != nil {
				return nil, err
			}
			answers[i] = kept
		}
		return answers, nil
	}
}

// ── From the host ────────────────────────────────────────────────

// arrive takes the host's event calls on the reading goroutine, in frame
// order (§4.1, §5.5); the rest runs as any call does.
func (rt *runtimeState) arrive(ctx context.Context, method string, args []any) func() (any, error) {
	switch method {
	case "events.elsewhere":
		err := rt.elsewhereChanged(args)
		return func() (any, error) { return nil, err }
	case "events.call":
		return rt.eventCall(ctx, args)
	}
	return nil
}

func (rt *runtimeState) elsewhereChanged(args []any) error {
	change, err := argument[struct {
		Add            []string `json:"add"`
		Remove         []string `json:"remove"`
		AddPrefixes    []string `json:"addPrefixes"`
		RemovePrefixes []string `json:"removePrefixes"`
	}](args, 0)
	if err != nil {
		return fmt.Errorf("events.elsewhere: %w", err)
	}
	rt.mu.Lock()
	defer rt.mu.Unlock()
	for _, name := range change.Add {
		rt.elsewhere[name] = true
	}
	for _, name := range change.Remove {
		delete(rt.elsewhere, name)
	}
	for _, prefix := range change.AddPrefixes {
		rt.elsewherePrefixes[prefix] = true
	}
	for _, prefix := range change.RemovePrefixes {
		delete(rt.elsewherePrefixes, prefix)
	}
	return nil
}

// fields is a record received as a call argument: its fields as values.
func fields(v any) (map[string]any, error) {
	switch x := v.(type) {
	case map[string]any:
		return x, nil
	case json.RawMessage:
		var raw map[string]json.RawMessage
		if err := json.Unmarshal(x, &raw); err != nil || raw == nil {
			return nil, errors.New("not a record")
		}
		out := make(map[string]any, len(raw))
		for key, item := range raw {
			out[key] = item
		}
		return out, nil
	}
	return nil, errors.New("not a record")
}

// eventCall runs one invocation of a listener here (§5.3): an `emit` one is
// queued now, after the earlier ones of that listener (§5.5).
func (rt *runtimeState) eventCall(ctx context.Context, args []any) func() (any, error) {
	failed := func(err error) func() (any, error) { return func() (any, error) { return nil, err } }
	var record map[string]any
	var err error
	if len(args) > 0 {
		record, err = fields(args[0])
	}
	if record == nil {
		return failed(fmt.Errorf("events.call: %v", err))
	}
	var id, name, mode string
	var values []any
	for key, out := range map[string]any{"listener": &id, "name": &name, "mode": &mode} {
		if err := convert.Decode(record[key], out); err != nil {
			return failed(fmt.Errorf("events.call %s: %w", key, err))
		}
	}
	switch list := record["args"].(type) {
	case []any:
		values = list
	case nil:
	default:
		if err := convert.Decode(list, &values); err != nil {
			return failed(fmt.Errorf("events.call args: %w", err))
		}
	}
	plain := make([]any, len(values))
	for i, value := range values {
		if plain[i], err = convert.Plain(value); err != nil {
			return failed(err)
		}
	}
	m := events.Mode(mode)
	if !m.Valid() {
		return failed(fmt.Errorf("events.call: unknown mode %s", mode))
	}
	rt.mu.Lock()
	l := rt.byID[id]
	rt.mu.Unlock()
	withdrawn := peer.Errorf("BindingError", "listener %s of %s is withdrawn", id, name)
	if l == nil {
		return failed(withdrawn)
	}
	if m == events.ModeEmit {
		if !l.reserve() {
			return failed(withdrawn)
		}
		done := make(chan error, 1)
		l.queue.Push(func() {
			defer l.row.listening.Done()
			l.rt.mu.Lock()
			live := l.live()
			l.rt.mu.Unlock()
			if !live {
				done <- withdrawn
				return
			}
			if _, err := l.run(ctx, m, plain, nil); err != nil {
				l.failed(err)
			}
			done <- nil
		})
		// Answered once it ran; its failure is reported here.
		return func() (any, error) { return nil, <-done }
	}
	var next events.Next
	if m.Waterfall() {
		remote, ok := record["next"].(*peer.RemoteFunction)
		if !ok {
			return failed(fmt.Errorf("events.call: a %s needs next", mode))
		}
		next = func(ctx context.Context) (any, error) {
			value, err := remote.Call(ctx, []any{}, m == events.ModeWaterfallSync)
			if err != nil {
				return nil, err
			}
			if value, err = peer.Settle(ctx, value); err != nil {
				return nil, err
			}
			return convert.Plain(value)
		}
	}
	call, ok := l.Begin()
	if !ok {
		return failed(withdrawn)
	}
	return func() (any, error) {
		value, err := call(ctx, m, plain, next)
		if err != nil {
			return nil, err
		}
		return convert.ToPeer(value)
	}
}
