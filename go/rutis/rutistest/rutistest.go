// Package rutistest tests a plugin in its real runtime under the real rutis
// kernel: it gives the plugin the services it injects, calls the services
// it provides, emits and listens to events, and unloads it when the test
// ends.
//
//	func TestWeather(t *testing.T) {
//		loaded := rutistest.Load(t, weather.Plugin, weather.Config{City: "Oslo"}, map[string]any{"llm": &FakeLLM{}})
//		var today string
//		if err := loaded.Service("weather").Call(context.Background(), "today", &today); err != nil {
//			t.Fatal(err)
//		}
//	}
//
// The test process runs the Go runtime, and starts `rutis-host test`
// (`rutis-host` on PATH, or the path RUTIS_HOST gives) once, on the first
// Load: a host with no rows of its own, in a session with that runtime.
// Each Load is a row of the host's loader, as a row of rutis.json is: the
// host checks its config against the plugin's schema and fills its
// defaults in, waits for the services it injects, leases them, dispatches
// its events, and unloads it. The services a test gives are a plugin of the
// test's in the same runtime that provides them; the test's listeners and
// emissions are another, which uses only what every plugin uses (Ctx.On,
// Ctx.Emit and the rest). Rows are unloaded when their test ends; the host
// stays for the next test, and ends with the test process.
//
// Values cross as they would between processes (data is copied, time.Time,
// []byte and the other tagged values cross as their values, functions and
// Ref objects pass by reference): every call goes through the host.
package rutistest

import (
	"context"
	"errors"
	"fmt"
	"reflect"
	"slices"
	"strconv"
	"sync"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/advanced"
	"github.com/arcships/rutis/go/rutis/internal/events"
	runtime "github.com/arcships/rutis/go/rutis/internal/runtime"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// Option adjusts Load and Loaded.Load.
type Option func(*options)

type options struct {
	row string
}

// Row gives the row the id `key` (by default the plugin's name, then
// `<name>-2`, `<name>-3`, ... while one is taken).
func Row(key string) Option { return func(o *options) { o.row = key } }

// Loaded is a row of a plugin loaded for a test.
type Loaded struct {
	h *host
	t testing.TB
	// Its plugin, the name the runtime finds it by, and the row's id.
	plugin *rutis.Definition
	name   string
	id     string
	// The rows of the test's plugins providing the services the test gave
	// it.
	given []*provider
	// How many lines the runtime had reported when the row loaded.
	since  int
	unload sync.Once
	mu     sync.Mutex
	used   map[string]*Service
}

// Load loads `plugin` with `config` (a value of its config type, or nil)
// and the services in `services` (Go values whose exported methods are the
// service's methods, all asynchronous). The plugin is unloaded when the
// test ends.
//
// `plugin` may also be a plugin definition a loaded plugin handed to a
// test service (a *rutis.Definition parameter of its method): it is then
// loaded as another row of the runtime, with what its `apply` prepared
// there.
func Load(t testing.TB, plugin *rutis.Definition, config any, services map[string]any, opts ...Option) *Loaded {
	t.Helper()
	loaded, err := load(t, plugin, config, services, opts)
	if err != nil {
		t.Fatalf("rutistest: %v", err)
	}
	return loaded
}

// TryLoad is Load giving back why the plugin did not load instead of
// failing the test: the host's line for the row, such as `weather: config:
// city is required` for a config that does not match the plugin's schema.
// Other mistakes of the test (rutis-host not found, say) still fail it.
func TryLoad(t testing.TB, plugin *rutis.Definition, config any, services map[string]any, opts ...Option) (loaded *Loaded, err error) {
	t.Helper()
	return load(t, plugin, config, services, opts)
}

func load(t testing.TB, plugin *rutis.Definition, config any, services map[string]any, opts []Option) (*Loaded, error) {
	t.Helper()
	if plugin == nil {
		t.Fatal("rutistest: a nil plugin definition")
	}
	h := shared(t)
	l := &Loaded{h: h, t: t, plugin: plugin, name: h.register(plugin)}
	given, err := h.give(services)
	if err != nil {
		t.Fatalf("rutistest: %v", err)
	}
	l.given = given
	if err := l.start(config, opts); err != nil {
		h.release(l.given)
		return nil, err
	}
	return l, nil
}

// start loads the row of l.name with `config`.
func (l *Loaded) start(config any, opts []Option) error {
	var o options
	for _, option := range opts {
		option(&o)
	}
	l.since = l.h.lines()
	if o.row == "" {
		o.row = l.h.id(l.plugin)
	}
	fields := map[string]any{"id": o.row}
	id, err := l.h.test.LoadWith(context.Background(), l.name, config, fields)
	if err != nil {
		return errors.New(message(err))
	}
	l.id = id
	l.h.taken(id, true)
	l.t.Cleanup(l.Unload)
	return nil
}

// message is the host's text of a failed call, without the error's name.
func message(err error) string {
	var remote *rutis.RemoteError
	if errors.As(err, &remote) {
		return remote.Message
	}
	return err.Error()
}

// Load loads another row of the same plugin into the same runtime, with
// `config`. It unloads when the test ends.
func (l *Loaded) Load(config any, options ...Option) *Loaded {
	l.t.Helper()
	row := &Loaded{h: l.h, t: l.t, plugin: l.plugin, name: l.name, given: l.h.share(l.given)}
	if err := row.start(config, options); err != nil {
		l.h.release(row.given)
		l.t.Fatalf("rutistest: %v", err)
	}
	return row
}

// Update changes the row's config as rutis does for a Go row: it restarts
// it. A config the plugin's schema refuses fails the test, and the row
// keeps its config.
func (l *Loaded) Update(config any) {
	l.t.Helper()
	if err := l.h.test.Update(context.Background(), l.id, config); err != nil {
		l.t.Fatalf("rutistest: update %s", message(err))
	}
}

// Unload unloads the row, failing the test if a cleanup failed. The
// runtime cancels its running listeners and waits for them first, up to
// RUTIS_SHUTDOWN_TIMEOUT seconds (default 10); past it, it reports so
// (Reported) and unloads anyway. It runs when the test ends; calling it
// earlier is allowed.
func (l *Loaded) Unload() {
	l.unload.Do(func() {
		l.mu.Lock()
		used := l.used
		l.used = nil
		l.mu.Unlock()
		for _, service := range used {
			_ = l.h.test.Unload(context.Background(), service.row)
		}
		ctx, cancel := context.WithTimeout(context.Background(), events.ShutdownTimeout()+30*time.Second)
		defer cancel()
		err := l.h.test.Unload(ctx, l.id)
		l.h.taken(l.id, false)
		if err != nil {
			l.t.Errorf("rutistest: unload %s", message(err))
		}
		// The services after the rows using them: its cleanups use them.
		l.h.release(l.given)
	})
}

// Reported is what was reported since the row loaded: one line per
// failure of a listener an Emit reached, of a filter, of an emission, as
// the runtime and the host write them.
func (l *Loaded) Reported() []string {
	lines := l.h.since(l.since)
	from, err := l.h.test.Reported(context.Background(), l.id)
	if err != nil {
		l.t.Errorf("rutistest: reported %s", message(err))
	}
	return append(lines, from...)
}

// Provided lists the services the row provides now.
func (l *Loaded) Provided() []string { return l.h.runtime.Provided(l.id) }

// ── Events ───────────────────────────────────────────────────────

// On listens to the event `name` on the bus, until the returned disposer
// is called or the test ends: the plugin's emissions of `name` reach `fn`,
// and so do the test's own. In a waterfall `fn` gets an advanced.Next last.
// The listener is the test's plugin's (row `rutistest`).
func (l *Loaded) On(name string, fn func(ctx context.Context, args ...any) (any, error), options ...rutis.ListenOption) func() {
	l.t.Helper()
	if fn == nil {
		l.t.Fatal("rutistest: On needs a listener")
	}
	off := l.h.driver.On(name, fn, options...)
	var once sync.Once
	dispose := func() { once.Do(off) }
	l.t.Cleanup(dispose)
	return dispose
}

// Emit starts every listener of `name` on the bus and returns; a
// listener's failure is reported (Reported), not returned.
func (l *Loaded) Emit(ctx context.Context, name string, args ...any) error {
	return l.h.driver.Emit(ctx, name, args...)
}

// Parallel runs every listener of `name` and waits for all; failures are
// a *advanced.AggregateError in registration order.
func (l *Loaded) Parallel(ctx context.Context, name string, args ...any) error {
	return l.h.driver.Parallel(ctx, name, args...)
}

// Serial runs the listeners of `name` one at a time and returns the first
// result.
func (l *Loaded) Serial(ctx context.Context, name string, args ...any) (any, error) {
	return l.h.driver.Serial(ctx, name, args...)
}

// Waterfall runs the listeners of `name` outermost first, each with
// `next`, the innermost `terminal`, called with `args` (nil: no result).
func (l *Loaded) Waterfall(ctx context.Context, name string, terminal rutis.Terminal, args ...any) (any, error) {
	return l.h.driver.Waterfall(ctx, name, terminal, args...)
}

// Bail is Serial as a synchronous call.
func (l *Loaded) Bail(ctx context.Context, name string, args ...any) (any, error) {
	return l.h.driver.Bail(ctx, name, args...)
}

// Filter dispatches only to the listeners `keep` accepts. An error `keep`
// returns (or a panic) fails the dispatch before any listener runs.
func (l *Loaded) Filter(keep func(rutis.Listener) (bool, error)) *Emitter {
	return &Emitter{l.h.driver.Filter(keep)}
}

// Emitter is Loaded's emissions, filtered.
type Emitter struct{ e *rutis.Emitter }

// Emit is Loaded.Emit, filtered.
func (e *Emitter) Emit(ctx context.Context, name string, args ...any) error {
	return e.e.Emit(ctx, name, args...)
}

// Parallel is Loaded.Parallel, filtered.
func (e *Emitter) Parallel(ctx context.Context, name string, args ...any) error {
	return e.e.Parallel(ctx, name, args...)
}

// Serial is Loaded.Serial, filtered.
func (e *Emitter) Serial(ctx context.Context, name string, args ...any) (any, error) {
	return e.e.Serial(ctx, name, args...)
}

// Waterfall is Loaded.Waterfall, filtered.
func (e *Emitter) Waterfall(ctx context.Context, name string, terminal rutis.Terminal, args ...any) (any, error) {
	return e.e.Waterfall(ctx, name, terminal, args...)
}

// Bail is Loaded.Bail, filtered.
func (e *Emitter) Bail(ctx context.Context, name string, args ...any) (any, error) {
	return e.e.Bail(ctx, name, args...)
}

// ── Services ─────────────────────────────────────────────────────

// Service is the service `name` as rutis gives it to a plugin injecting
// it: its declared methods, or, undeclared, the value's exported methods,
// all asynchronous. The test fails if nothing provides it.
func (l *Loaded) Service(name string) *Service {
	l.t.Helper()
	l.mu.Lock()
	service := l.used[name]
	l.mu.Unlock()
	if service != nil {
		return service
	}
	service = &Service{name: name, used: make(chan struct{})}
	user := l.h.register(service.plugin())
	ctx, cancel := context.WithTimeout(context.Background(), testwait.HangGuard)
	defer cancel()
	row, err := l.h.test.Load(ctx, user, nil)
	if err != nil {
		l.t.Fatalf("rutistest: the plugin does not provide %s (now): %s", name, message(err))
	}
	service.row = row
	testwait.Within(l.t, "the service "+name, service.used)
	if service.err != nil {
		l.t.Fatalf("rutistest: use %s: %v", name, service.err)
	}
	l.mu.Lock()
	if l.used == nil {
		l.used = map[string]*Service{}
	}
	l.used[name] = service
	l.mu.Unlock()
	return service
}

// Service is a provided service, as a plugin injecting it uses it.
type Service struct {
	name string
	// The row of the test's plugin using it.
	row  string
	ctx  *rutis.Ctx
	s    *advanced.Service
	err  error
	used chan struct{}
}

// plugin is the test's plugin using the service.
func (s *Service) plugin() *rutis.Definition {
	var once sync.Once
	return rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Inject: []string{s.name},
		Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
			once.Do(func() {
				s.ctx = ctx
				s.err = ctx.Use(s.name, &s.s)
				close(s.used)
			})
			return nil
		},
	})
}

// Methods maps the service's methods to "sync" or "async".
func (s *Service) Methods() map[string]string { return s.s.Methods() }

// Call calls `method` and decodes its result into `result` (a pointer, or
// nil).
func (s *Service) Call(ctx context.Context, method string, result any, args ...any) error {
	return s.s.Call(ctx, method, result, args...)
}

// Bind fills `target`, a pointer to a struct of function fields, with
// functions calling the service's methods, as rutis.Ctx.Use does.
func (s *Service) Bind(target any) error { return s.ctx.Use(s.name, target) }

// provider is a row of the test's plugin providing services the test
// gave, and how many rows use them.
type provider struct {
	id    string
	given map[string]any
	users int
}

// give provides the services in `services` for a row: those the rows
// loaded already use (the same value) are theirs; the others are a new
// row of the test's. A name a loaded row is given another value of fails:
// a host has one provider of a name.
func (h *host) give(services map[string]any) ([]*provider, error) {
	h.mu.Lock()
	var used []*provider
	fresh := map[string]any{}
	for name, service := range services {
		existing := h.given[name]
		switch {
		case existing == nil:
			fresh[name] = service
		case !same(existing.given[name], service):
			h.mu.Unlock()
			return nil, fmt.Errorf("the service %s is given another value to a row still loaded: a host has one provider of a name; give the same value, or unload that row first", name)
		case !slices.Contains(used, existing):
			used = append(used, existing)
		}
	}
	for _, p := range used {
		p.users++
	}
	h.mu.Unlock()
	if len(fresh) == 0 {
		return used, nil
	}
	shapes := rutis.Provides{}
	for name, service := range fresh {
		shapes[name] = runtime.ShapeOf(reflect.TypeOf(service))
	}
	plugin := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Provides: shapes,
		Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
			for name, service := range fresh {
				ctx.Provide(name, service)
			}
			return nil
		},
	})
	id, err := h.test.Load(context.Background(), h.register(plugin), nil)
	if err != nil {
		h.release(used)
		return nil, fmt.Errorf("the services the test gives: %s", message(err))
	}
	p := &provider{id: id, given: fresh, users: 1}
	h.mu.Lock()
	for name := range fresh {
		h.given[name] = p
	}
	h.mu.Unlock()
	return append(used, p), nil
}

// share counts another row using `given`.
func (h *host) share(given []*provider) []*provider {
	h.mu.Lock()
	defer h.mu.Unlock()
	for _, p := range given {
		p.users++
	}
	return append([]*provider(nil), given...)
}

// release counts a row no longer using `given`: a row of the test's no
// row uses any more unloads.
func (h *host) release(given []*provider) {
	for _, p := range given {
		h.mu.Lock()
		p.users--
		last := p.users == 0
		if last {
			for name := range p.given {
				if h.given[name] == p {
					delete(h.given, name)
				}
			}
		}
		h.mu.Unlock()
		if last {
			_ = h.test.Unload(context.Background(), p.id)
		}
	}
}

// same reports whether two services the test gives are the same value.
func same(a, b any) bool {
	ta, tb := reflect.TypeOf(a), reflect.TypeOf(b)
	if ta != tb || ta == nil || !ta.Comparable() {
		return false
	}
	return a == b
}

// ── The host ─────────────────────────────────────────────────────

// driverName is the test's plugin the host loads first: it injects
// rutis-host/test, and its listeners and emissions are the test's.
const driverName = "rutistest"

// harness is the service rutis-host/test.
type harness struct {
	Load func(ctx context.Context, name string, config any) (string, error)
	// load with options: `id`.
	LoadWith func(ctx context.Context, name string, config any, options map[string]any) (string, error) `rutis:"load"`
	Update   func(ctx context.Context, id string, config any) error
	Unload   func(ctx context.Context, id string) error
	Reported func(ctx context.Context, id string) ([]string, error)
}

// host is the runtime of the test process and the rutis-host test it is
// in a session with: one per test process.
type host struct {
	runtime *runtime.Tester
	test    harness
	driver  *rutis.Ctx

	mu sync.Mutex
	// The plugins rows load, by name, and the name of each.
	plugins map[string]*rutis.Definition
	named   map[*rutis.Definition]string
	next    int
	// The ids of the rows loaded.
	ids map[string]bool
	// The row of the test's providing each service the test gave.
	given map[string]*provider
	// What the runtime reported, in order.
	reported []string
}

var (
	once    sync.Once
	started *host
	failed  error
)

// shared is the host of the test process, started on first use.
func shared(t testing.TB) *host {
	t.Helper()
	once.Do(func() { started, failed = start() })
	if failed != nil {
		t.Fatalf("rutistest: %v", failed)
	}
	return started
}

func start() (*host, error) {
	h := &host{plugins: map[string]*rutis.Definition{}, named: map[*rutis.Definition]string{}, ids: map[string]bool{driverName: true}, given: map[string]*provider{}}
	previous := events.SetReport(nil)
	events.SetReport(func(line string) {
		h.mu.Lock()
		h.reported = append(h.reported, line)
		h.mu.Unlock()
		previous(line)
	})
	ready := make(chan error, 1)
	h.plugins[driverName] = rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name:   driverName,
		Inject: []string{"rutis-host/test"},
		Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
			err := ctx.Use("rutis-host/test", &h.test)
			h.driver = ctx
			ready <- err
			return err
		},
	})
	h.runtime = runtime.NewTester(h.find)
	conn, ended, err := channel(driverName)
	if err != nil {
		return nil, err
	}
	served := make(chan error, 1)
	go func() { served <- h.runtime.Serve(conn, "test") }()
	timer := time.NewTimer(30 * time.Second)
	defer timer.Stop()
	select {
	case err := <-ready:
		if err != nil {
			return nil, fmt.Errorf("rutis-host/test: %w", err)
		}
		return h, nil
	case err := <-served:
		return nil, fmt.Errorf("the session with rutis-host ended before it loaded the test: %v", err)
	case err := <-ended:
		return nil, fmt.Errorf("rutis-host test ended: %v", err)
	case <-timer.C:
		return nil, errors.New("timed out (30s): rutis-host test did not load the test")
	}
}

// find is the plugin rows find by `name`.
func (h *host) find(name string) *rutis.Definition {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.plugins[name]
}

// register is the name rows load `plugin` by: its own, unless another
// plugin had it (then `<name>-<n>`: the host knows a plugin by its name
// for good), or it has none (then `rutistest-<n>`).
func (h *host) register(plugin *rutis.Definition) string {
	h.mu.Lock()
	defer h.mu.Unlock()
	if name, ok := h.named[plugin]; ok {
		return name
	}
	name := plugin.Name()
	if _, taken := h.plugins[name]; taken || name == "" {
		base := name
		if base == "" {
			base = driverName
		}
		for {
			h.next++
			name = base + "-" + strconv.Itoa(h.next)
			if _, taken := h.plugins[name]; !taken {
				break
			}
		}
	}
	h.plugins[name] = plugin
	h.named[plugin] = name
	return name
}

// id is the id of a new row of `plugin`: its name, then `<name>-2`,
// `<name>-3`... while one is taken (`plugin` for a plugin without one).
func (h *host) id(plugin *rutis.Definition) string {
	base := plugin.Name()
	if base == "" {
		base = "plugin"
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	id := base
	for n := 2; h.ids[id]; n++ {
		id = base + "-" + strconv.Itoa(n)
	}
	return id
}

// taken marks the row id `id` taken, or (false) free.
func (h *host) taken(id string, taken bool) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if taken {
		h.ids[id] = true
	} else {
		delete(h.ids, id)
	}
}

func (h *host) lines() int {
	h.mu.Lock()
	defer h.mu.Unlock()
	return len(h.reported)
}

func (h *host) since(from int) []string {
	h.mu.Lock()
	defer h.mu.Unlock()
	return append([]string(nil), h.reported[from:]...)
}
