// Package rutistest tests a plugin without a host: it gives the plugin the
// services it injects, calls the services it provides, emits and listens
// to events as a host does, and unloads it when the test ends.
//
//	func TestWeather(t *testing.T) {
//		loaded := rutistest.Load(t, weather.Plugin, weather.Config{City: "Oslo"}, map[string]any{"llm": &FakeLLM{}})
//		var today string
//		if err := loaded.Service("weather").Call(context.Background(), "today", &today); err != nil {
//			t.Fatal(err)
//		}
//	}
//
// The plugin runs in its real runtime, across a real session: values cross
// as they would between processes (data is copied, time.Time, []byte and
// the other tagged values cross as their values, functions and Ref
// objects pass by reference), so what works only in one process fails here
// too; the test side takes every tagged value, as a host does. It checks what a host would: the plugin uses only the services it
// declares in Inject, provides what Provides declares with the declared
// types, and its cleanups succeed. A service Provides does not declare is
// used as rutis uses it: its methods are the value's exported ones, all
// asynchronous.
//
// Events: the test side is the host of the event bus (#464). The plugin's
// listeners register with it; Loaded's Emit, Parallel, Serial, Waterfall,
// Bail and Filter dispatch to them from that row's scope, as a host
// plugin would. Loaded.On listens as the host: the plugin's emissions of
// that name then go through the test side, as they go through rutis when
// a listener is in another process. More rows of the plugin load into the
// same runtime with Loaded.Load, each in a scope of its own (InScope).
package rutistest

import (
	"context"
	"encoding/json"
	"fmt"
	"maps"
	"net"
	"reflect"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/events"
	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

const (
	firstRow = "test"
	// What the test side calls the runtime, in descriptors, unless
	// RuntimeName says.
	defaultRuntime = "go"
)

// Option adjusts Load and Loaded.Load.
type Option func(*options)

type options struct {
	row     string
	scope   []rutis.Scope
	runtime string
}

// Row names the row (by default "test", then "test-2", "test-3", ...).
func Row(key string) Option { return func(o *options) { o.row = key } }

// RuntimeName is the runtime's name in descriptors (Listener.Runtime), "go"
// by default, as a host names the runtime it mounts. Only Load's counts:
// every row is in the one runtime.
func RuntimeName(name string) Option { return func(o *options) { o.runtime = name } }

// InScope loads the row into the scope `path` ("t1/s1"): one loader
// instance per segment, each instance's id the path up to it and its group
// the segment, unless `groups` names the group of each segment.
func InScope(path string, groups ...string) Option {
	return func(o *options) {
		o.scope = nil
		if path == "" || path == "root" {
			return
		}
		segments := strings.Split(path, "/")
		for i, segment := range segments {
			group := segment
			if i < len(groups) {
				group = groups[i]
			}
			o.scope = append(o.scope, rutis.Scope{ID: strings.Join(segments[:i+1], "/"), Group: group})
		}
	}
}

// InChain loads the row into the scope chain `chain`, outermost first.
func InChain(chain ...rutis.Scope) Option {
	return func(o *options) { o.scope = append([]rutis.Scope(nil), chain...) }
}

// Loaded is a row of a plugin loaded for a test.
type Loaded struct {
	h      *harness
	row    string
	scope  []rutis.Scope
	unload sync.Once
	// What its plugin declares: the harness's plugin's, or, for a copy of
	// a plugin definition the plugin handed over, that definition's.
	declared declaration
	// A copy of a plugin definition (loadCopy), and, when known, the row
	// (and its generation) the definition belongs to.
	copy   bool
	known  bool
	origin belonging
	// What descriptors and reported lines call the row: its key, or, for
	// a copy, the id of the row its definition belongs to (as the host
	// calls a copy, R5).
	id string
	// The labels the row uses names under: what it injects under its
	// lease (leaseLabel), and, for a copy, what it declares it provides
	// under a label of the copy's own (copyLabel).
	isolated map[string]string
	// What it is loaded from, to load it again (Update): the plugin's
	// name or the definition, the services it exports, the services
	// given to a copy alone, and its options.
	entry   any
	exports map[string]map[string]string
	own     map[string]any
	options options
	// Where its failures go: the TB it was loaded with.
	t testing.TB
}

// harness is the runtime under test and the host side of its session.
type harness struct {
	t      testing.TB
	plugin *rutis.Definition
	// The name rows load the plugin by: its own, or, for one without a
	// name (a definition only handed over), a name it is served under.
	entry    string
	declared declaration
	services map[string]any
	// The services given to a copy alone, by scoped id (name, NUL, label).
	own map[string]any
	// The row (and its generation) each plugin definition belongs to, as
	// the host knows it (R6): the row whose lease the call handing it over
	// came through, the row providing the service that returned it, or the
	// row whose emission carried it.
	origins map[*peer.RemotePlugin]belonging
	// Each row key's generation: one more at each load and update, so a
	// definition of an earlier generation (an updated row, or an earlier
	// row of the same key) is known to have ended.
	generations map[string]int
	// The row providing each exported or announced service, by id.
	providers map[string]string
	conn      net.Conn
	session   *peer.Peer
	done      chan error
	// The runtime's name in descriptors (mount `runtime`).
	runtime string

	mu      sync.Mutex
	handles map[string]string
	// The methods of each service announced without a declaration.
	announced map[string]map[string]string
	changed   chan struct{}
	// The rows, in load order, with their scopes.
	rows  []*Loaded
	scope map[string][]rutis.Scope
	// The bus: every listener, in position order.
	entries     []*busEntry
	first, last int64
	dispatches  int64
	reported    []string
	closed      bool
	// Serializes events.elsewhere, which is sent without mu.
	pushing sync.Mutex
}

// busEntry is a listener on the test side's bus: one of the runtime's (by
// its id) or one of the test's (fn).
type busEntry struct {
	h         *harness
	id        string
	name      string
	row       string
	plugin    string
	global    bool
	fn        func(context.Context, ...any) (any, error)
	position  int64
	withdrawn bool
	queue     events.Queue
}

var (
	harnessesMu sync.Mutex
	harnesses   = map[*harness]bool{}
	hookOnce    sync.Once
)

// hook collects the lines the runtime reports (§5.6) for every harness.
func hook() {
	hookOnce.Do(func() {
		previous := events.SetReport(nil)
		events.SetReport(func(line string) {
			harnessesMu.Lock()
			for h := range harnesses {
				h.mu.Lock()
				h.reported = append(h.reported, line)
				h.mu.Unlock()
			}
			harnessesMu.Unlock()
			previous(line)
		})
	})
}

// Load loads `plugin` with `config` (a value of its config type, or nil)
// and the services in `services` (Go values whose exported methods are the
// service's methods). The plugin is unloaded when the test ends.
//
// `plugin` may also be a plugin definition a loaded plugin handed to a
// test service (a *rutis.Definition parameter of its method): it is then
// loaded as rutis mounts one, as another row in the runtime that defined
// it, with what its `apply` prepared there (docs/design-plugin-values
// R10). `services` are that copy's own, as the services of the instance
// rutis mounts a copy in; a name it injects that `services` leaves out is
// the runtime's, given to the plugin that handed it over. As under rutis:
//   - the copy's rows are called by the id of the row the definition
//     belongs to in descriptors and reported lines (a copy of a copy: by
//     the first row's; a row not known: by the definition's name, or
//     "plugin");
//   - the services the copy declares in Provides are its own, so several
//     copies of a definition can be loaded at once when they provide only
//     declared services (an undeclared one's name is not known in
//     advance, and is not isolated);
//   - a definition belongs to the row that handed it to a test service,
//     returned it from a service of its own, or emitted it. When that row
//     unloads or is updated (a restart), its copies unload first, and
//     loading the definition fails from then on. A definition that reached
//     the test any other way (the result of an emission the test made)
//     belongs to no row the test knows of, and this is not checked;
//   - a copy's config is fixed when it is mounted: Update and Load on it
//     fail.
func Load(t testing.TB, plugin *rutis.Definition, config any, services map[string]any, opts ...Option) *Loaded {
	t.Helper()
	if remote, ok := remoteOf(plugin); ok {
		return loadCopy(t, remote, config, services, opts)
	}
	// A definition only handed over may have no name: it is served under
	// one of the test's, which only rows.load sees.
	if plugin == nil {
		t.Fatal("rutistest: a nil plugin definition")
	}
	entry := plugin.Name()
	if entry == "" {
		entry = servedName
		plugin = convert.PluginServedAs(plugin, entry).(*rutis.Definition)
	}
	declared := describe(t, plugin, entry)
	for _, name := range declared.Inject {
		if _, ok := services[name]; !ok {
			t.Fatalf("rutistest: the plugin injects %s: give the test a service %s", name, name)
		}
	}
	hook()
	o := options{runtime: defaultRuntime}
	for _, option := range opts {
		option(&o)
	}
	runtimeSide, hostSide := net.Pipe()
	h := &harness{
		runtime:  o.runtime,
		t:        t,
		plugin:   plugin,
		entry:    entry,
		declared: declared,
		// Copies of plugin definitions may add services (loadCopy).
		services:    maps.Clone(services),
		own:         map[string]any{},
		origins:     map[*peer.RemotePlugin]belonging{},
		generations: map[string]int{},
		providers:   map[string]string{},
		conn:        hostSide,
		done:        make(chan error, 1),
		handles:     map[string]string{},
		announced:   map[string]map[string]string{},
		changed:     make(chan struct{}, 1),
		scope:       map[string][]rutis.Scope{},
	}
	go func() {
		err := rutis.ServeConn(runtimeSide, plugin)
		// A runtime that ended early reads nothing more: closing its end
		// fails the test side's writes at once instead of hanging them.
		runtimeSide.Close()
		h.done <- err
	}()
	session, err := peer.New(peer.NewLines(hostSide), peer.Options{
		Host:     true,
		Dispatch: h.dispatch,
		Arrive:   h.arrive,
		// Plugin definitions the plugin hands to test services, as rutis
		// takes them.
		Plugins: true,
	})
	if err != nil {
		t.Fatal(err)
	}
	h.session = session
	if err := session.Start(); err != nil {
		h.ended(err)
	}
	guard := time.NewTimer(testwait.HangGuard)
	defer guard.Stop()
	select {
	case <-session.Ready():
	case err := <-h.done:
		t.Fatalf("rutistest: the runtime ended before the handshake: %v", err)
	case <-guard.C:
		t.Fatalf("timed out (%v): rutistest: the handshake", testwait.HangGuard)
	}
	harnessesMu.Lock()
	harnesses[h] = true
	harnessesMu.Unlock()
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	// As a host does: the names with listeners elsewhere before the mount
	// (none yet), and services the plugin does not declare are announced.
	if _, err := session.Call(ctx, "", "events.elsewhere", []any{map[string]any{"add": []any{}}}, false); err != nil {
		t.Fatalf("rutistest: events.elsewhere: %v", err)
	}
	if _, err := session.Call(ctx, "", "mount", []any{map[string]any{"announce": true, "runtime": h.runtime}}, false); err != nil {
		t.Fatalf("rutistest: mount: %v", err)
	}
	loaded := h.load(config, opts)
	t.Cleanup(h.close)
	return loaded
}

// servedName is what a plugin without a name is served under.
const servedName = "rutistest-plugin"

// ended fails the test with why the runtime ended, when it did: an error
// talking to it (`err`) is only its consequence.
func (h *harness) ended(err error) {
	h.t.Helper()
	select {
	case cause := <-h.done:
		h.t.Fatalf("rutistest: the runtime ended: %v", cause)
	case <-time.After(time.Second):
		h.t.Fatal(err)
	}
}

// remoteOf is the plugin definition another process handed over that
// `plugin` stands for, if it is one.
func remoteOf(plugin *rutis.Definition) (*peer.RemotePlugin, bool) {
	if plugin == nil {
		return nil, false
	}
	value, err := convert.ToPeer(plugin)
	if err != nil {
		return nil, false
	}
	remote, ok := value.(*peer.RemotePlugin)
	return remote, ok
}

// loadCopy loads a copy of a plugin definition a loaded plugin handed
// over, in the runtime that defined it, as rutis mounts one: described
// with `plugins.read`, loaded with the definition as the row's entry.
func loadCopy(t testing.TB, remote *peer.RemotePlugin, config any, services map[string]any, opts []Option) *Loaded {
	t.Helper()
	var h *harness
	harnessesMu.Lock()
	for candidate := range harnesses {
		if candidate.session == remote.Peer() {
			h = candidate
		}
	}
	harnessesMu.Unlock()
	if h == nil {
		t.Fatal("rutistest: the plugin definition comes from a runtime that is no longer loaded")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	described, err := h.session.Call(ctx, "", "plugins.read", []any{remote}, false)
	if err != nil {
		t.Fatalf("rutistest: plugins.read: %v", err)
	}
	var declared declaration
	if err := convert.Decode(described, &declared); err != nil {
		t.Fatalf("rutistest: plugins.read: %v", err)
	}
	// A definition belongs to the row that handed it over (R6): once that
	// row unloaded or restarted, rutis cannot mount it any more.
	h.mu.Lock()
	from, known := h.origins[remote]
	_, live := h.scope[from.row]
	current := live && h.generations[from.row] == from.generation
	h.mu.Unlock()
	if known && !current {
		t.Fatalf("rutistest: the row %s that handed this plugin definition over has since unloaded or restarted: a definition ends with the generation of the row that handed it over, and its copies with it", from.row)
	}
	// The services given here are this copy's own, as the services of the
	// instance rutis mounts a copy in; those not given are the test's.
	for _, name := range declared.Inject {
		if _, ok := services[name]; ok {
			continue
		}
		h.mu.Lock()
		_, provided := h.services[name]
		h.mu.Unlock()
		if !provided {
			t.Fatalf("rutistest: the plugin definition injects %s: give the test a service %s", name, name)
		}
	}
	// Descriptors call a copy's row as the host does: by the id of the row
	// its definition belongs to (a copy of a copy, by the first one's), or,
	// that not known or empty (a runtime takes an empty id for the copy's
	// own key), by the definition's name, or "plugin".
	id := ""
	if known {
		h.mu.Lock()
		for _, row := range h.rows {
			if row.row == from.row {
				id = row.id
			}
		}
		h.mu.Unlock()
	}
	if id == "" && declared.Name != nil {
		id = *declared.Name
	}
	if id == "" {
		id = "plugin"
	}
	spec := &Loaded{declared: declared, copy: true, known: known, origin: from, id: id, entry: remote, exports: declared.Provides, own: services}
	loaded := h.loadEntry(t, spec, config, opts)
	t.Cleanup(loaded.Unload)
	return loaded
}

// belonging is the row a plugin definition belongs to, and its generation.
type belonging struct {
	row        string
	generation int
}

// leaseLabel is the label row `row` uses the services it injects under,
// as a host leases them per row (rutis-bridge's row_label): U+0001 sets it
// apart from any label a test gives.
func leaseLabel(row string) string { return "\x01row:" + row }

// copyLabel is the label a copy uses the services it provides under, as a
// copy in an instance of its own.
func copyLabel(row string) string { return "rutistest/copy/" + row }

// remember records that the plugin definitions in `value` belong to `row`
// in its current generation, unless they already belong to a row; h.mu is
// held. A row unloading (handing one over in its cleanup) is still in that
// generation, which has ended: loadCopy refuses it.
func (h *harness) remember(value any, row string) {
	for _, remote := range remotes(value, nil) {
		if _, seen := h.origins[remote]; !seen {
			h.origins[remote] = belonging{row, h.generations[row]}
		}
	}
}

// Load loads another row of the same plugin into the same runtime, with
// `config`; give it a scope with InScope to test how events cross scopes.
// It unloads when the test ends.
func (l *Loaded) Load(config any, options ...Option) *Loaded {
	l.h.t.Helper()
	if l.copy {
		l.h.t.Fatalf("rutistest: row %s is a copy of a plugin definition: load the definition again with rutistest.Load", l.row)
	}
	return l.h.load(config, options)
}

func (h *harness) load(config any, list []Option) *Loaded {
	h.t.Helper()
	// The first row exports what the plugin declares.
	var exports map[string]map[string]string
	h.mu.Lock()
	if len(h.rows) == 0 {
		exports = h.declared.Provides
	}
	h.mu.Unlock()
	return h.loadEntry(h.t, &Loaded{declared: h.declared, entry: h.entry, exports: exports}, config, list)
}

// loadEntry loads a row of `spec.entry`: the plugin's name, or a plugin
// definition it handed over (spec.copy). Failures are `t`'s: the
// caller's.
//
// As a host leases services per row, each row uses the services it
// injects under a label of its own (leaseLabel): the test side knows from
// it which row a call came from. spec.own are the services given to a
// copy alone. A copy also isolates the services it declares it provides,
// so two copies of a definition providing only declared services can be
// loaded at once, as in two instances; what a copy provides without
// declaring it is not known in advance and is not isolated.
func (h *harness) loadEntry(t testing.TB, spec *Loaded, config any, list []Option) *Loaded {
	t.Helper()
	h.mu.Lock()
	o := options{row: firstRow}
	if n := len(h.rows); n > 0 {
		o.row = firstRow + "-" + strconv.Itoa(n+1)
	}
	h.mu.Unlock()
	for _, option := range list {
		option(&o)
	}
	spec.h, spec.t, spec.options = h, t, o
	spec.row, spec.scope = o.row, o.scope
	if !spec.copy {
		spec.id = o.row
	}
	h.start(spec, config)
	return spec
}

// start loads row `l` with `config`: a new generation of its key.
func (h *harness) start(l *Loaded, config any) {
	t := l.t
	t.Helper()
	h.mu.Lock()
	if _, taken := h.scope[l.row]; taken {
		h.mu.Unlock()
		t.Fatalf("rutistest: a row %s is already loaded", l.row)
	}
	h.scope[l.row] = l.scope
	h.rows = append(h.rows, l)
	h.generations[l.row]++
	type lease struct {
		name  string
		kinds map[string]string
	}
	var leases []lease
	isolate := []any{}
	label := leaseLabel(l.row)
	l.isolated = map[string]string{}
	for _, name := range l.declared.Inject {
		service, given := l.own[name]
		if given {
			h.own[name+"\x00"+label] = service
		} else {
			service = h.services[name]
		}
		kinds := map[string]string{}
		for wire := range convert.Methods(reflect.TypeOf(service)) {
			kinds[wire] = "sync"
		}
		leases = append(leases, lease{name, kinds})
		isolate = append(isolate, []any{name, label})
		l.isolated[name] = label
	}
	if l.copy {
		for name := range l.declared.Provides {
			if _, injected := l.isolated[name]; !injected {
				isolate = append(isolate, []any{name, copyLabel(l.row)})
				l.isolated[name] = copyLabel(l.row)
			}
		}
	}
	exports := map[string]any{}
	for name, kinds := range l.exports {
		exports[name] = kinds
		h.providers[l.serviceID(name)] = l.row
	}
	h.mu.Unlock()
	// A row that fails to load forgets every service given to it alone,
	// and gives back the leases it took.
	var leased []string
	failed := func(format string, args ...any) {
		h.forgetRow(l.row)
		h.forgetOwn(l.row)
		h.withdraw(t, l.row, leased)
		t.Fatalf(format, args...)
	}
	configValue, err := convert.ToPeer(config)
	if err != nil {
		failed("rutistest: config: %v", err)
	}
	if configValue == nil {
		configValue = json.RawMessage("null")
	}
	scope := make([]any, len(l.scope))
	for i, s := range l.scope {
		scope[i] = map[string]any{"id": s.ID, "group": s.Group}
	}
	placed := map[string]any{"scope": scope, "id": l.id}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	for _, lease := range leases {
		if _, err := h.session.Call(ctx, "", "hosts.provide", []any{lease.name, lease.kinds, label}, false); err != nil {
			failed("rutistest: provide %s: %v", lease.name, err)
		}
		leased = append(leased, lease.name)
	}
	if _, err := h.session.Call(ctx, "", "rows.load", []any{l.row, l.entry, configValue, isolate, []any{}, exports, placed}, false); err != nil {
		failed("rutistest: load %s: %v", l.row, err)
	}
}

// stop unloads row `l`, as a host does: the copies of the definitions it
// handed over first (R6), its listeners withdrawn, the row unloaded, then
// its leases given back (its cleanups still use them).
func (h *harness) stop(l *Loaded) {
	h.unloadCopiesOf(l.row)
	h.forgetRow(l.row)
	h.mu.Lock()
	closed := h.closed
	h.mu.Unlock()
	if !closed {
		ctx, cancel := context.WithTimeout(context.Background(), unloadWait())
		defer cancel()
		if _, err := h.session.Call(ctx, "", "rows.unload", []any{l.row}, false); err != nil {
			l.t.Errorf("rutistest: unload %s: %v", l.row, err)
		}
	}
	h.forgetOwn(l.row)
	h.withdraw(l.t, l.row, l.declared.Inject)
}

// serviceID is the id the row uses the service `name` by: scoped by the label it
// isolates it under, if any.
func (l *Loaded) serviceID(name string) string {
	if label, ok := l.isolated[name]; ok {
		return name + "\x00" + label
	}
	return name
}

// forgetRow withdraws a row's listeners from the bus, as a host does when
// it unloads the row.
func (h *harness) forgetRow(row string) {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.withdrawRow(row)
	delete(h.scope, row)
	h.rows = slices.DeleteFunc(h.rows, func(l *Loaded) bool { return l.row == row })
}

// forgetOwn forgets the services given to row `row` alone.
func (h *harness) forgetOwn(row string) {
	h.mu.Lock()
	defer h.mu.Unlock()
	// By the label at the end: a name may hold a NUL (and be refused).
	for id := range h.own {
		if strings.HasSuffix(id, "\x00"+leaseLabel(row)) {
			delete(h.own, id)
		}
	}
}

// withdraw gives back the leases of row `row` on the services `names`;
// failures are `t`'s.
func (h *harness) withdraw(t testing.TB, row string, names []string) {
	h.mu.Lock()
	closed := h.closed
	h.mu.Unlock()
	if closed {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	for _, name := range names {
		if _, err := h.session.Call(ctx, "", "hosts.withdraw", []any{name + "\x00" + leaseLabel(row)}, false); err != nil {
			t.Errorf("rutistest: withdraw %s: %v", name, err)
		}
	}
}

type declaration struct {
	// Its name, as plugins.read answers it (null: none).
	Name     *string                      `json:"name"`
	Inject   []string                     `json:"inject"`
	Provides map[string]map[string]string `json:"provides"`
}

func describe(t testing.TB, plugin *rutis.Definition, entry string) declaration {
	data, err := rutis.Manifest(plugin)
	if err != nil {
		t.Fatal(err)
	}
	var manifest struct {
		Plugins map[string]declaration `json:"plugins"`
	}
	if err := json.Unmarshal(data, &manifest); err != nil {
		t.Fatal(err)
	}
	return manifest.Plugins[entry]
}

// arrive takes the runtime's registrations in frame order (§4.1).
func (h *harness) arrive(_ context.Context, method string, args []any) func() (any, error) {
	var err error
	switch method {
	case "events.on":
		err = h.register(args)
	case "events.off":
		var ids []string
		if err = convert.Decode(index(args, 0), &ids); err == nil {
			h.mu.Lock()
			h.entries = slices.DeleteFunc(h.entries, func(e *busEntry) bool {
				if e.fn == nil && slices.Contains(ids, e.id) {
					e.withdrawn = true
					return true
				}
				return false
			})
			h.mu.Unlock()
		}
	default:
		return nil
	}
	return func() (any, error) { return nil, err }
}

func (h *harness) register(args []any) error {
	var registration struct {
		ID      string  `json:"id"`
		Name    string  `json:"name"`
		Row     *string `json:"row"`
		Plugin  string  `json:"plugin"`
		Kind    string  `json:"kind"`
		Prepend bool    `json:"prepend"`
		Global  bool    `json:"global"`
	}
	if err := convert.Decode(index(args, 0), &registration); err != nil {
		return fmt.Errorf("events.on: %w", err)
	}
	if strings.HasPrefix(registration.Name, "internal/") {
		return peer.Errorf("BindingError", "event %s is internal to its runtime", registration.Name)
	}
	row := ""
	if registration.Row != nil {
		row = *registration.Row
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	if _, loaded := h.scope[row]; !loaded {
		return peer.Errorf("BindingError", "row %s is not loaded in this runtime", row)
	}
	h.add(&busEntry{h: h, id: registration.ID, name: registration.Name, row: row, plugin: registration.Plugin, global: registration.Global}, registration.Prepend)
	return nil
}

// add puts an entry on the bus; h.mu is held.
func (h *harness) add(e *busEntry, prepend bool) {
	if prepend {
		h.first--
		e.position = h.first
		h.entries = append([]*busEntry{e}, h.entries...)
		return
	}
	h.last++
	e.position = h.last
	h.entries = append(h.entries, e)
}

func (h *harness) dispatch(ctx context.Context, target, method string, args []any) (any, error) {
	if target == "" && method == "service" {
		var id, handle string
		var announced struct {
			Methods map[string]string
			Row     string `json:"row"`
		}
		_ = convert.Decode(index(args, 0), &id)
		_ = convert.Decode(index(args, 1), &handle)
		_ = convert.Decode(index(args, 3), &announced)
		h.mu.Lock()
		if handle == "" {
			delete(h.handles, id)
		} else {
			h.handles[id] = handle
			if announced.Methods != nil {
				h.announced[id] = announced.Methods
			}
			if announced.Row != "" {
				h.providers[id] = announced.Row
			}
		}
		h.mu.Unlock()
		select {
		case h.changed <- struct{}{}:
		default:
		}
		return nil, nil
	}
	if target == "" && method == "events.dispatch" {
		return h.dispatchFromRuntime(ctx, args)
	}
	id, ok := strings.CutPrefix(target, "host:")
	if !ok {
		return nil, fmt.Errorf("the test serves no %s %s", target, method)
	}
	// Each row uses its services under its lease: the row the call came
	// from, which a plugin definition among the arguments belongs to.
	name, label, _ := strings.Cut(id, "\x00")
	h.mu.Lock()
	service, ok := h.own[id]
	if !ok {
		service, ok = h.services[name]
	}
	if row, leased := strings.CutPrefix(label, leaseLabel("")); leased {
		h.remember(args, row)
	}
	h.mu.Unlock()
	if !ok {
		return nil, fmt.Errorf("the test gives no service %s", name)
	}
	value := reflect.ValueOf(service)
	index, ok := convert.Methods(value.Type())[method]
	if !ok {
		return nil, fmt.Errorf("the test's %s has no method %s", name, method)
	}
	return convert.Call(ctx, value.Method(index), args)
}

// remotes are the plugin definitions in `value`, a call's arguments.
func remotes(value any, found []*peer.RemotePlugin) []*peer.RemotePlugin {
	switch v := value.(type) {
	case *peer.RemotePlugin:
		return append(found, v)
	case []any:
		for _, item := range v {
			found = remotes(item, found)
		}
	case map[string]any:
		for _, item := range v {
			found = remotes(item, found)
		}
	}
	return found
}

func index(args []any, i int) any {
	if i < len(args) {
		return args[i]
	}
	return nil
}

// ── The bus ──────────────────────────────────────────────────────

// invocation is an entry within one dispatch.
type invocation struct {
	*busEntry
	dispatch string
	own      bool
}

func (e *busEntry) Info() rutis.Listener {
	if e.fn != nil {
		return rutis.Listener{Name: e.name, Plugin: "rutistest", Global: e.global}
	}
	e.h.mu.Lock()
	// A copy: a filter changing it changes nothing else.
	scope := slices.Clone(e.h.scope[e.row])
	e.h.mu.Unlock()
	return rutis.Listener{Name: e.name, Plugin: e.plugin, Row: e.row, Runtime: e.h.runtime, Scope: scope, Global: e.global}
}

func (e *busEntry) live() bool {
	e.h.mu.Lock()
	defer e.h.mu.Unlock()
	return !e.withdrawn
}

func (i *invocation) Begin() (events.Call, bool) {
	if !i.live() {
		return nil, false
	}
	return i.call, true
}

func (i *invocation) call(ctx context.Context, mode events.Mode, args []any, next events.Next) (any, error) {
	if i.fn != nil {
		return i.local(ctx, mode, args, next)
	}
	value, err := i.h.session.Call(ctx, "", "events.call", []any{i.record(mode, args, next)}, mode == events.ModeBail || mode == events.ModeWaterfallSync)
	if err != nil {
		return nil, err
	}
	return peer.Settle(ctx, value)
}

// local runs one of the test's listeners.
func (i *invocation) local(ctx context.Context, mode events.Mode, args []any, next events.Next) (any, error) {
	plain := make([]any, 0, len(args)+1)
	for _, arg := range args {
		value, err := convert.Plain(arg)
		if err != nil {
			return nil, err
		}
		plain = append(plain, value)
	}
	if mode.Waterfall() {
		plain = append(plain, rutis.Next(func(ctx context.Context) (any, error) {
			value, err := next(ctx)
			if err != nil {
				return nil, err
			}
			return convert.Plain(value)
		}))
	}
	value, err := peer.Protect(func() (any, error) { return i.fn(ctx, plain...) })
	if err != nil {
		return nil, err
	}
	return convert.ToPeer(value)
}

// record is an `events.call` (§5.3).
func (i *invocation) record(mode events.Mode, args []any, next events.Next) map[string]any {
	record := map[string]any{"listener": i.id, "dispatch": i.dispatch, "own": i.own, "name": i.name, "mode": string(mode), "args": args}
	if next != nil {
		record["next"] = peer.Func(func(ctx context.Context, _ []any) (any, error) {
			value, err := next(ctx)
			if err != nil {
				return nil, err
			}
			return convert.ToPeer(value)
		})
	}
	return record
}

func (i *invocation) Emit(ctx context.Context, args []any) bool {
	if !i.live() {
		return false
	}
	if i.fn != nil {
		i.queue.Push(func() {
			if !i.live() {
				return
			}
			if _, err := i.local(ctx, events.ModeEmit, args, nil); err != nil {
				events.Report(fmt.Sprintf("rutis: listener of %s in row rutistest failed: %s", i.name, events.Failure(err)))
			}
		})
		return true
	}
	// Sent now, in order: the runtime starts them in arrival order.
	wait, err := i.h.session.Send(ctx, "", "events.call", []any{i.record(events.ModeEmit, args, nil)}, false)
	report := func(err error) {
		events.Report(fmt.Sprintf("rutis: listener of %s in runtime %s failed: %s", i.name, i.h.runtime, events.Failure(err)))
	}
	if err != nil {
		report(err)
		return true
	}
	go func() {
		if _, err := wait(); err != nil {
			report(err)
		}
	}()
	return true
}

// snapshot is a dispatch of `name` (§5.1); delivery is by name.
func (h *harness) snapshot(name string, _ []rutis.Scope, own bool) *events.Dispatch {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.dispatches++
	id := "d" + strconv.FormatInt(h.dispatches, 10)
	d := &events.Dispatch{Name: name}
	for _, e := range h.entries {
		if e.name == name {
			d.Entries = append(d.Entries, &invocation{busEntry: e, dispatch: id, own: own && e.fn == nil})
		}
	}
	return d
}

// run runs a dispatch in `mode`.
func run(ctx context.Context, d *events.Dispatch, mode events.Mode, args []any, terminal events.Terminal) (any, error) {
	switch mode {
	case events.ModeEmit:
		return nil, d.Emit(ctx, args)
	case events.ModeParallel:
		return nil, d.Parallel(ctx, args)
	case events.ModeWaterfall, events.ModeWaterfallSync:
		return d.Waterfall(ctx, mode, args, terminal)
	}
	return d.Serial(ctx, mode, args)
}

// dispatchFromRuntime serves the runtime's `events.dispatch` (§5.3).
func (h *harness) dispatchFromRuntime(ctx context.Context, args []any) (any, error) {
	var record map[string]any
	switch x := index(args, 0).(type) {
	case map[string]any:
		record = x
	case json.RawMessage:
		var raw map[string]json.RawMessage
		if err := json.Unmarshal(x, &raw); err != nil {
			return nil, err
		}
		record = map[string]any{}
		for key, value := range raw {
			record[key] = value
		}
	default:
		return nil, fmt.Errorf("events.dispatch needs a record")
	}
	var row, name, mode string
	for key, out := range map[string]any{"row": &row, "name": &name, "mode": &mode} {
		if err := convert.Decode(record[key], out); err != nil {
			return nil, fmt.Errorf("events.dispatch %s: %w", key, err)
		}
	}
	var values []any
	switch list := record["args"].(type) {
	case []any:
		values = list
	case nil:
	default:
		if err := convert.Decode(list, &values); err != nil {
			return nil, err
		}
	}
	m := events.Mode(mode)
	if !m.Valid() || m == events.ModeWaterfallSync {
		return nil, fmt.Errorf("events.dispatch: unknown mode %s", mode)
	}
	// A plugin definition among the arguments belongs to the emitting row.
	h.mu.Lock()
	h.remember(values, row)
	h.mu.Unlock()
	if err := events.CheckEmit(name); err != nil {
		return nil, err
	}
	h.mu.Lock()
	scope, loaded := h.scope[row]
	h.mu.Unlock()
	if !loaded {
		return nil, peer.Errorf("BindingError", "row %s is not loaded in this runtime", row)
	}
	d := h.snapshot(name, scope, true)
	if filter, ok := record["filter"].(*peer.RemoteFunction); ok {
		// One call of the emitter's selector, with every candidate the
		// filter applies to (§6).
		var infos []any
		for _, e := range d.Entries {
			if info := e.Info(); !info.Global {
				infos = append(infos, info)
			}
		}
		encoded, err := convert.ToPeer(infos)
		if err != nil {
			return nil, err
		}
		if encoded == nil {
			encoded = []any{}
		}
		answer, err := filter.Call(ctx, []any{encoded}, m == events.ModeBail)
		if err != nil {
			if m == events.ModeEmit {
				events.Report(fmt.Sprintf("rutis: filter of %s from row %s failed: %s", name, row, events.Failure(err)))
				return nil, nil
			}
			return nil, err
		}
		var answers []bool
		if err := convert.Decode(answer, &answers); err != nil || len(answers) != len(infos) {
			failure := peer.Errorf("BindingError", "filter of %s returned %d answers for %d listeners", name, len(answers), len(infos))
			if m == events.ModeEmit {
				events.Report(fmt.Sprintf("rutis: filter of %s from row %s failed: %s", name, row, events.Failure(failure)))
				return nil, nil
			}
			return nil, failure
		}
		asked := 0
		d.Keep = func(rutis.Listener) (bool, error) {
			keep := answers[asked]
			asked++
			return keep, nil
		}
	}
	var terminal events.Terminal
	if remote, ok := record["terminal"].(*peer.RemoteFunction); ok {
		// Called with the event's arguments (§5.4).
		terminal = func(ctx context.Context, args ...any) (any, error) {
			value, err := remote.Call(ctx, args, false)
			if err != nil {
				return nil, err
			}
			return peer.Settle(ctx, value)
		}
	}
	if m == events.ModeEmit {
		// Answered once every invocation started (§5.5).
		_, err := run(context.WithoutCancel(ctx), d, m, values, nil)
		return nil, err
	}
	value, err := run(ctx, d, m, values, terminal)
	if err != nil {
		return nil, err
	}
	return convert.ToPeer(value)
}

// ── What a test does ─────────────────────────────────────────────

// On listens to the event `name` as the host does, until the returned
// disposer is called or the test ends: the plugin's emissions of `name`
// reach `fn` (through the test side, as through rutis), and so do the
// test's own. `fn` gets the arguments as a listener in another process
// does, and in a waterfall a rutis.Next last. The listener is at the root
// scope.
func (l *Loaded) On(name string, fn func(ctx context.Context, args ...any) (any, error), options ...rutis.ListenOption) func() {
	l.h.t.Helper()
	if err := events.CheckListen(name); err != nil {
		l.h.t.Fatalf("rutistest: %v", err)
	}
	h := l.h
	e := &busEntry{h: h, name: name, fn: fn}
	var o events.ListenOptions
	for _, option := range options {
		option(&o)
	}
	prepend := o.Prepend
	e.global = o.Global
	h.pushing.Lock()
	h.mu.Lock()
	first := !h.hasTestListener(name)
	h.add(e, prepend)
	h.mu.Unlock()
	if first {
		h.elsewhere(map[string]any{"add": []any{name}})
	}
	h.pushing.Unlock()
	var once sync.Once
	return func() {
		once.Do(func() {
			h.pushing.Lock()
			defer h.pushing.Unlock()
			h.mu.Lock()
			e.withdrawn = true
			h.entries = slices.DeleteFunc(h.entries, func(other *busEntry) bool { return other == e })
			last := !h.hasTestListener(name)
			h.mu.Unlock()
			if last {
				h.elsewhere(map[string]any{"remove": []any{name}})
			}
		})
	}
}

// hasTestListener reports whether a test listener of `name` is on the bus;
// h.mu is held.
func (h *harness) hasTestListener(name string) bool {
	for _, e := range h.entries {
		if e.fn != nil && e.name == name {
			return true
		}
	}
	return false
}

// elsewhere tells the runtime of a change of the names with listeners
// outside it, and waits until it applied it.
func (h *harness) elsewhere(change map[string]any) {
	h.mu.Lock()
	closed := h.closed
	h.mu.Unlock()
	if closed {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if _, err := h.session.Call(ctx, "", "events.elsewhere", []any{change}, false); err != nil {
		h.t.Errorf("rutistest: events.elsewhere: %v", err)
	}
}

// Emit starts every listener of `name` on the bus and
// returns; a listener's failure is reported (Reported), not returned.
func (l *Loaded) Emit(ctx context.Context, name string, args ...any) error {
	_, err := l.emit(ctx, nil, name, events.ModeEmit, args, nil)
	return err
}

// Parallel runs every listener of `name` and waits for all; failures are
// a *rutis.AggregateError in registration order.
func (l *Loaded) Parallel(ctx context.Context, name string, args ...any) error {
	_, err := l.emit(ctx, nil, name, events.ModeParallel, args, nil)
	return err
}

// Serial runs the listeners of `name` one at a time and returns the first
// result.
func (l *Loaded) Serial(ctx context.Context, name string, args ...any) (any, error) {
	return l.emit(ctx, nil, name, events.ModeSerial, args, nil)
}

// Waterfall runs the listeners of `name` outermost first, each with
// `next`, the innermost `terminal`, called with `args` (nil: no result).
func (l *Loaded) Waterfall(ctx context.Context, name string, terminal rutis.Terminal, args ...any) (any, error) {
	return l.emit(ctx, nil, name, events.ModeWaterfall, args, terminal)
}

// Bail is Serial as a synchronous call.
func (l *Loaded) Bail(ctx context.Context, name string, args ...any) (any, error) {
	return l.emit(ctx, nil, name, events.ModeBail, args, nil)
}

// Filter dispatches only to the listeners `keep` accepts. An error `keep`
// returns (or a panic) fails the dispatch before any listener runs.
func (l *Loaded) Filter(keep func(rutis.Listener) (bool, error)) *Emitter {
	return &Emitter{l: l, keep: keep}
}

// Emitter is Loaded's emissions, filtered.
type Emitter struct {
	l    *Loaded
	keep func(rutis.Listener) (bool, error)
}

// Emit is Loaded.Emit, filtered.
func (e *Emitter) Emit(ctx context.Context, name string, args ...any) error {
	_, err := e.l.emit(ctx, e.keep, name, events.ModeEmit, args, nil)
	return err
}

// Parallel is Loaded.Parallel, filtered.
func (e *Emitter) Parallel(ctx context.Context, name string, args ...any) error {
	_, err := e.l.emit(ctx, e.keep, name, events.ModeParallel, args, nil)
	return err
}

// Serial is Loaded.Serial, filtered.
func (e *Emitter) Serial(ctx context.Context, name string, args ...any) (any, error) {
	return e.l.emit(ctx, e.keep, name, events.ModeSerial, args, nil)
}

// Waterfall is Loaded.Waterfall, filtered.
func (e *Emitter) Waterfall(ctx context.Context, name string, terminal rutis.Terminal, args ...any) (any, error) {
	return e.l.emit(ctx, e.keep, name, events.ModeWaterfall, args, terminal)
}

// Bail is Loaded.Bail, filtered.
func (e *Emitter) Bail(ctx context.Context, name string, args ...any) (any, error) {
	return e.l.emit(ctx, e.keep, name, events.ModeBail, args, nil)
}

func (l *Loaded) emit(ctx context.Context, keep func(rutis.Listener) (bool, error), name string, mode events.Mode, args []any, terminal rutis.Terminal) (any, error) {
	if err := events.CheckEmit(name); err != nil {
		return nil, err
	}
	values, err := convert.ToPeerArgs(args)
	if err != nil {
		return nil, err
	}
	d := l.h.snapshot(name, l.scope, false)
	if keep != nil {
		d.Keep = func(info rutis.Listener) (bool, error) {
			kept, err := peer.Protect(func() (any, error) { return keep(info) })
			if err != nil {
				return false, err
			}
			return kept.(bool), nil
		}
	}
	if mode == events.ModeEmit {
		ctx = context.WithoutCancel(ctx)
	}
	var last events.Terminal
	if terminal != nil {
		// With the arguments as a listener gets them.
		last = func(ctx context.Context, args ...any) (any, error) {
			plain := make([]any, len(args))
			for i, arg := range args {
				var err error
				if plain[i], err = convert.Plain(arg); err != nil {
					return nil, err
				}
			}
			return terminal(ctx, plain...)
		}
	}
	value, err := run(ctx, d, mode, values, last)
	if err != nil {
		return nil, err
	}
	return convert.Plain(value)
}

// Reported is what the runtime reported so far (§5.6): one line per
// failure of a listener an Emit reached, as the runtime writes it on
// stderr.
func (l *Loaded) Reported() []string {
	l.h.mu.Lock()
	defer l.h.mu.Unlock()
	return append([]string(nil), l.h.reported...)
}

// Scope is the row's scope chain.
func (l *Loaded) Scope() []rutis.Scope { return append([]rutis.Scope(nil), l.scope...) }

// ── Services ─────────────────────────────────────────────────────

// Provided lists the services the plugin provides now.
func (l *Loaded) Provided() []string {
	l.h.mu.Lock()
	defer l.h.mu.Unlock()
	names := make([]string, 0, len(l.h.handles))
	for id := range l.h.handles {
		name, label, scoped := strings.Cut(id, "\x00")
		// A copy's isolated services are its own.
		if scoped && label != l.isolated[name] {
			continue
		}
		names = append(names, name)
	}
	return names
}

// Service is the service `name` the plugin provides, as rutis sees it:
// its declared methods, or, undeclared, the value's exported methods, all
// asynchronous. The test fails if the plugin does not provide it.
func (l *Loaded) Service(name string) *Service {
	h := l.h
	h.t.Helper()
	// The notification may still be on its way when the load returned. A
	// notification signals h.changed after it changed h.handles, and the
	// signal is kept until taken: looking, then waiting, misses none.
	deadline := time.NewTimer(testwait.HangGuard)
	defer deadline.Stop()
	var handle string
	id := l.serviceID(name)
	for {
		h.mu.Lock()
		found, ok := h.handles[id]
		h.mu.Unlock()
		if ok {
			handle = found
			break
		}
		select {
		case <-h.changed:
		case <-deadline.C:
			h.t.Fatalf("rutistest: the plugin does not provide %s (now)", name)
		}
	}
	kinds, declared := l.declared.Provides[name]
	if !declared {
		h.mu.Lock()
		kinds = h.announced[id]
		h.mu.Unlock()
	}
	// What it returns belongs to the row providing it (R6).
	h.mu.Lock()
	provider, known := h.providers[id]
	h.mu.Unlock()
	if !known {
		provider = l.row
	}
	returned := func(value any) {
		h.mu.Lock()
		h.remember(value, provider)
		h.mu.Unlock()
	}
	return &Service{session: h.session, handle: handle, name: name, kinds: kinds, returned: returned}
}

// unloadWait is how long the test side waits for an unload: the runtime
// waits for running listeners up to RUTIS_SHUTDOWN_TIMEOUT (seconds,
// default 10), as under a host, then runs the cleanups.
func unloadWait() time.Duration { return events.ShutdownTimeout() + 30*time.Second }

// Unload unloads the row, failing the test if a cleanup failed. The
// runtime cancels its running listeners and waits for them first, up to
// RUTIS_SHUTDOWN_TIMEOUT seconds (default 10), as under a host; past it,
// it reports so (Reported) and unloads anyway. It runs when the test
// ends; calling it earlier is allowed.
func (l *Loaded) Unload() {
	l.unload.Do(func() { l.h.stop(l) })
}

// unloadCopiesOf unloads the copies of the definitions row `row` handed
// over (those whose row is known to be it), latest first.
func (h *harness) unloadCopiesOf(row string) {
	h.mu.Lock()
	var copies []*Loaded
	for _, other := range h.rows {
		if other.copy && other.known && other.origin.row == row {
			copies = append(copies, other)
		}
	}
	h.mu.Unlock()
	for i := len(copies) - 1; i >= 0; i-- {
		copies[i].Unload()
	}
}

// withdrawRow withdraws the runtime's listeners of `row` from the bus;
// h.mu is held.
func (h *harness) withdrawRow(row string) {
	h.entries = slices.DeleteFunc(h.entries, func(e *busEntry) bool {
		if e.fn == nil && e.row == row {
			e.withdrawn = true
			return true
		}
		return false
	})
}

// Update changes the row's config as rutis does for a leaf row: it
// restarts it. The copies of the definitions it handed over unload first,
// then the row (its listeners withdrawn, its cleanups run), and it loads
// again under the same key with `config`: a new generation, so its
// listeners register again, after every existing one, and the definitions
// it handed over before have ended (R6). rutis never sends a Go row
// rows.update either: a change of config is a restart. The row's cleanups
// run once it is no longer loaded, as under rutis: an event a cleanup
// emits fails.
func (l *Loaded) Update(config any) {
	h := l.h
	h.t.Helper()
	if l.copy {
		h.t.Fatalf("rutistest: row %s is a copy of a plugin definition, whose config is fixed when it is mounted (rutis never updates a copy): unload it and load the definition again", l.row)
	}
	if _, err := convert.ToPeer(config); err != nil {
		h.t.Fatalf("rutistest: config: %v", err)
	}
	h.mu.Lock()
	if _, loaded := h.scope[l.row]; !loaded || h.closed {
		h.mu.Unlock()
		h.t.Fatalf("rutistest: row %s is not loaded", l.row)
	}
	h.mu.Unlock()
	h.stop(l)
	h.start(l, config)
}

// close unloads every row, latest first, and ends the runtime.
func (h *harness) close() {
	h.mu.Lock()
	rows := append([]*Loaded(nil), h.rows...)
	h.mu.Unlock()
	for i := len(rows) - 1; i >= 0; i-- {
		rows[i].Unload()
	}
	h.mu.Lock()
	h.closed = true
	h.mu.Unlock()
	harnessesMu.Lock()
	delete(harnesses, h)
	harnessesMu.Unlock()
	h.session.Close(nil)
	h.conn.Close()
	select {
	case <-h.done:
	case <-time.After(10 * time.Second):
		h.t.Errorf("rutistest: the runtime did not end")
	}
}

// Service is a provided service, called across the session.
type Service struct {
	session *peer.Peer
	handle  string
	name    string
	kinds   map[string]string
	// Sees each result, settled (the plugin definitions in it).
	returned func(value any)
}

// invoke calls `method`, settling and seeing the result.
func (s *Service) invoke(ctx context.Context, method string, args []any, sync bool) (any, error) {
	value, err := s.session.Call(ctx, s.handle, method, args, sync)
	if err == nil {
		value, err = peer.Settle(ctx, value)
	}
	if err == nil && s.returned != nil {
		s.returned(value)
	}
	return value, err
}

// Methods maps the service's declared methods to "sync" or "async".
func (s *Service) Methods() map[string]string { return s.kinds }

// Call calls `method` and decodes its result into `result` (a pointer, or
// nil).
func (s *Service) Call(ctx context.Context, method string, result any, args ...any) error {
	kind, ok := s.kinds[method]
	if !ok {
		return fmt.Errorf("%s has no declared method %s", s.name, method)
	}
	converted, err := convert.ToPeerArgs(args)
	if err != nil {
		return err
	}
	value, err := s.invoke(ctx, method, converted, kind == "sync")
	if err != nil {
		return err
	}
	return convert.Decode(value, result)
}

// Bind fills `target`, a pointer to a struct of function fields, with
// functions calling the service's methods, as rutis.Ctx.Use does.
func (s *Service) Bind(target any) error {
	value := reflect.ValueOf(target)
	if value.Kind() != reflect.Pointer || value.Elem().Kind() != reflect.Struct {
		return fmt.Errorf("bind %s: the target must point to a struct of function fields", s.name)
	}
	value = value.Elem()
	for _, field := range reflect.VisibleFields(value.Type()) {
		if !field.IsExported() || field.Anonymous || field.Type.Kind() != reflect.Func {
			continue
		}
		wire := convert.WireName(field.Name)
		if tag, ok := field.Tag.Lookup("rutis"); ok && tag != "" {
			wire = tag
		}
		kind, ok := s.kinds[wire]
		if !ok {
			return fmt.Errorf("bind %s: field %s calls %s, which the service does not declare", s.name, field.Name, wire)
		}
		if err := convert.CheckSignature(field.Type, true); err != nil {
			return fmt.Errorf("bind %s: field %s: %w", s.name, field.Name, err)
		}
		method := wire
		value.FieldByIndex(field.Index).Set(convert.MakeFunc(field.Type, func(ctx context.Context, args []any) (any, error) {
			return s.invoke(ctx, method, args, kind == "sync")
		}))
	}
	return nil
}
