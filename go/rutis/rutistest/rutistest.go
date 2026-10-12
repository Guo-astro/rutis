// Package rutistest tests a plugin without a host: it gives the plugin the
// services it injects, calls the services it provides, and unloads it when
// the test ends.
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
// as they would between processes (data is copied, functions and Ref
// objects pass by reference), so what works only in one process fails here
// too. It checks what a host would: the plugin uses only the services it
// declares in Inject, provides what Provides declares with the declared
// types, and its cleanups succeed.
package rutistest

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"reflect"
	"sync"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

const row = "test"

// Loaded is a plugin loaded for a test.
type Loaded struct {
	t        testing.TB
	conn     net.Conn
	session  *peer.Peer
	provides map[string]map[string]string

	mu      sync.Mutex
	handles map[string]string
	changed chan struct{}
	done    chan error
	unload  sync.Once
}

// Load loads `plugin` with `config` (a value of its config type, or nil)
// and the services in `services` (Go values whose exported methods are the
// service's methods). The plugin is unloaded when the test ends.
func Load(t testing.TB, plugin *rutis.Definition, config any, services map[string]any) *Loaded {
	t.Helper()
	declared := describe(t, plugin)
	for _, name := range declared.Inject {
		if _, ok := services[name]; !ok {
			t.Fatalf("rutistest: the plugin injects %s: give the test a service %s", name, name)
		}
	}
	runtimeSide, hostSide := net.Pipe()
	loaded := &Loaded{
		t:        t,
		conn:     hostSide,
		provides: declared.Provides,
		handles:  map[string]string{},
		changed:  make(chan struct{}, 1),
		done:     make(chan error, 1),
	}
	go func() { loaded.done <- rutis.ServeConn(runtimeSide, plugin) }()
	session, err := peer.New(peer.NewLines(hostSide), peer.Options{
		Host: true,
		Dispatch: func(ctx context.Context, target, method string, args []any) (any, error) {
			return loaded.dispatch(ctx, services, target, method, args)
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	loaded.session = session
	if err := session.Start(); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "rutistest: the handshake", session.Ready())
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	if _, err := session.Call(ctx, "", "mount", []any{map[string]any{}}, false); err != nil {
		t.Fatalf("rutistest: mount: %v", err)
	}
	for _, name := range declared.Inject {
		kinds := map[string]string{}
		for wire := range convert.Methods(reflect.TypeOf(services[name])) {
			kinds[wire] = "sync"
		}
		if _, err := session.Call(ctx, "", "hosts.provide", []any{name, kinds}, false); err != nil {
			t.Fatalf("rutistest: provide %s: %v", name, err)
		}
	}
	configValue, err := convert.ToPeer(config)
	if err != nil {
		t.Fatalf("rutistest: config: %v", err)
	}
	if configValue == nil {
		configValue = json.RawMessage("null")
	}
	exports := map[string]any{}
	for name, kinds := range declared.Provides {
		exports[name] = kinds
	}
	if _, err := session.Call(ctx, "", "rows.load", []any{row, plugin.Name(), configValue, []any{}, []any{}, exports}, false); err != nil {
		t.Fatalf("rutistest: load %s: %v", plugin.Name(), err)
	}
	t.Cleanup(loaded.Unload)
	return loaded
}

type declaration struct {
	Inject   []string                     `json:"inject"`
	Provides map[string]map[string]string `json:"provides"`
}

func describe(t testing.TB, plugin *rutis.Definition) declaration {
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
	return manifest.Plugins[plugin.Name()]
}

func (l *Loaded) dispatch(ctx context.Context, services map[string]any, target, method string, args []any) (any, error) {
	if target == "" && method == "service" {
		var id, handle string
		_ = convert.Decode(index(args, 0), &id)
		_ = convert.Decode(index(args, 1), &handle)
		l.mu.Lock()
		if handle == "" {
			delete(l.handles, id)
		} else {
			l.handles[id] = handle
		}
		l.mu.Unlock()
		select {
		case l.changed <- struct{}{}:
		default:
		}
		return nil, nil
	}
	name, ok := cutPrefix(target, "host:")
	if !ok {
		return nil, fmt.Errorf("the test serves no %s", target)
	}
	service, ok := services[name]
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

func index(args []any, i int) any {
	if i < len(args) {
		return args[i]
	}
	return nil
}

func cutPrefix(s, prefix string) (string, bool) {
	if len(s) >= len(prefix) && s[:len(prefix)] == prefix {
		return s[len(prefix):], true
	}
	return "", false
}

// Provided lists the services the plugin provides now.
func (l *Loaded) Provided() []string {
	l.mu.Lock()
	defer l.mu.Unlock()
	names := make([]string, 0, len(l.handles))
	for name := range l.handles {
		names = append(names, name)
	}
	return names
}

// Service is the service `name` the plugin provides, as rutis sees it:
// only its declared methods. The test fails if the plugin does not
// declare it or does not provide it.
func (l *Loaded) Service(name string) *Service {
	l.t.Helper()
	kinds, declared := l.provides[name]
	if !declared {
		l.t.Fatalf("rutistest: %s is not declared in Provides, so rutis cannot use it", name)
	}
	// The notification may still be on its way when the load returned. A
	// notification signals l.changed after it changed l.handles, and the
	// signal is kept until taken: looking, then waiting, misses none.
	deadline := time.NewTimer(testwait.HangGuard)
	defer deadline.Stop()
	var handle string
	for {
		l.mu.Lock()
		found, ok := l.handles[name]
		l.mu.Unlock()
		if ok {
			handle = found
			break
		}
		select {
		case <-l.changed:
		case <-deadline.C:
			l.t.Fatalf("rutistest: the plugin does not provide %s (now)", name)
		}
	}
	return &Service{session: l.session, handle: handle, name: name, kinds: kinds}
}

// Unload unloads the plugin and fails the test if a cleanup failed. It runs
// when the test ends; calling it earlier is allowed.
func (l *Loaded) Unload() {
	l.unload.Do(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		if _, err := l.session.Call(ctx, "", "rows.unload", []any{row}, false); err != nil {
			l.t.Errorf("rutistest: unload: %v", err)
		}
		l.session.Close(nil)
		l.conn.Close()
		select {
		case <-l.done:
		case <-time.After(10 * time.Second):
			l.t.Errorf("rutistest: the runtime did not end")
		}
	})
}

// Service is a provided service, called across the session.
type Service struct {
	session *peer.Peer
	handle  string
	name    string
	kinds   map[string]string
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
	value, err := s.session.Call(ctx, s.handle, method, converted, kind == "sync")
	if err != nil {
		return err
	}
	if value, err = peer.Settle(ctx, value); err != nil {
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
			return s.session.Call(ctx, s.handle, method, args, kind == "sync")
		}))
	}
	return nil
}
