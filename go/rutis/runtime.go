package rutis

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"reflect"
	goruntime "runtime"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/events"
	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// features are what this runtime supports of the runtime contract.
// rows.announce: services rows provide without declaring them are announced
// with their methods, to a host whose mount says `announce`. Events are the
// capability events-v2 (#464), no longer a feature. plugin-values: a plugin
// definition this runtime handed over comes back to be described
// (`plugins.read`) and loaded as a row's entry
// (docs/design-plugin-values-2026-10-25.md §5).
var features = []string{"rows.v2", "hosts", "leaf", "scopes", "rows.announce", "plugin-values"}

// Future is an asynchronous result of another process.
type Future = convert.Future

// Function is a function of another process, received where a value of
// any type was expected.
type Function = convert.Function

// RemoteError is an error thrown in another process.
type RemoteError = peer.RemoteError

// ErrSyncWaitCycle matches (errors.Is) a SyncWaitCycle error from another
// process.
var ErrSyncWaitCycle = peer.ErrSyncWaitCycle

// Ref marks v to cross by reference: the other process gets a proxy
// calling v's exported methods. Values otherwise cross as data.
func Ref(v any) any { return convert.Ref(v) }

// scopedID is how the service `name` is identified in the scope `label`:
// the name outside any scope, the name, a NUL and the label inside one.
func scopedID(name, label string, scoped bool) (string, error) {
	if strings.Contains(name, "\x00") || strings.Contains(label, "\x00") {
		return "", fmt.Errorf("service %q or its scope label contains NUL", name)
	}
	if !scoped {
		return name, nil
	}
	if label == "" {
		return "", fmt.Errorf("service %q has an empty scope label", name)
	}
	return name + "\x00" + label, nil
}

func handleOf(id string, generation int) string {
	if generation == 1 {
		return id
	}
	separator := "#"
	if strings.Contains(id, "\x00") {
		separator = "\x00"
	}
	return id + separator + strconv.Itoa(generation)
}

type providedService struct {
	row   string
	value any
	shape *Shape
}

type slot struct {
	row        string
	name       string
	methods    map[string]bool
	object     any
	present    bool
	handle     string
	generation int
	// Not declared: announced, with the methods of each object.
	announced bool
}

type handleEntry struct {
	id       string
	object   any
	shape    *Shape
	methods  map[string]bool
	current  bool
	released bool
}

type row struct {
	key string
	// The row's id in the configuration (rows.load's `id`; the key when
	// not given): descriptors and reported lines call the row so.
	name string
	// A plugin's name in this binary, or a plugin definition this runtime
	// handed over (a *Definition back home).
	entry    any
	def      *Definition
	config   any
	exports  map[string]map[string]string
	isolate  map[string]string
	ctx      context.Context
	cancel   context.CancelFunc
	cleanups []func(context.Context) error
	provided []string
	tasks    sync.WaitGroup
	// The listener invocations running or queued: unload waits for them,
	// up to the shutdown deadline.
	listening sync.WaitGroup
	// Its scope chain, outermost first (rows.load).
	scope []Scope
	order int64
	// Ids of the slots of services it announced.
	announced []string
	// Announcements the host has yet to answer, while the row loads: each
	// waits for its answer, and an error is a refusal that fails the load.
	// Nil once it loaded.
	pending []func() error
	loading bool
}

// plugin is what descriptors call the row's plugin: its name, or, for a
// nameless plugin definition handed over as a value, the row's id.
func (r *row) plugin() string {
	if r.def.name != "" {
		return r.def.name
	}
	return r.name
}

func (r *row) id(name string) string {
	label, scoped := r.isolate[name]
	id, err := scopedID(name, label, scoped)
	if err != nil {
		return name
	}
	return id
}

// runtimeState runs the rows one controller loads into this process.
type runtimeState struct {
	defs    map[string]*Definition
	session *peer.Peer

	mu       sync.Mutex
	rows     map[string]*row
	services map[string]providedService
	hosts    map[string]map[string]string
	slots    map[string]*slot
	handles  map[string]*handleEntry
	version  int64
	loaded   int64
	closing  bool
	// The listeners of each name, in position order, and by id.
	listeners   map[string][]*listener
	byID        map[string]*listener
	listenerSeq int64
	// The lowest and highest positions given (Prepend takes one below).
	first, last int64
	// The names and prefixes with listeners outside this process
	// (events.elsewhere), and the emits through the host not answered yet.
	elsewhere         map[string]bool
	elsewherePrefixes map[string]bool
	hostEmits         map[string]int
	// What the host calls this runtime (mount `runtime`), for descriptors.
	runtimeName string
	// The host takes announced services (mount `announce`).
	announce bool
	// Mounted by a host; one without `announce` is older than 0.12.
	mounted bool
	// The undeclared names a plugin was told such a host does not take.
	told map[string]bool

	// Frames for the host, queued under mu in the order they are taken and
	// sent by flush once mu is released: a write blocked by a far end that
	// does not read then never holds mu, which the session's reader needs.
	// outMu guards the queue; sendMu keeps the sends in queue order.
	outMu  sync.Mutex
	outbox []func()
	sendMu sync.Mutex
}

// post queues `send`, which writes to the session, behind every frame
// queued before; rt.mu is held. The caller flushes after releasing rt.mu.
func (rt *runtimeState) post(send func()) {
	rt.outMu.Lock()
	rt.outbox = append(rt.outbox, send)
	rt.outMu.Unlock()
}

// flush sends the queued frames in order; rt.mu is not held. Once it
// returns, every frame queued before the call is sent.
func (rt *runtimeState) flush() {
	rt.sendMu.Lock()
	defer rt.sendMu.Unlock()
	for {
		rt.outMu.Lock()
		if len(rt.outbox) == 0 {
			rt.outMu.Unlock()
			return
		}
		send := rt.outbox[0]
		rt.outbox[0] = nil
		rt.outbox = rt.outbox[1:]
		rt.outMu.Unlock()
		send()
	}
}

// relay closes `to` once `from` is closed.
func relay(from <-chan struct{}, to chan struct{}) {
	go func() {
		<-from
		close(to)
	}()
}

// Where a plugin is told that the host does not take an undeclared
// service (oldHost).
var oldHostOutput io.Writer = os.Stderr

// Where a row's late failures are written (tests replace it).
var rowOutput io.Writer = os.Stderr

// What a plugin providing the undeclared service name is told when the
// host, older than 0.12, mounted without `announce`: such a host does not
// take the service, which stays in this process.
func oldHost(name string) string {
	return fmt.Sprintf("rutis: service %s is not declared in provides, and this host (older than 0.12) does not take undeclared services; declare it or upgrade rutis-host", name)
}

func newRuntime(defs []*Definition) *runtimeState {
	byName := map[string]*Definition{}
	for _, def := range defs {
		byName[def.key()] = def
	}
	return &runtimeState{
		defs:     byName,
		rows:     map[string]*row{},
		services: map[string]providedService{},
		hosts:    map[string]map[string]string{},
		slots:    map[string]*slot{},
		handles:  map[string]*handleEntry{},
		told:     map[string]bool{},

		listeners:         map[string][]*listener{},
		byID:              map[string]*listener{},
		elsewhere:         map[string]bool{},
		elsewherePrefixes: map[string]bool{},
		hostEmits:         map[string]int{},
	}
}

func (rt *runtimeState) names() string {
	names := make([]string, 0, len(rt.defs))
	for name := range rt.defs {
		names = append(names, name)
	}
	sort.Strings(names)
	return strings.Join(names, ", ")
}

func (rt *runtimeState) definition(entry string) (*Definition, error) {
	def := rt.defs[entry]
	if def == nil {
		return nil, &peer.RemoteError{Name: "NotFound", Message: fmt.Sprintf("plugin %s is not in this binary; it has: %s", entry, rt.names())}
	}
	if def.api > PluginAPI {
		return nil, fmt.Errorf("plugin %s needs plugin API %d; this runtime supports %d", entry, def.api, PluginAPI)
	}
	return def, nil
}

// entryDefinition is the plugin of a row's entry: a plugin's name in this
// binary, or a plugin definition this runtime handed over, back home as
// itself (docs/design-plugin-values-2026-10-25.md §5).
func (rt *runtimeState) entryDefinition(entry any) (*Definition, error) {
	switch e := entry.(type) {
	case string:
		return rt.definition(e)
	case *Definition:
		if e == nil {
			break
		}
		if e.remote != nil {
			return nil, errors.New("a plugin definition of another process cannot be loaded here: hand it back to the runtime that defined it")
		}
		if e.api > PluginAPI {
			return nil, fmt.Errorf("%s needs plugin API %d; this runtime supports %d", called(e.name), e.api, PluginAPI)
		}
		return e, nil
	}
	return nil, errors.New("not a plugin definition: expected a rutis.Define result")
}

// ── Services ─────────────────────────────────────────────────────

func same(a, b any) bool {
	if a == nil || b == nil {
		return a == nil && b == nil
	}
	ta, tb := reflect.TypeOf(a), reflect.TypeOf(b)
	if ta != tb || !ta.Comparable() {
		return false
	}
	return a == b
}

// refresh reports the object now in an exported slot under a new handle;
// rt.mu is held, and the report is queued (post).
func (rt *runtimeState) refresh(id string) {
	s := rt.slots[id]
	if s == nil {
		return
	}
	var current any
	present := false
	if provided, ok := rt.services[id]; ok && provided.row == s.row {
		current, present = provided.value, true
	}
	if present == s.present && same(current, s.object) {
		return
	}
	if s.handle != "" {
		rt.retire(s.handle)
	}
	s.object, s.present, s.handle = current, present, ""
	if present {
		s.generation++
		s.handle = handleOf(id, s.generation)
		var shape *Shape
		if r := rt.rows[s.row]; r != nil {
			shape = r.def.provides[s.name]
		}
		if s.announced {
			s.methods = map[string]bool{}
			for wire := range convert.Methods(reflect.TypeOf(current)) {
				s.methods[wire] = true
			}
		}
		rt.handles[s.handle] = &handleEntry{id: id, object: current, shape: shape, methods: s.methods, current: true}
	}
	rt.version++
	if rt.session != nil && !rt.closing {
		session := rt.session
		var handle any
		if s.handle != "" {
			handle = s.handle
		}
		if !s.announced {
			args := []any{id, handle, rt.version}
			rt.post(func() { session.Notify("", "service", args) })
			return
		}
		// An announced service says whose it is and what it offers, and a
		// row loading answers once rutis has heard it (a refusal fails the
		// row there).
		methods := map[string]any{}
		for wire := range s.methods {
			methods[wire] = "async"
		}
		args := []any{id, handle, rt.version, map[string]any{"row": s.row, "methods": methods}}
		answered := make(chan struct{})
		rt.post(func() { relay(session.Announce("", "service", args), answered) })
		if r := rt.rows[s.row]; r != nil && r.loading {
			r.pending = append(r.pending, func() error { <-answered; return nil })
		}
	}
}

func (rt *runtimeState) retire(handle string) {
	entry := rt.handles[handle]
	if entry == nil {
		return
	}
	entry.current = false
	if entry.released {
		delete(rt.handles, handle)
	}
}

func (rt *runtimeState) provide(r *row, name string, value any) func() {
	id := r.id(name)
	shape := r.def.provides[name]
	if value == nil {
		panic(fmt.Sprintf("rutis: provide %s: a nil service", name))
	}
	// Not declared: its methods are its exported ones, all async.
	if shape != nil && !reflect.TypeOf(value).AssignableTo(shape.typ) {
		panic(fmt.Sprintf("rutis: provide %s: %T is not the %s Provides declares", name, value, shape.typ))
	}
	rt.mu.Lock()
	// A task of an unloaded row providing: refused, or the service would
	// outlive its row and the row's next load find it taken. Reported, not
	// a panic: a plain goroutine or timer calling it recovers nothing.
	if rt.rows[r.key] != r {
		rt.mu.Unlock()
		fmt.Fprintf(rowOutput, "rutis: row %s is not active: service %s is not provided\n", r.key, name)
		return func() {}
	}
	if existing, ok := rt.services[id]; ok {
		rt.mu.Unlock()
		panic(fmt.Sprintf("rutis: service %s is already provided by row %s", printable(id), existing.row))
	}
	rt.services[id] = providedService{row: r.key, value: value, shape: shape}
	r.provided = append(r.provided, id)
	// Not declared: announced, with its exported methods; a host older
	// than 0.12 does not take it, and the plugin is told, once per name.
	if rt.slots[id] == nil && !strings.Contains(name, "#") {
		if rt.announce {
			rt.slots[id] = &slot{row: r.key, name: name, announced: true}
			r.announced = append(r.announced, id)
		} else if rt.mounted && !rt.told[name] {
			rt.told[name] = true
			fmt.Fprintln(oldHostOutput, oldHost(name))
		}
	}
	rt.refresh(id)
	rt.mu.Unlock()
	rt.flush()
	return func() {
		rt.mu.Lock()
		if existing, ok := rt.services[id]; ok && existing.row == r.key && same(existing.value, value) {
			delete(rt.services, id)
			for i, provided := range r.provided {
				if provided == id {
					r.provided = append(r.provided[:i], r.provided[i+1:]...)
					break
				}
			}
			rt.refresh(id)
		}
		rt.mu.Unlock()
		rt.flush()
	}
}

func printable(id string) string { return strings.ReplaceAll(id, "\x00", "@") }

// ── Rows ─────────────────────────────────────────────────────────

func (rt *runtimeState) load(ctx context.Context, key, entry string, config any, exports map[string]map[string]string, isolate map[string]string, scope []Scope) error {
	return rt.loadNamed(ctx, key, key, entry, config, exports, isolate, scope)
}

// loadNamed loads row key, whose id in the configuration is name.
// entry is a plugin's name in this binary, or a plugin definition this
// runtime handed over.
func (rt *runtimeState) loadNamed(ctx context.Context, key, name string, entry any, config any, exports map[string]map[string]string, isolate map[string]string, scope []Scope) error {
	def, err := rt.entryDefinition(entry)
	if err != nil {
		return err
	}
	if name == "" {
		name = key
	}
	r := &row{key: key, name: name, entry: entry, def: def, config: config, exports: exports, isolate: isolate, scope: scope, loading: true}
	rt.mu.Lock()
	if _, loaded := rt.rows[key]; loaded {
		rt.mu.Unlock()
		return fmt.Errorf("row %s is already loaded", key)
	}
	for name, label := range isolate {
		if _, err := scopedID(name, label, true); err != nil {
			rt.mu.Unlock()
			return err
		}
	}
	for name := range exports {
		if strings.Contains(name, "#") {
			rt.mu.Unlock()
			return fmt.Errorf("service name %s cannot be projected", name)
		}
		if existing := rt.slots[r.id(name)]; existing != nil {
			rt.mu.Unlock()
			return fmt.Errorf("service %s is already exported by row %s", printable(r.id(name)), existing.row)
		}
	}
	// The row's context outlives the rows.load call, keeping its chain.
	r.ctx, r.cancel = context.WithCancel(context.WithoutCancel(ctx))
	rt.loaded++
	r.order = rt.loaded
	rt.rows[key] = r
	for name, methods := range exports {
		allowed := map[string]bool{}
		for method := range methods {
			allowed[method] = true
		}
		rt.slots[r.id(name)] = &slot{row: key, name: name, methods: allowed}
	}
	rt.mu.Unlock()
	_, err = peer.Protect(func() (any, error) {
		return nil, def.apply(&Ctx{Context: r.ctx, rt: rt, row: r}, config)
	})
	if err != nil {
		if unloadErr := rt.unload(ctx, key); unloadErr != nil {
			return errors.Join(err, unloadErr)
		}
		return err
	}
	rt.mu.Lock()
	for name := range exports {
		rt.refresh(r.id(name))
	}
	pending := r.pending
	r.pending, r.loading = nil, false
	rt.mu.Unlock()
	rt.flush()
	// rutis has published the announced services (or refused one, which
	// fails the row there) and registered the row's listeners before it
	// hears that the row loaded. A refused listener fails the load (§4.4).
	var refused error
	for _, wait := range pending {
		if err := wait(); err != nil && refused == nil {
			refused = err
		}
	}
	if refused != nil {
		if unloadErr := rt.unload(ctx, key); unloadErr != nil {
			return errors.Join(refused, unloadErr)
		}
		return refused
	}
	return nil
}

func (rt *runtimeState) unload(ctx context.Context, key string) error {
	rt.mu.Lock()
	r := rt.rows[key]
	if r == nil {
		rt.mu.Unlock()
		return nil
	}
	delete(rt.rows, key)
	rt.forgetListeners(r)
	// Withdrawals first: rutis hears them before the plugin goes away.
	for _, id := range append([]string{}, r.provided...) {
		if existing, ok := rt.services[id]; ok && existing.row == key {
			delete(rt.services, id)
			rt.refresh(id)
		}
	}
	r.provided = nil
	ids := append([]string{}, r.announced...)
	for name := range r.exports {
		ids = append(ids, r.id(name))
	}
	for _, id := range ids {
		if s := rt.slots[id]; s != nil && s.row == key {
			delete(rt.slots, id)
			if s.handle != "" {
				rt.retire(s.handle)
			}
		}
	}
	r.announced = nil
	rt.mu.Unlock()
	rt.flush()
	r.cancel()
	waitListeners(r)
	r.tasks.Wait()
	var errs []error
	for i := len(r.cleanups) - 1; i >= 0; i-- {
		cleanup := r.cleanups[i]
		if _, err := peer.Protect(func() (any, error) { return nil, cleanup(ctx) }); err != nil {
			errs = append(errs, err)
		}
	}
	return errors.Join(errs...)
}

func (rt *runtimeState) update(ctx context.Context, key string, config any) error {
	rt.mu.Lock()
	r := rt.rows[key]
	rt.mu.Unlock()
	if r == nil {
		return fmt.Errorf("row %s is not loaded", key)
	}
	// Leaf plugins have no volatile fields: a new config restarts the row.
	if err := rt.unload(ctx, key); err != nil {
		return err
	}
	return rt.loadNamed(ctx, key, r.name, r.entry, config, r.exports, r.isolate, r.scope)
}

// waitListeners waits for the row's running listeners, cancelled, at most
// the shutdown deadline; then it gives them up (§7.1).
func waitListeners(r *row) {
	done := make(chan struct{})
	go func() { r.listening.Wait(); close(done) }()
	budget := endedBudget()
	timer := time.NewTimer(budget)
	defer timer.Stop()
	select {
	case <-done:
	case <-timer.C:
		events.Report(fmt.Sprintf("rutis: listeners of row %s still running after %ss; unloading anyway", r.name, strconv.FormatFloat(budget.Seconds(), 'f', -1, 64)))
	}
}

// dispose unloads every row, latest first. It returns ctx's error when it
// gave up cleanups that had not finished by then.
func (rt *runtimeState) dispose(ctx context.Context) error {
	rt.mu.Lock()
	rows := make([]*row, 0, len(rt.rows))
	for _, r := range rt.rows {
		rows = append(rows, r)
	}
	rt.mu.Unlock()
	sort.Slice(rows, func(i, j int) bool { return rows[i].order < rows[j].order })
	for i := len(rows) - 1; i >= 0; i-- {
		done := make(chan struct{})
		go func(key string) {
			defer close(done)
			_ = rt.unload(ctx, key)
		}(rows[i].key)
		select {
		case <-done:
		case <-ctx.Done():
			fmt.Fprintf(os.Stderr, "rutis: gave up the cleanups of row %s and those after it: %v\n", rows[i].key, ctx.Err())
			return ctx.Err()
		}
	}
	return nil
}

// ── Dispatch ─────────────────────────────────────────────────────

func (rt *runtimeState) dispatch(ctx context.Context, target, method string, args []any) (any, error) {
	rt.mu.Lock()
	closing := rt.closing
	rt.mu.Unlock()
	if closing {
		return nil, errors.New("runtime is closing")
	}
	if target == "" {
		return rt.control(ctx, method, args)
	}
	rt.mu.Lock()
	entry := rt.handles[target]
	rt.mu.Unlock()
	if entry == nil {
		return nil, fmt.Errorf("unknown or released service object %s", printable(target))
	}
	if entry.methods != nil && !entry.methods[method] {
		return nil, fmt.Errorf("unknown service method %s.%s", printable(entry.id), method)
	}
	fn, ok := entry.shape.method(reflect.ValueOf(entry.object), method)
	if !ok {
		return nil, fmt.Errorf("unknown service method %s.%s", printable(entry.id), method)
	}
	return convert.Call(ctx, fn, args)
}

func argument[T any](args []any, i int) (T, error) {
	var out T
	if i >= len(args) {
		return out, nil
	}
	err := convert.Decode(args[i], &out)
	return out, err
}

func (rt *runtimeState) control(ctx context.Context, method string, args []any) (any, error) {
	switch method {
	case "mount":
		mount, err := argument[struct {
			Announce bool
			// The name the host gives this runtime, which descriptors of
			// its listeners carry; optional.
			Runtime string
		}](args, 0)
		if err != nil {
			return nil, fmt.Errorf("mount: %w", err)
		}
		rt.mu.Lock()
		rt.announce = mount.Announce
		rt.runtimeName = mount.Runtime
		rt.mounted = true
		rt.mu.Unlock()
		return map[string]any{
			"services":       map[string]any{},
			"features":       features,
			"implementation": map[string]string{"name": Implementation, "version": Version},
			"engine":         map[string]string{"name": "go", "version": goruntime.Version()},
		}, nil
	case "events.elsewhere", "events.call":
		// Served by arrive on the reading goroutine; here only when called
		// directly.
		return rt.arrive(ctx, method, args)()
	case "dispose":
		rt.mu.Lock()
		rt.closing = true
		rt.mu.Unlock()
		rt.dispose(ctx)
		if rt.session != nil {
			rt.session.Drain(ctx)
		}
		return nil, nil
	case "rows.load":
		key, err := argument[string](args, 0)
		if err != nil {
			return nil, err
		}
		// A plugin's name in this binary, or a plugin definition this
		// runtime handed over.
		var entry any
		if len(args) > 1 {
			if def, ok := args[1].(*Definition); ok {
				entry = def
			}
		}
		if entry == nil {
			name, err := argument[string](args, 1)
			if err != nil {
				return nil, err
			}
			entry = name
		}
		var config any = peer.Undefined
		if len(args) > 2 {
			config = args[2]
		}
		// args[4], the row's own `inject`, names services a Cordis row waits
		// for in Cordis; a leaf runtime's rows wait in rutis, which gates
		// every name its plugin injects (as in the Python runtime).
		pairs, err := argument[[][2]string](args, 3)
		if err != nil {
			return nil, fmt.Errorf("isolate: %w", err)
		}
		isolate := map[string]string{}
		for _, pair := range pairs {
			isolate[pair[0]] = pair[1]
		}
		exports, err := argument[map[string]map[string]string](args, 5)
		if err != nil {
			return nil, fmt.Errorf("exports: %w", err)
		}
		// The seventh, from rutis 0.13: the row's scope chain and its id.
		placed, err := argument[struct {
			Scope []Scope
			ID    string `json:"id"`
		}](args, 6)
		if err != nil {
			return nil, fmt.Errorf("scope: %w", err)
		}
		return nil, rt.loadNamed(ctx, key, placed.ID, entry, config, exports, isolate, placed.Scope)
	case "rows.update":
		key, err := argument[string](args, 0)
		if err != nil {
			return nil, err
		}
		var config any = peer.Undefined
		if len(args) > 1 {
			config = args[1]
		}
		return nil, rt.update(ctx, key, config)
	case "rows.unload":
		key, err := argument[string](args, 0)
		if err != nil {
			return nil, err
		}
		return nil, rt.unload(ctx, key)
	case "rows.schema":
		entry, err := argument[string](args, 0)
		if err != nil {
			return nil, err
		}
		def, err := rt.definition(entry)
		if err != nil {
			return nil, err
		}
		return def.describe(), nil
	case "plugins.read":
		// A plugin definition this runtime handed over, back home: what it
		// declares, with its name.
		var value any
		if len(args) > 0 {
			value = args[0]
		}
		def, ok := value.(*Definition)
		if !ok || def == nil {
			return nil, errors.New("not a plugin definition: expected a rutis.Define result")
		}
		if _, err := rt.entryDefinition(def); err != nil {
			return nil, err
		}
		described := def.describe()
		described["name"] = nil
		if def.name != "" {
			described["name"] = def.name
		}
		// A value has no package version of its own.
		described["version"] = nil
		return described, nil
	case "hosts.provide":
		name, err := argument[string](args, 0)
		if err != nil {
			return nil, err
		}
		methods, err := argument[map[string]string](args, 1)
		if err != nil {
			return nil, err
		}
		label, err := argument[*string](args, 2)
		if err != nil {
			return nil, err
		}
		id, err := scopedID(name, deref(label), label != nil)
		if err != nil {
			return nil, err
		}
		rt.mu.Lock()
		defer rt.mu.Unlock()
		if _, exists := rt.hosts[id]; exists {
			return nil, fmt.Errorf("host service %s is already provided", printable(id))
		}
		if methods == nil {
			methods = map[string]string{}
		}
		rt.hosts[id] = methods
		return nil, nil
	case "hosts.withdraw":
		id, err := argument[string](args, 0)
		if err != nil {
			return nil, err
		}
		rt.mu.Lock()
		delete(rt.hosts, id)
		rt.mu.Unlock()
		return nil, nil
	case "release":
		handle, err := argument[string](args, 0)
		if err != nil {
			return nil, err
		}
		rt.mu.Lock()
		if entry := rt.handles[handle]; entry != nil {
			entry.released = true
			if !entry.current {
				delete(rt.handles, handle)
			}
		}
		rt.mu.Unlock()
		return nil, nil
	case "get":
		handle, err := argument[string](args, 0)
		if err != nil {
			return nil, err
		}
		property, err := argument[string](args, 1)
		if err != nil {
			return nil, err
		}
		rt.mu.Lock()
		entry := rt.handles[handle]
		rt.mu.Unlock()
		if entry == nil {
			return nil, fmt.Errorf("unknown or released service object %s", printable(handle))
		}
		return convert.Property(reflect.ValueOf(entry.object), property)
	}
	return nil, fmt.Errorf("unknown control method %s", method)
}

func deref(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

// ── Ctx ──────────────────────────────────────────────────────────

// Ctx is what Apply receives. It is a context.Context cancelled when the
// plugin unloads: goroutines the plugin starts should watch it.
type Ctx struct {
	context.Context
	rt  *runtimeState
	row *row
}

// Use binds the service `name`, which Inject must list, to `target`: a
// pointer to a struct of function fields, or to a *Service.
//
// A field calls the service method its name maps to (Ask -> ask; a
// `rutis:"name"` tag overrides). The struct may list only the methods it
// uses, but each field must match a method the service declares. A field
// is a func with an optional leading context.Context and an error as its
// last result, after at most one value.
func (c *Ctx) Use(name string, target any) error {
	if !contains(c.row.def.inject, name) {
		return fmt.Errorf("the plugin uses %s without declaring it in Inject: add %q to its Inject (rutis gives a plugin only the services it declares, running or not)", name, name)
	}
	id := c.row.id(name)
	c.rt.mu.Lock()
	local, isLocal := c.rt.services[id]
	methods, isHost := c.rt.hosts[id]
	c.rt.mu.Unlock()
	var service *Service
	switch {
	case isLocal:
		service = localService(name, local)
	case isHost:
		service = c.rt.hostService(name, id, methods)
	default:
		return fmt.Errorf("service %s is not available", name)
	}
	return service.bind(target)
}

// Provide provides `value` as the service `name` until the plugin unloads,
// or until the returned function is called. Provides must declare `name`,
// and `value` must be of the type declared there; otherwise Provide panics
// (in Apply, the load fails).
func (c *Ctx) Provide(name string, value any) func() {
	return c.rt.provide(c.row, name, value)
}

// Effect registers a cleanup, run when the plugin unloads (latest first).
func (c *Ctx) Effect(cleanup func(context.Context) error) {
	c.row.cleanups = append(c.row.cleanups, cleanup)
}

// Go runs `task` on a goroutine of its own: a panic is recovered and
// written to stderr; when the plugin unloads, the context is cancelled and
// the unload waits for `task` to return.
func (c *Ctx) Go(task func(context.Context) error) {
	c.row.tasks.Add(1)
	go func() {
		defer c.row.tasks.Done()
		if _, err := peer.Protect(func() (any, error) { return nil, task(c.Context) }); err != nil && !errors.Is(err, context.Canceled) {
			fmt.Fprintf(os.Stderr, "rutis: a task of row %s failed: %v\n", c.row.key, err)
		}
	}()
}

func contains(list []string, item string) bool {
	for _, x := range list {
		if x == item {
			return true
		}
	}
	return false
}

// ── Services ─────────────────────────────────────────────────────

// Service is a service used by name.
type Service struct {
	name    string
	methods map[string]string
	local   *providedService
	call    func(ctx context.Context, method string, args []any) (any, error)
}

// Methods maps the service's methods to "sync" or "async".
func (s *Service) Methods() map[string]string { return s.methods }

// Local reports whether a plugin in this process provides the service:
// then calls reach the provider's object directly, without the session.
func (s *Service) Local() bool { return s.local != nil }

// Call calls `method` with `args` and decodes its result into `result` (a
// pointer, or nil).
func (s *Service) Call(ctx context.Context, method string, result any, args ...any) error {
	if ctx == nil {
		panic("rutis: a nil context: pass the context.Context your caller gave you")
	}
	if _, ok := s.methods[method]; !ok {
		return fmt.Errorf("%s has no method %s", s.name, method)
	}
	converted, err := convert.ToPeerArgs(args)
	if err != nil {
		return err
	}
	value, err := s.call(ctx, method, converted)
	if err != nil {
		return err
	}
	if value, err = peer.Settle(ctx, value); err != nil {
		return err
	}
	return convert.Decode(value, result)
}

func localService(name string, local providedService) *Service {
	value := reflect.ValueOf(local.value)
	methods := map[string]string{}
	for wire := range local.shape.wires(value) {
		// Not declared: async, as rutis sees it.
		kind := "async"
		if local.shape != nil {
			kind = local.shape.methods[wire].kind
		}
		methods[wire] = kind
	}
	return &Service{
		name:    name,
		methods: methods,
		local:   &local,
		call: func(ctx context.Context, method string, args []any) (any, error) {
			fn, ok := local.shape.method(value, method)
			if !ok {
				return nil, fmt.Errorf("%s has no method %s", name, method)
			}
			return convert.Call(ctx, fn, args)
		},
	}
}

func (rt *runtimeState) hostService(name, id string, methods map[string]string) *Service {
	return &Service{
		name:    name,
		methods: methods,
		call: func(ctx context.Context, method string, args []any) (any, error) {
			rt.warnChainless(ctx)
			session := rt.session
			if session == nil {
				return nil, errors.New("the runtime has no session")
			}
			value, err := session.Call(ctx, "host:"+id, method, args, methods[method] == "sync")
			if err != nil {
				return nil, err
			}
			return peer.Settle(ctx, value)
		},
	}
}

var serviceType = reflect.TypeFor[Service]()

// bind fills `target`: a *Service, or a pointer to a struct of function
// fields.
func (s *Service) bind(target any) error {
	pointer := reflect.ValueOf(target)
	if pointer.Kind() != reflect.Pointer || pointer.IsNil() {
		return fmt.Errorf("use %s: the target must be a non-nil pointer", s.name)
	}
	value := pointer.Elem()
	if value.Type() == serviceType {
		value.Set(reflect.ValueOf(*s))
		return nil
	}
	if value.Type() == reflect.TypeFor[*Service]() {
		value.Set(reflect.ValueOf(s))
		return nil
	}
	if value.Kind() != reflect.Struct {
		return fmt.Errorf("use %s: the target must point to a struct of function fields or a *rutis.Service, not %s", s.name, value.Type())
	}
	for _, field := range reflect.VisibleFields(value.Type()) {
		if !field.IsExported() || field.Anonymous {
			continue
		}
		if field.Type.Kind() != reflect.Func {
			return fmt.Errorf("use %s: field %s is not a function", s.name, field.Name)
		}
		wire := convert.WireName(field.Name)
		if tag, ok := field.Tag.Lookup("rutis"); ok && tag != "" {
			wire = tag
		}
		if _, declared := s.methods[wire]; !declared {
			return fmt.Errorf("use %s: field %s calls %s, which the service does not declare", s.name, field.Name, wire)
		}
		if err := convert.CheckSignature(field.Type, true); err != nil {
			return fmt.Errorf("use %s: field %s: %w", s.name, field.Name, err)
		}
		if s.local != nil {
			if fn, ok := s.local.shape.method(reflect.ValueOf(s.local.value), wire); ok && fn.Type() == field.Type {
				value.FieldByIndex(field.Index).Set(fn)
				continue
			}
		}
		method := wire
		value.FieldByIndex(field.Index).Set(convert.MakeFunc(field.Type, func(ctx context.Context, args []any) (any, error) {
			return s.call(ctx, method, args)
		}))
	}
	return nil
}

var (
	warned   sync.Map
	devMode  = os.Getenv("RUTIS_DEV") == "1"
	selfPath = reflect.TypeFor[runtimeState]().PkgPath()
)

// warnChainless warns, in development mode, of a call without a call chain
// made while an incoming synchronous call runs: a context was not passed
// on, and a far end waiting for this process may wait forever.
func (rt *runtimeState) warnChainless(ctx context.Context) {
	if !devMode || len(peer.ChainOf(ctx)) > 0 || rt.session == nil || rt.session.Incoming() == 0 {
		return
	}
	pcs := make([]uintptr, 32)
	frames := goruntime.CallersFrames(pcs[:goruntime.Callers(2, pcs)])
	for {
		frame, more := frames.Next()
		if !strings.HasPrefix(frame.Function, "reflect.") && !strings.HasPrefix(frame.Function, selfPath) && !strings.HasPrefix(frame.Function, "runtime.") {
			site := fmt.Sprintf("%s:%d", frame.File, frame.Line)
			if _, seen := warned.LoadOrStore(site, true); !seen {
				fmt.Fprintf(os.Stderr, "rutis: a call at %s carries no call chain while a call into this process runs: pass on the context.Context the plugin was given\n", site)
			}
			return
		}
		if !more {
			return
		}
	}
}
