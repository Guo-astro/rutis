// Package peer is one session of the rutis protocol: the compat format
// (version 2) local runtimes speak, and the endpoint format (version 3)
// network channels speak.
//
// Every incoming call runs on a goroutine of its own, so a synchronous call
// blocks only the goroutine that made it and any incoming call can run at
// any time: the session is reentrant. The call chain (`path`) of a call
// travels in its context.Context: an incoming call's context carries its
// chain, and outgoing calls take their chain from the context they are
// given.
package peer

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"reflect"
	"regexp"
	"runtime"
	"runtime/debug"
	"strconv"
	"strings"
	"sync"
	"weak"
)

const (
	Protocol         = 2
	EndpointProtocol = 3
	MaxSafe          = 1<<53 - 1
)

var (
	endpointID = regexp.MustCompile(`^[a-z0-9-]+$`)
	// In the compat format, this session's ids are `node:`/`rust:`; another
	// session's come tagged with it, whatever its format (`s1/node:3`, #225).
	compatOrigin   = regexp.MustCompile(`^(node|rust|s[0-9]+/[a-z0-9-]+):[1-9][0-9]*$`)
	endpointOrigin = regexp.MustCompile(`^(s[0-9]+/)?[a-z0-9-]+:[1-9][0-9]*$`)
	sequence       = regexp.MustCompile(`^[1-9][0-9]*$`)
)

// UndefinedType is the wire's `undefined`: an omitted argument or field.
type UndefinedType struct{}

var Undefined = UndefinedType{}

// Func is a function this side exports: the far end calls it.
type Func func(ctx context.Context, args []any) (any, error)

// Object is an object this side exports: the far end calls its methods and
// reads its properties.
type Object interface {
	Call(ctx context.Context, method string, args []any) (any, error)
	Get(ctx context.Context, property string) (any, error)
}

// Signal is the cancellation a call received as an argument: done once the
// caller cancels.
type Signal struct {
	done chan struct{}
	once sync.Once
}

func newSignal() *Signal { return &Signal{done: make(chan struct{})} }

func (s *Signal) Done() <-chan struct{} { return s.done }
func (s *Signal) cancel()               { s.once.Do(func() { close(s.done) }) }

// Endpoint selects the endpoint format: this side's id, the far end's
// expected id (if known), and capabilities to declare beyond the session's.
type Endpoint struct {
	Local    string
	Expected string
	Declare  []string
}

// Options configure a session.
type Options struct {
	// Dispatch serves `invoke`: target "" is a control operation.
	Dispatch func(ctx context.Context, target, method string, args []any) (any, error)
	// Endpoint selects the endpoint format; nil speaks compat.
	Endpoint *Endpoint
	// Implementation is sent in the endpoint handshake.
	Implementation map[string]string
	// Capabilities this side has (endpoint format).
	Capabilities []string
	// Host makes this side the host of a compat session (call ids `rust:`),
	// for tests that drive a runtime.
	Host bool
}

// Greeting is what the far end said of itself (endpoint format).
type Greeting struct {
	Endpoint       string
	Implementation json.RawMessage
	Capabilities   []string
}

type result struct {
	value any
	err   error
}

type export struct {
	value  any
	kind   string
	origin []string
	grants int64
}

type imported struct {
	id       int64
	kind     string
	origin   []string
	grants   int64
	released bool
	function weak.Pointer[RemoteFunction]
	future   weak.Pointer[RemoteFuture]
}

// Peer is one session.
type Peer struct {
	channel Channel
	opts    Options

	writeMu sync.Mutex
	mu      sync.Mutex

	local  string
	remote string

	next      uint64
	received  uint64
	ref       int64
	handshake bool
	greeting  *Greeting

	pending    map[string]chan result
	exports    map[int64]*export
	identities map[any]int64
	imports    map[int64]*imported
	running    map[string]context.CancelFunc
	signals    map[string]*Signal

	active   int
	draining []chan struct{}

	base       context.Context
	cancelBase context.CancelFunc
	ready      chan struct{}
	closed     chan struct{}
	closeErr   error
}

// New makes a session on `channel`; Start begins it.
func New(channel Channel, opts Options) (*Peer, error) {
	p := &Peer{
		channel:    channel,
		opts:       opts,
		pending:    map[string]chan result{},
		exports:    map[int64]*export{},
		identities: map[any]int64{},
		imports:    map[int64]*imported{},
		running:    map[string]context.CancelFunc{},
		signals:    map[string]*Signal{},
		ready:      make(chan struct{}),
		closed:     make(chan struct{}),
	}
	p.base, p.cancelBase = context.WithCancel(context.Background())
	if opts.Endpoint != nil {
		if !endpointID.MatchString(opts.Endpoint.Local) {
			return nil, fmt.Errorf("invalid endpoint id %s", opts.Endpoint.Local)
		}
		p.local = opts.Endpoint.Local + ":"
	} else if opts.Host {
		p.local = "rust:"
		p.remote = "node:"
	} else {
		p.local = "node:"
		p.remote = "rust:"
	}
	return p, nil
}

// Start sends the handshake and starts reading.
func (p *Peer) Start() error {
	go p.read()
	hello := map[string]any{"op": "hello"}
	if p.opts.Endpoint == nil {
		hello["version"] = Protocol
	} else {
		capabilities := append(append([]string{}, p.opts.Capabilities...), p.opts.Endpoint.Declare...)
		hello["version"] = EndpointProtocol
		hello["endpoint"] = p.opts.Endpoint.Local
		hello["implementation"] = p.opts.Implementation
		hello["capabilities"] = capabilities
	}
	return p.send(hello)
}

// Ready is closed once the far end greeted (or the session closed).
func (p *Peer) Ready() <-chan struct{} { return p.ready }

// Closed is closed once the session ended; Err says why.
func (p *Peer) Closed() <-chan struct{} { return p.closed }

func (p *Peer) Err() error {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.closeErr
}

// Supports reports whether the far end declared `capability`.
func (p *Peer) Supports(capability string) bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.supports(capability)
}

func (p *Peer) supports(capability string) bool {
	if p.greeting == nil {
		return false
	}
	for _, c := range p.greeting.Capabilities {
		if c == capability {
			return true
		}
	}
	return false
}

// Greeting is what the far end said of itself, once it greeted.
func (p *Peer) Greeting() *Greeting {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.greeting
}

// ── Call chains ──────────────────────────────────────────────────

type chainKey struct{}

// WithChain gives ctx the call chain `chain`.
func WithChain(ctx context.Context, chain []string) context.Context {
	return context.WithValue(ctx, chainKey{}, chain)
}

// ChainOf is the call chain ctx carries, if any.
func ChainOf(ctx context.Context) []string {
	if ctx == nil {
		return nil
	}
	chain, _ := ctx.Value(chainKey{}).([]string)
	return chain
}

// Incoming is the number of incoming calls running now.
func (p *Peer) Incoming() int {
	p.mu.Lock()
	defer p.mu.Unlock()
	return len(p.running)
}

// ── I/O ──────────────────────────────────────────────────────────

func marshal(frame any) ([]byte, error) {
	var buffer bytes.Buffer
	encoder := json.NewEncoder(&buffer)
	encoder.SetEscapeHTML(false)
	if err := encoder.Encode(frame); err != nil {
		return nil, err
	}
	return bytes.TrimRight(buffer.Bytes(), "\n"), nil
}

func (p *Peer) send(frame any) error {
	data, err := marshal(frame)
	if err != nil {
		return err
	}
	return p.write(data)
}

func (p *Peer) write(data []byte) error {
	p.writeMu.Lock()
	defer p.writeMu.Unlock()
	return p.writeLocked(data)
}

// writeLocked sends `data`; p.writeMu is held.
func (p *Peer) writeLocked(data []byte) error {
	if err := p.Err(); err != nil {
		return err
	}
	return p.channel.Send(data)
}

func (p *Peer) read() {
	for {
		message, err := p.channel.Recv()
		if err != nil {
			if errors.Is(err, io.EOF) {
				p.Close(errors.New("Rust process disconnected"))
			} else {
				p.Close(fmt.Errorf("session failed: %w", err))
			}
			return
		}
		var f frame
		if err := json.Unmarshal(message, &f); err != nil {
			p.fault(fmt.Errorf("session failed: %w", err))
			return
		}
		if err := p.receive(&f); err != nil {
			p.fault(err)
			return
		}
	}
}

// Close ends the session with `err`.
func (p *Peer) Close(err error) {
	p.mu.Lock()
	if p.closeErr != nil {
		p.mu.Unlock()
		return
	}
	if err == nil {
		err = errors.New("session closed")
	}
	p.closeErr = err
	pending := p.pending
	p.pending = map[string]chan result{}
	p.exports = map[int64]*export{}
	p.identities = map[any]int64{}
	p.imports = map[int64]*imported{}
	draining := p.draining
	p.draining = nil
	select {
	case <-p.ready:
	default:
		close(p.ready)
	}
	p.mu.Unlock()
	for _, waiter := range pending {
		waiter <- result{err: err}
	}
	for _, waiter := range draining {
		close(waiter)
	}
	p.cancelBase()
	close(p.closed)
}

func (p *Peer) fault(err error) {
	p.Close(err)
	p.channel.Close(err.Error())
}

// ── Values ───────────────────────────────────────────────────────

// RemoteFunction is a function of the far end.
type RemoteFunction struct {
	peer   *Peer
	record *imported
}

// RemoteFuture is an asynchronous result of the far end.
type RemoteFuture struct {
	peer   *Peer
	record *imported
	mu     sync.Mutex
	done   bool
	result result
}

func (r *imported) check() error {
	if r.released {
		return errors.New("reference released")
	}
	return nil
}

// Call calls the function: synchronously (the far end may call back on
// this chain while it runs) or as an asynchronous call, which ctx cancels.
func (f *RemoteFunction) Call(ctx context.Context, args []any, sync bool) (any, error) {
	p := f.peer
	p.mu.Lock()
	err := f.record.check()
	p.mu.Unlock()
	if err != nil {
		return nil, err
	}
	value, err := p.call(ctx, "call", map[string]any{"reference": f.record.id}, args, true, nil, !sync)
	if err != nil || sync {
		return value, err
	}
	return Settle(ctx, value)
}

// Await waits for the result; ctx cancels the wait.
func (f *RemoteFuture) Await(ctx context.Context) (any, error) {
	f.mu.Lock()
	if f.done {
		f.mu.Unlock()
		return f.result.value, f.result.err
	}
	f.mu.Unlock()
	p := f.peer
	p.mu.Lock()
	err := f.record.check()
	p.mu.Unlock()
	if err != nil {
		return nil, err
	}
	value, err := p.call(ctx, "await", map[string]any{"reference": f.record.id}, nil, false, f.record.origin, true)
	if err != nil && ctx.Err() != nil {
		return nil, err
	}
	f.mu.Lock()
	f.done, f.result = true, result{value, err}
	f.mu.Unlock()
	return value, err
}

// Settle awaits `value` while it is a future.
func Settle(ctx context.Context, value any) (any, error) {
	for {
		future, ok := value.(*RemoteFuture)
		if !ok {
			return value, nil
		}
		var err error
		if value, err = future.Await(ctx); err != nil {
			return nil, err
		}
	}
}

// Release gives an imported reference back now instead of at collection.
func (p *Peer) Release(proxy any) {
	switch proxy := proxy.(type) {
	case *RemoteFunction:
		p.release(proxy.record)
	case *RemoteFuture:
		p.release(proxy.record)
	}
}

func (p *Peer) release(record *imported) {
	p.mu.Lock()
	if record.released || p.closeErr != nil {
		record.released = true
		p.mu.Unlock()
		return
	}
	record.released = true
	if p.imports[record.id] == record {
		delete(p.imports, record.id)
	}
	count := record.grants
	p.mu.Unlock()
	if err := p.send(map[string]any{"op": "release", "reference": record.id, "count": count}); err != nil {
		p.fault(err)
	}
}

func holdsReference(value any) bool {
	switch v := value.(type) {
	case Func, Object, *RemoteFunction, *RemoteFuture:
		return true
	case []any:
		for _, item := range v {
			if holdsReference(item) {
				return true
			}
		}
	case map[string]any:
		for _, item := range v {
			if holdsReference(item) {
				return true
			}
		}
	}
	return false
}

// encode turns a value into its wire form; p.mu is held. `grants` collects
// the exports it granted, for a rollback.
func (p *Peer) encode(value any, grants *[]int64, origin []string) (map[string]any, error) {
	switch v := value.(type) {
	case UndefinedType:
		return map[string]any{"type": "undefined"}, nil
	case *RemoteFunction:
		if err := v.record.check(); err != nil {
			return nil, err
		}
		return homeReference(v.record), nil
	case *RemoteFuture:
		if err := v.record.check(); err != nil {
			return nil, err
		}
		return homeReference(v.record), nil
	case Func:
		return p.exportValue(v, "function", nil, grants, origin)
	case Object:
		if p.opts.Endpoint != nil && !p.supports("objects") {
			return nil, errors.New("the far end cannot receive object references")
		}
		var key any
		if reflect.ValueOf(v).Comparable() {
			key = v
		}
		return p.exportValue(v, "object", key, grants, origin)
	case []any:
		if holdsReference(v) {
			items := make([]any, len(v))
			for i, item := range v {
				encoded, err := p.encode(item, grants, origin)
				if err != nil {
					return nil, err
				}
				items[i] = encoded
			}
			return map[string]any{"type": "list", "value": items}, nil
		}
	case map[string]any:
		if holdsReference(v) {
			items := make(map[string]any, len(v))
			for key, item := range v {
				encoded, err := p.encode(item, grants, origin)
				if err != nil {
					return nil, err
				}
				items[key] = encoded
			}
			return map[string]any{"type": "record", "value": items}, nil
		}
	case error:
		return map[string]any{"type": "data", "value": map[string]any{"name": ErrorName(v), "message": v.Error()}}, nil
	}
	data, err := marshal(plain(value))
	if err != nil {
		return nil, fmt.Errorf("%T cannot cross the boundary: %w", value, err)
	}
	return map[string]any{"type": "data", "value": json.RawMessage(data)}, nil
}

// plain replaces undefined inside data by null.
func plain(value any) any {
	switch v := value.(type) {
	case UndefinedType:
		return nil
	case []any:
		out := make([]any, len(v))
		for i, item := range v {
			out[i] = plain(item)
		}
		return out
	case map[string]any:
		out := make(map[string]any, len(v))
		for key, item := range v {
			if _, undefined := item.(UndefinedType); undefined {
				continue
			}
			out[key] = plain(item)
		}
		return out
	}
	return value
}

func homeReference(record *imported) map[string]any {
	return map[string]any{"type": "reference", "value": map[string]any{
		"id": record.id, "kind": record.kind, "home": true, "origin": record.origin,
	}}
}

func (p *Peer) exportValue(value any, kind string, key any, grants *[]int64, origin []string) (map[string]any, error) {
	var entry *export
	ref, found := int64(0), false
	if key != nil {
		ref, found = p.identities[key]
	}
	if found {
		entry = p.exports[ref]
	}
	if entry == nil {
		p.ref++
		ref = p.ref
		entry = &export{value: value, kind: kind, origin: append([]string{}, origin...)}
		if entry.origin == nil {
			entry.origin = []string{}
		}
		p.exports[ref] = entry
		if key != nil {
			p.identities[key] = ref
		}
	}
	entry.grants++
	*grants = append(*grants, ref)
	return map[string]any{"type": "reference", "value": map[string]any{
		"id": ref, "kind": entry.kind, "home": false, "origin": entry.origin,
	}}, nil
}

func (p *Peer) rollback(grants []int64) {
	for _, ref := range grants {
		entry := p.exports[ref]
		if entry == nil {
			continue
		}
		entry.grants--
		if entry.grants == 0 {
			p.forget(ref)
		}
	}
}

func (p *Peer) forget(ref int64) {
	entry := p.exports[ref]
	delete(p.exports, ref)
	if entry == nil {
		return
	}
	for key, id := range p.identities {
		if id == ref {
			delete(p.identities, key)
		}
	}
}

type wireValue struct {
	Type  string          `json:"type"`
	Value json.RawMessage `json:"value"`
}

// decode turns a wire value into a value; p.mu is held. `call` is the
// call whose arguments these are ("" elsewhere).
func (p *Peer) decode(raw json.RawMessage, call string) (any, error) {
	if len(raw) == 0 {
		return Undefined, nil
	}
	var wire wireValue
	if err := json.Unmarshal(raw, &wire); err != nil {
		return nil, errors.New("invalid wire value")
	}
	switch wire.Type {
	case "undefined":
		return Undefined, nil
	case "data":
		if len(wire.Value) == 0 {
			return json.RawMessage("null"), nil
		}
		return append(json.RawMessage{}, wire.Value...), nil
	case "list":
		var items []json.RawMessage
		if err := json.Unmarshal(wire.Value, &items); err != nil {
			return nil, errors.New("invalid list")
		}
		out := make([]any, len(items))
		for i, item := range items {
			decoded, err := p.decode(item, call)
			if err != nil {
				return nil, err
			}
			out[i] = decoded
		}
		return out, nil
	case "record":
		var items map[string]json.RawMessage
		if err := json.Unmarshal(wire.Value, &items); err != nil || items == nil {
			return nil, errors.New("invalid record")
		}
		out := make(map[string]any, len(items))
		for key, item := range items {
			decoded, err := p.decode(item, call)
			if err != nil {
				return nil, err
			}
			out[key] = decoded
		}
		return out, nil
	case "signal":
		if call == "" {
			return nil, errors.New("a signal is only valid as a call argument")
		}
		signal := p.signals[call]
		if signal == nil {
			signal = newSignal()
			p.signals[call] = signal
		}
		return signal, nil
	case "reference":
		return p.decodeReference(wire.Value)
	}
	return nil, errors.New("invalid wire value")
}

func (p *Peer) originPattern() *regexp.Regexp {
	if p.opts.Endpoint == nil {
		return compatOrigin
	}
	return endpointOrigin
}

func (p *Peer) decodeReference(raw json.RawMessage) (any, error) {
	var value struct {
		ID     *json.Number `json:"id"`
		Kind   string       `json:"kind"`
		Home   *bool        `json:"home"`
		Origin []any        `json:"origin"`
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.UseNumber()
	if err := decoder.Decode(&value); err != nil {
		return nil, errors.New("invalid reference")
	}
	invalid := errors.New("invalid reference")
	if value.ID == nil || value.Home == nil || value.Origin == nil {
		return nil, invalid
	}
	ref, err := strconv.ParseInt(value.ID.String(), 10, 64)
	if err != nil || ref <= 0 || ref > MaxSafe {
		return nil, invalid
	}
	if value.Kind != "function" && value.Kind != "future" && value.Kind != "object" {
		return nil, invalid
	}
	origin := make([]string, len(value.Origin))
	for i, item := range value.Origin {
		text, ok := item.(string)
		if !ok || !p.originPattern().MatchString(text) {
			return nil, invalid
		}
		origin[i] = text
	}
	if *value.Home {
		entry := p.exports[ref]
		if entry == nil || entry.kind != value.Kind {
			return nil, errors.New("unknown, released or mismatched reference")
		}
		return entry.value, nil
	}
	if value.Kind == "object" {
		return nil, errors.New("object references from the far end are not supported")
	}
	if record := p.imports[ref]; record != nil {
		var proxy any
		if record.kind == "function" {
			if live := record.function.Value(); live != nil {
				proxy = live
			}
		} else if live := record.future.Value(); live != nil {
			proxy = live
		}
		if proxy != nil {
			if record.kind != value.Kind || !equal(record.origin, origin) {
				return nil, errors.New("invalid repeated grant")
			}
			record.grants++
			return proxy, nil
		}
	}
	record := &imported{id: ref, kind: value.Kind, origin: origin, grants: 1}
	p.imports[ref] = record
	if value.Kind == "function" {
		proxy := &RemoteFunction{peer: p, record: record}
		record.function = weak.Make(proxy)
		runtime.AddCleanup(proxy, func(r *imported) { go p.release(r) }, record)
		return proxy, nil
	}
	proxy := &RemoteFuture{peer: p, record: record}
	record.future = weak.Make(proxy)
	runtime.AddCleanup(proxy, func(r *imported) { go p.release(r) }, record)
	return proxy, nil
}

func equal(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// ── Outgoing calls ───────────────────────────────────────────────

// Call invokes `method` of `target` on the far end: synchronously, or as
// an asynchronous call (ctx cancels it, and the far end hears so). The
// call carries ctx's chain.
func (p *Peer) Call(ctx context.Context, target, method string, args []any, sync bool) (any, error) {
	return p.call(ctx, "invoke", map[string]any{"target": target, "method": method}, args, true, nil, !sync)
}

// Notify sends a call whose result nobody awaits. It is sent before Notify
// returns, so it goes out ahead of anything sent later.
func (p *Peer) Notify(target, method string, args []any) {
	id, _, err := p.request(context.Background(), "invoke", map[string]any{"target": target, "method": method}, args, true, nil)
	if err == nil {
		// Its reply, when it comes, is a late one: dropped.
		p.mu.Lock()
		delete(p.pending, id)
		p.mu.Unlock()
	}
}

func (p *Peer) call(ctx context.Context, op string, fields map[string]any, args []any, hasArgs bool, origin []string, cancellable bool) (any, error) {
	if ctx == nil {
		panic("rutis: a nil context: pass the context.Context your caller gave you")
	}
	id, waiter, err := p.request(ctx, op, fields, args, hasArgs, origin)
	if err != nil {
		return nil, err
	}
	select {
	case done := <-waiter:
		return done.value, done.err
	case <-ctx.Done():
		p.mu.Lock()
		_, waiting := p.pending[id]
		delete(p.pending, id)
		p.mu.Unlock()
		if waiting && cancellable {
			_ = p.send(map[string]any{"op": "cancel", "id": id})
		}
		return nil, ctx.Err()
	}
}

func (p *Peer) request(ctx context.Context, op string, fields map[string]any, args []any, hasArgs bool, origin []string) (string, chan result, error) {
	chain := ChainOf(ctx)
	// Identities go out in the order they are taken: the far end refuses
	// one not above the last it received, so the write lock spans both.
	p.writeMu.Lock()
	defer p.writeMu.Unlock()
	p.mu.Lock()
	if p.closeErr != nil {
		err := p.closeErr
		p.mu.Unlock()
		return "", nil, err
	}
	if !p.handshake {
		p.mu.Unlock()
		return "", nil, errors.New("protocol handshake incomplete")
	}
	p.next++
	if p.next > MaxSafe {
		p.mu.Unlock()
		return "", nil, errors.New("call identifiers exhausted")
	}
	id := p.local + strconv.FormatUint(p.next, 10)
	path := dedup(chain, origin)
	frame := map[string]any{"op": op, "id": id, "path": path}
	for key, value := range fields {
		frame[key] = value
	}
	var grants []int64
	if hasArgs {
		if args == nil {
			args = []any{}
		}
		encoded, err := p.encode(args, &grants, chain)
		if err != nil {
			p.rollback(grants)
			p.mu.Unlock()
			return "", nil, err
		}
		frame["args"] = encoded
	}
	data, err := marshal(frame)
	if err != nil {
		p.rollback(grants)
		p.mu.Unlock()
		return "", nil, err
	}
	waiter := make(chan result, 1)
	p.pending[id] = waiter
	p.mu.Unlock()
	if err := p.writeLocked(data); err != nil {
		p.mu.Lock()
		delete(p.pending, id)
		p.rollback(grants)
		p.mu.Unlock()
		return "", nil, err
	}
	return id, waiter, nil
}

func dedup(chain, origin []string) []string {
	path := make([]string, 0, len(chain)+len(origin))
	seen := map[string]bool{}
	for _, list := range [][]string{chain, origin} {
		for _, item := range list {
			if !seen[item] {
				seen[item] = true
				path = append(path, item)
			}
		}
	}
	return path
}

// Drain waits until no call that is the far end's business runs.
func (p *Peer) Drain(ctx context.Context) {
	p.mu.Lock()
	if p.active == 0 || p.closeErr != nil {
		p.mu.Unlock()
		return
	}
	waiter := make(chan struct{})
	p.draining = append(p.draining, waiter)
	p.mu.Unlock()
	select {
	case <-waiter:
	case <-ctx.Done():
	}
}

func (p *Peer) finish() {
	p.mu.Lock()
	p.active--
	var draining []chan struct{}
	if p.active == 0 {
		draining, p.draining = p.draining, nil
	}
	p.mu.Unlock()
	for _, waiter := range draining {
		close(waiter)
	}
}

// ── Incoming frames ──────────────────────────────────────────────

type frame struct {
	Op             string          `json:"op"`
	Version        json.RawMessage `json:"version"`
	Endpoint       json.RawMessage `json:"endpoint"`
	Implementation json.RawMessage `json:"implementation"`
	Capabilities   json.RawMessage `json:"capabilities"`
	ID             json.RawMessage `json:"id"`
	Path           json.RawMessage `json:"path"`
	Target         *string         `json:"target"`
	Method         json.RawMessage `json:"method"`
	Property       json.RawMessage `json:"property"`
	Reference      json.RawMessage `json:"reference"`
	Count          json.RawMessage `json:"count"`
	Args           json.RawMessage `json:"args"`
	Value          json.RawMessage `json:"value"`
	Error          json.RawMessage `json:"error"`
}

func (p *Peer) receive(f *frame) error {
	p.mu.Lock()
	if p.closeErr != nil {
		p.mu.Unlock()
		return nil
	}
	if f.Op == "hello" {
		defer p.mu.Unlock()
		if p.handshake {
			return errors.New("duplicate protocol handshake")
		}
		if err := p.greet(f); err != nil {
			return err
		}
		p.handshake = true
		close(p.ready)
		return nil
	}
	if !p.handshake {
		p.mu.Unlock()
		return errors.New("request before protocol handshake")
	}
	switch f.Op {
	case "return", "throw":
		var id string
		if err := json.Unmarshal(f.ID, &id); err != nil {
			p.mu.Unlock()
			return errors.New("invalid response")
		}
		var outcome result
		if f.Op == "return" {
			value, err := p.decode(f.Value, "")
			if err != nil {
				p.mu.Unlock()
				return err
			}
			outcome.value = value
		} else {
			outcome.err = decodeError(f.Error)
		}
		waiter, ok := p.pending[id]
		if !ok {
			// A late reply to a call of ours given up (cancelled, or a
			// notification) is dropped; any other is a fault.
			p.mu.Unlock()
			if p.issued(id) {
				return nil
			}
			return errors.New("response for unknown call")
		}
		delete(p.pending, id)
		p.mu.Unlock()
		waiter <- outcome
		return nil
	case "cancel":
		var id string
		if json.Unmarshal(f.ID, &id) != nil {
			p.mu.Unlock()
			return errors.New("invalid cancel")
		}
		cancel := p.running[id]
		signal := p.signals[id]
		p.mu.Unlock()
		if signal != nil {
			signal.cancel()
		}
		if cancel != nil {
			cancel()
		}
		return nil
	case "release":
		defer p.mu.Unlock()
		var ref, count int64
		if json.Unmarshal(f.Reference, &ref) != nil {
			return errors.New("unknown, released or mismatched reference")
		}
		entry := p.exports[ref]
		if entry == nil {
			return errors.New("unknown, released or mismatched reference")
		}
		if json.Unmarshal(f.Count, &count) != nil || count <= 0 || count > entry.grants {
			return errors.New("invalid reference release count")
		}
		entry.grants -= count
		if entry.grants == 0 {
			p.forget(ref)
		}
		return nil
	}
	job, err := p.invocation(f)
	if err != nil {
		p.mu.Unlock()
		return err
	}
	p.mu.Unlock()
	go p.execute(job)
	return nil
}

// issued reports whether `id` names a call this side made; p.mu is not
// held.
func (p *Peer) issued(id string) bool {
	if !strings.HasPrefix(id, p.local) || !sequence.MatchString(id[len(p.local):]) {
		return false
	}
	n, err := strconv.ParseUint(id[len(p.local):], 10, 64)
	p.mu.Lock()
	defer p.mu.Unlock()
	return err == nil && n <= p.next
}

func (p *Peer) greet(f *frame) error {
	var version int
	_ = json.Unmarshal(f.Version, &version)
	if p.opts.Endpoint == nil {
		if version != Protocol || len(f.Endpoint) > 0 {
			return fmt.Errorf("incompatible session: the far end speaks protocol %s, this side %d", strings.TrimSpace(string(f.Version)), Protocol)
		}
		return nil
	}
	if version != EndpointProtocol {
		return fmt.Errorf("incompatible session: the far end speaks protocol %s, this side %d", strings.TrimSpace(string(f.Version)), EndpointProtocol)
	}
	var endpoint string
	if json.Unmarshal(f.Endpoint, &endpoint) != nil || !endpointID.MatchString(endpoint) {
		return errors.New("incompatible session: the far end named no valid endpoint")
	}
	if expected := p.opts.Endpoint.Expected; expected != "" && expected != endpoint {
		return fmt.Errorf("endpoint mismatch: the far end greeted as %s, but %s is the expected endpoint", endpoint, expected)
	}
	if endpoint == p.opts.Endpoint.Local {
		return fmt.Errorf("endpoint mismatch: the far end greeted as this endpoint (%s)", endpoint)
	}
	p.remote = endpoint + ":"
	var capabilities []string
	_ = json.Unmarshal(f.Capabilities, &capabilities)
	p.greeting = &Greeting{Endpoint: endpoint, Implementation: f.Implementation, Capabilities: capabilities}
	return nil
}

type job struct {
	op       string
	id       string
	path     []string
	target   string
	method   *string
	property string
	args     []any
	entry    *export
	business bool
	ctx      context.Context
	cancel   context.CancelFunc
}

// invocation validates an incoming call; p.mu is held.
func (p *Peer) invocation(f *frame) (*job, error) {
	invalid := errors.New("invalid invocation")
	if f.Op != "invoke" && f.Op != "call" && f.Op != "get" && f.Op != "await" {
		return nil, invalid
	}
	var id string
	var path []string
	if json.Unmarshal(f.ID, &id) != nil || json.Unmarshal(f.Path, &path) != nil || f.Path == nil || string(f.Path) == "null" {
		return nil, invalid
	}
	if !strings.HasPrefix(id, p.remote) || !sequence.MatchString(id[len(p.remote):]) {
		return nil, invalid
	}
	for _, item := range path {
		if item == id {
			return nil, invalid
		}
	}
	seq, err := strconv.ParseUint(id[len(p.remote):], 10, 64)
	if err != nil || seq <= p.received || seq > MaxSafe {
		return nil, errors.New("invalid or repeated invocation identity")
	}
	p.received = seq
	j := &job{op: f.Op, id: id, path: path}
	if len(f.Method) > 0 && string(f.Method) != "null" {
		var method string
		if json.Unmarshal(f.Method, &method) != nil {
			return nil, errors.New("invalid method")
		}
		j.method = &method
	}
	if f.Op == "get" {
		if json.Unmarshal(f.Property, &j.property) != nil {
			return nil, errors.New("invalid property")
		}
	}
	if f.Op == "invoke" || f.Op == "call" {
		decoded, err := p.decode(f.Args, id)
		if err != nil {
			return nil, err
		}
		j.args = arguments(decoded)
	}
	switch {
	case f.Op == "invoke":
		if f.Target == nil || j.method == nil {
			return nil, invalid
		}
		j.target = *f.Target
		j.business = j.target != ""
	default:
		kind := "function"
		if f.Op == "await" {
			kind = "future"
		} else if f.Op == "get" || j.method != nil {
			kind = "object"
		}
		var ref int64
		if json.Unmarshal(f.Reference, &ref) != nil {
			return nil, errors.New("unknown, released or mismatched reference")
		}
		entry := p.exports[ref]
		if entry == nil || entry.kind != kind {
			return nil, errors.New("unknown, released or mismatched reference")
		}
		j.entry = entry
		j.business = true
	}
	if j.business {
		p.active++
	}
	j.ctx, j.cancel = context.WithCancel(p.base)
	p.running[id] = j.cancel
	return j, nil
}

// arguments is a call's argument list.
func arguments(decoded any) []any {
	switch v := decoded.(type) {
	case []any:
		return v
	case UndefinedType, nil:
		return []any{}
	case json.RawMessage:
		var items []json.RawMessage
		if json.Unmarshal(v, &items) == nil && items != nil {
			out := make([]any, len(items))
			for i, item := range items {
				out[i] = item
			}
			return out
		}
		if string(v) == "null" {
			return []any{}
		}
	}
	return []any{decoded}
}

func (p *Peer) execute(j *job) {
	if j.business {
		defer p.finish()
	}
	defer j.cancel()
	ctx := WithChain(j.ctx, append(append([]string{}, j.path...), j.id))
	value, err := protect(func() (any, error) {
		switch {
		case j.op == "invoke":
			if p.opts.Dispatch == nil {
				return nil, fmt.Errorf("no target %s", j.target)
			}
			return p.opts.Dispatch(ctx, j.target, *j.method, j.args)
		case j.op == "await":
			return nil, errors.New("this side exports no futures")
		case j.op == "get":
			return j.entry.value.(Object).Get(ctx, j.property)
		case j.method != nil:
			return j.entry.value.(Object).Call(ctx, *j.method, j.args)
		default:
			return j.entry.value.(Func)(ctx, j.args)
		}
	})
	p.respond(j.id, value, err, ChainOf(ctx))
}

// Protect runs `call`, turning a panic into a PanicError.
func Protect(call func() (any, error)) (any, error) { return protect(call) }

func protect(call func() (value any, err error)) (value any, err error) {
	defer func() {
		if recovered := recover(); recovered != nil {
			value, err = nil, &PanicError{Value: recovered, Stack: string(debug.Stack())}
		}
	}()
	return call()
}

func (p *Peer) respond(id string, value any, failure error, chain []string) {
	p.mu.Lock()
	delete(p.running, id)
	delete(p.signals, id)
	if p.closeErr != nil {
		p.mu.Unlock()
		return
	}
	var data []byte
	var grants []int64
	var err error
	if failure == nil {
		var encoded map[string]any
		encoded, err = p.encode(value, &grants, chain)
		if err == nil {
			data, err = marshal(map[string]any{"op": "return", "id": id, "value": encoded})
		}
		if err != nil {
			p.rollback(grants)
			failure = err
		}
	}
	if failure != nil {
		data, err = marshal(map[string]any{"op": "throw", "id": id, "error": encodeError(failure)})
		if err != nil {
			data, _ = marshal(map[string]any{"op": "throw", "id": id, "error": map[string]any{"name": "Error", "message": failure.Error()}})
		}
	}
	p.mu.Unlock()
	if err := p.write(data); err != nil {
		p.fault(err)
	}
}
