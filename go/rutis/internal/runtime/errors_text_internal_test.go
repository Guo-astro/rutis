package rutis

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"reflect"
	"strings"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

type textForecast struct {
	City string `json:"city"`
}

type textWeather struct{}

func (textWeather) Today(context.Context) (string, error)               { return "sun", nil }
func (textWeather) Tomorrow(_ context.Context, day int) (string, error) { return "rain", nil }
func (textWeather) Forecast(_ context.Context, f textForecast) error    { return nil }

// coded is the code and data of err, which must be named `name`.
func coded(t *testing.T, err error, name, code string) map[string]any {
	t.Helper()
	if err == nil {
		t.Fatal("no error")
	}
	if got := peer.ErrorName(err); got != name {
		t.Fatalf("%v: named %s, not %s", err, got, name)
	}
	c, ok := err.(interface {
		Code() string
		Data() any
	})
	if !ok || c.Code() != code {
		t.Fatalf("%v: not code %s (%#v)", err, code, err)
	}
	data, _ := c.Data().(map[string]any)
	return data
}

func sameData(t *testing.T, got, want map[string]any) {
	t.Helper()
	g, _ := json.Marshal(got)
	w, _ := json.Marshal(want)
	if string(g) != string(w) {
		t.Fatalf("data %s, want %s", g, w)
	}
}

// guarantee: sdk/error-texts (a call the Go runtime refuses says it with the
// service, method and 1-based argument the caller wrote, code no-method or
// bad-argument and its data: #677)
func TestARefusedCallNamesTheServiceMethodAndArgument(t *testing.T) {
	rt := newRuntime(nil)
	rt.handles["weather"] = &handleEntry{id: "weather", name: "weather", object: textWeather{}, current: true}
	ctx := context.Background()
	dispatch := func(method string, args ...string) error {
		raw := make([]any, len(args))
		for i, arg := range args {
			raw[i] = json.RawMessage(arg)
		}
		_, err := rt.dispatch(ctx, "weather", method, raw)
		return err
	}

	err := dispatch("tomorow")
	if err.Error() != "weather has no method tomorow (methods: forecast, today, tomorrow)" {
		t.Fatal(err)
	}
	sameData(t, coded(t, err, "BindingError", "no-method"), map[string]any{"service": "weather", "method": "tomorow", "methods": []string{"forecast", "today", "tomorrow"}})

	err = dispatch("tomorrow", "1", "2")
	if err.Error() != "weather.tomorrow takes 1 argument, got 2" {
		t.Fatal(err)
	}
	sameData(t, coded(t, err, "BindingError", "bad-argument"), map[string]any{"service": "weather", "method": "tomorrow", "expected": 1, "received": 2})
	if err := dispatch("today", "1", "2"); err.Error() != "weather.today takes 0 arguments, got 2" {
		t.Fatal(err)
	}

	err = dispatch("tomorrow", `"x"`)
	if err.Error() != `weather.tomorrow: argument 1 should be an integer, got "x"` {
		t.Fatal(err)
	}
	sameData(t, coded(t, err, "BindingError", "bad-argument"), map[string]any{"service": "weather", "method": "tomorrow", "argument": 1, "expected": "an integer"})
	if err := dispatch("tomorrow", "1.5"); err.Error() != "weather.tomorrow: argument 1 should be an integer, got 1.5" {
		t.Fatal(err)
	}
	if err := dispatch("tomorrow", "99999999999999999999"); err.Error() != "weather.tomorrow: argument 1 should be an integer, got 99999999999999999999 (out of range for int)" {
		t.Fatal(err)
	}
	if err := dispatch("forecast", `"oslo"`); err.Error() != `weather.forecast: argument 1 should be a textForecast, got "oslo"` {
		t.Fatal(err)
	}
	if err := dispatch("forecast", `{"city":3}`); err.Error() != "weather.forecast: argument 1 should be a textForecast, got an object whose city is a number" {
		t.Fatal(err)
	}
	if err := dispatch("forecast", `[1]`); err.Error() != "weather.forecast: argument 1 should be a textForecast, got a list" {
		t.Fatal(err)
	}
	// A missing argument is the parameter's zero value, as Go has no
	// optional parameters: no refusal.
	if err := dispatch("tomorrow"); err != nil {
		t.Fatal(err)
	}

	// A service declaring its methods lists those.
	rt.handles["weather"].methods = map[string]bool{"today": true}
	if err := dispatch("tomorrow", "1"); err.Error() != "weather has no method tomorrow (methods: today)" {
		t.Fatal(err)
	}
}

type textCache struct{}

func (textCache) Get(_ context.Context, key string) (int, error) { return 0, nil }

// guarantee: sdk/error-texts (an object passed by reference refuses a method
// it lacks naming its type: #677)
func TestAnObjectRefusesAMethodByItsTypeName(t *testing.T) {
	_, err := convert.Object(textCache{}).Call(context.Background(), "gett", nil)
	if err.Error() != "textCache has no method gett (methods: get)" {
		t.Fatal(err)
	}
	sameData(t, coded(t, err, "BindingError", "no-method"), map[string]any{"object": "textCache", "method": "gett", "methods": []string{"get"}})
}

// guarantee: sdk/undeclared-use (codes: a service not in Inject is
// not-declared, one nobody provides unavailable, each with {service}: #677)
func TestUseSaysWhyAServiceIsNotThere(t *testing.T) {
	rt := newRuntime(nil)
	r := &row{key: "chat", name: "chat", def: &Definition{inject: []string{"weather"}}}
	c := &Ctx{Context: context.Background(), rt: rt, row: r}
	var target struct{}

	err := c.Use("llm", &target)
	if err.Error() != `service llm is not available to this plugin, which does not declare it in Inject: add "llm" to its Inject` {
		t.Fatal(err)
	}
	sameData(t, coded(t, err, "BindingError", "not-declared"), map[string]any{"service": "llm"})

	err = c.Use("weather", &target)
	if err.Error() != "service weather is not available" {
		t.Fatal(err)
	}
	sameData(t, coded(t, err, "BindingError", "unavailable"), map[string]any{"service": "weather"})
}

// guarantee: sdk/error-texts (the Go SDK's own types print as package
// rutis's, never an internal package's: #677)
func TestTheTypesAPluginMeetsPrintAsPackageRutis(t *testing.T) {
	for _, err := range []error{&RemoteError{Name: "E", Message: "m"}, &peer.AggregateError{}, ErrBinding} {
		if got := fmt.Sprintf("%T", err); !strings.HasPrefix(got, "*rutis.") {
			t.Fatalf("%s", got)
		}
		if got := fmt.Sprintf("%#v", err); strings.Contains(got, "peer.") {
			t.Fatalf("%s", got)
		}
	}
	if got := fmt.Sprintf("%T", peer.HostGone(nil)); got != "*rutis.Coded" {
		t.Fatal(got)
	}
	// A context carrying a call chain prints no internal type.
	ctx := peer.WithChain(context.Background(), []string{"chat"})
	if got := fmt.Sprint(ctx); strings.Contains(got, "peer") || !strings.Contains(got, "call chain") {
		t.Fatal(got)
	}
	if reflect.TypeOf(peer.ErrSyncWaitCycle) == nil || !errors.Is(&RemoteError{Name: "SyncWaitCycle"}, peer.ErrSyncWaitCycle) {
		t.Fatal("SyncWaitCycle")
	}
}

// guarantee: sdk/error-texts (a service two rows provide names the row that
// has it, by its name, never the host's key for it: #677)
func TestADuplicateProviderIsNamedByItsRow(t *testing.T) {
	rt := newRuntime(nil)
	a := &row{key: "row:1.0", name: "a", def: &Definition{}}
	b := &row{key: "row:2.0", name: "b", def: &Definition{}}
	rt.rows[a.key], rt.rows[b.key] = a, b
	rt.provide(a, "forecast", textWeather{})
	defer func() {
		if got := fmt.Sprint(recover()); got != "forecast is already provided by a" {
			t.Fatal(got)
		}
	}()
	rt.provide(b, "forecast", textWeather{})
}

// guarantee: sdk/optional-deps (Get reads a service in Optional: bound and
// true when there, false when not; Use counts Optional as declared, so an
// absent one is unavailable; Get of a name declared nowhere is
// not-declared, saying to add it to Optional)
func TestGetReadsAnOptionalService(t *testing.T) {
	rt := newRuntime(nil)
	rt.hosts["llm"] = map[string]string{"ask": "async"}
	r := &row{key: "chat", name: "chat", def: &Definition{inject: []string{"notes"}, optional: []string{"llm", "search"}}}
	c := &Ctx{Context: context.Background(), rt: rt, row: r}
	var llm struct {
		Ask func(ctx context.Context, question string) (string, error)
	}

	if ok, err := c.Get("llm", &llm); !ok || err != nil || llm.Ask == nil {
		t.Fatalf("present: %v %v", ok, err)
	}
	// Use reads only what Inject lists, as Rust's `require`: an Optional
	// one is told to go there, present or not.
	notInject := `service llm is not available to this plugin, which does not declare it in Inject: add "llm" to its Inject`
	if err := c.Use("llm", &llm); err == nil || err.Error() != notInject {
		t.Fatalf("use present: %v", err)
	}
	var search struct{}
	if ok, err := c.Get("search", &search); ok || err != nil {
		t.Fatalf("absent: %v %v", ok, err)
	}
	// A required one is declared too: Get reads it as Use would.
	if ok, err := c.Get("notes", &search); ok || err != nil {
		t.Fatalf("absent required: %v %v", ok, err)
	}

	err := c.Use("search", &search)
	if err.Error() != `service search is not available to this plugin, which does not declare it in Inject: add "search" to its Inject` {
		t.Fatal(err)
	}
	sameData(t, coded(t, err, "BindingError", "not-declared"), map[string]any{"service": "search"})

	ok, err := c.Get("other", &search)
	if ok || err.Error() != `service other is not available to this plugin, which does not declare it: add "other" to its Optional (or to its Inject, if it cannot run without it)` {
		t.Fatal(ok, err)
	}
	sameData(t, coded(t, err, "BindingError", "not-declared"), map[string]any{"service": "other"})
	// Use keeps its own line for a name declared nowhere.
	if err := c.Use("other", &search); err.Error() != `service other is not available to this plugin, which does not declare it in Inject: add "other" to its Inject` {
		t.Fatal(err)
	}
}

// guarantee: sdk/optional-deps (a service whose row unloaded, called or
// read through a handle a caller still holds, is unavailable; one its row
// replaced while running keeps its object until then, as a snapshot of a
// service does in Rust)
func TestAServiceWhoseRowUnloadedIsNotAvailable(t *testing.T) {
	rt := newRuntime(nil)
	rt.handles["weather"] = &handleEntry{id: "weather", name: "weather", object: textWeather{}, current: true}
	ctx := context.Background()
	if _, err := rt.dispatch(ctx, "weather", "today", nil); err != nil {
		t.Fatal(err)
	}
	// Replaced: the handle still reaches its object.
	rt.retire("weather")
	if _, err := rt.dispatch(ctx, "weather", "today", nil); err != nil {
		t.Fatal(err)
	}
	rt.gone("weather")
	for _, call := range []func() error{
		func() error { _, err := rt.dispatch(ctx, "weather", "today", nil); return err },
		func() error {
			_, err := rt.dispatch(ctx, "", "get", []any{json.RawMessage(`"weather"`), json.RawMessage(`"today"`)})
			return err
		},
	} {
		err := call()
		if err == nil || err.Error() != "service weather is not available" {
			t.Fatal(err)
		}
		sameData(t, coded(t, err, "BindingError", "unavailable"), map[string]any{"service": "weather"})
	}
}

// guarantee: sdk/optional-deps (a row loaded again gives its service a
// handle of its own: one a caller still holds for the object before stays
// retired, rather than reaching the new one)
func TestAServiceOfARowLoadedAgainHasANewHandle(t *testing.T) {
	weather := Define(Plugin[NoConfig]{Name: "weather", Apply: func(c *Ctx, _ NoConfig) error {
		c.Provide("weather", textWeather{})
		return nil
	}})
	h := startFakeHost(t, nil, weather)
	handle := func(what string, ok func(string) bool) string {
		var got string
		testwait.Eventually(t, what, func() bool {
			h.mu.Lock()
			defer h.mu.Unlock()
			got = h.handles["weather"]
			return ok(got)
		}, nil)
		return got
	}
	if err := h.load("w", "weather"); err != nil {
		t.Fatal(err)
	}
	old := handle("the weather", func(got string) bool { return got != "" })
	if err := h.call("rows.unload", "w"); err != nil {
		t.Fatal(err)
	}
	if err := h.load("w", "weather"); err != nil {
		t.Fatal(err)
	}
	now := handle("the new weather", func(got string) bool { return got != "" && got != old })
	ctx, cancel := context.WithTimeout(context.Background(), testwait.HangGuard)
	defer cancel()
	if _, err := h.host.Call(ctx, now, "today", []any{}, false); err != nil {
		t.Fatal(err)
	}
	if _, err := h.host.Call(ctx, old, "today", []any{}, false); err == nil || !strings.Contains(err.Error(), "service weather is not available") {
		t.Fatal(err)
	}
}

// guarantee: sdk/optional-deps (a row's own `inject` counts as declared:
// the plugin uses it without listing it, and it is unavailable rather than
// not-declared when nothing provides it)
func TestARowsInjectCountsAsDeclared(t *testing.T) {
	rt := newRuntime(nil)
	rt.hosts["llm"] = map[string]string{"ask": "async"}
	r := &row{key: "chat", name: "chat", def: &Definition{}, inject: []string{"llm", "search"}}
	c := &Ctx{Context: context.Background(), rt: rt, row: r}
	var llm struct {
		Ask func(ctx context.Context, question string) (string, error)
	}
	if err := c.Use("llm", &llm); err != nil || llm.Ask == nil {
		t.Fatalf("use from the row's inject: %v", err)
	}
	if ok, err := c.Get("llm", &llm); !ok || err != nil {
		t.Fatalf("get from the row's inject: %v %v", ok, err)
	}
	var search struct{}
	err := c.Use("search", &search)
	if err == nil || err.Error() != "service search is not available" {
		t.Fatal(err)
	}
	sameData(t, coded(t, err, "BindingError", "unavailable"), map[string]any{"service": "search"})
	// Nothing declares `other`: use says so, whether or not this process
	// has one.
	if err := c.Use("other", &search); err == nil || err.Error() != `service other is not available to this plugin, which does not declare it in Inject: add "other" to its Inject` {
		t.Fatal(err)
	}
}

// guarantee: sdk/optional-deps (the row's `inject` is rows.load's fifth
// argument, as rutis sends it, and a restarted row keeps it)
func TestARowsInjectArrivesWithRowsLoad(t *testing.T) {
	type seen struct {
		use, get, absent, undeclared error
		there                        bool
	}
	applied := make(chan seen, 2)
	reader := Define(Plugin[NoConfig]{Name: "reader", Apply: func(c *Ctx, _ NoConfig) error {
		var llm struct {
			Ask func(ctx context.Context) (string, error)
		}
		var none struct{}
		s := seen{use: c.Use("llm", &llm), absent: c.Use("search", &none), undeclared: c.Use("other", &none)}
		s.there, s.get = c.Get("llm", &llm)
		applied <- s
		return nil
	}})
	h := startFakeHost(t, nil, reader)
	if err := h.call("hosts.provide", "llm", map[string]string{"ask": "async"}, nil); err != nil {
		t.Fatal(err)
	}
	check := func(when string) {
		t.Helper()
		got := testwait.Within(t, "the row applies "+when, applied)
		if got.use != nil {
			t.Fatalf("%s: use of a name in the row's inject: %v", when, got.use)
		}
		if !got.there || got.get != nil {
			t.Fatalf("%s: get of a name in the row's inject: %v %v", when, got.there, got.get)
		}
		if got.absent == nil || got.absent.Error() != "service search is not available" {
			t.Fatalf("%s: %v", when, got.absent)
		}
		if got.undeclared == nil || got.undeclared.Error() != `service other is not available to this plugin, which does not declare it in Inject: add "other" to its Inject` {
			t.Fatalf("%s: %v", when, got.undeclared)
		}
	}
	if err := h.call("rows.load", "reader", "reader", json.RawMessage("null"), []any{}, []any{"llm", "search"}, map[string]any{}, map[string]any{"scope": []any{}, "id": "reader"}); err != nil {
		t.Fatal(err)
	}
	check("loaded")
	if err := h.call("rows.update", "reader", json.RawMessage("null")); err != nil {
		t.Fatal(err)
	}
	check("updated")
}

// guarantee: sdk/optional-deps (a name the plugin provides itself counts as
// declared while it provides it; withdrawn, it is like an undeclared one,
// as for a Rust plugin's strict read)
func TestAPluginReadsWhatItProvidesWhileItProvidesIt(t *testing.T) {
	type seen struct {
		use, get, useWithdrawn, getWithdrawn error
		there                                bool
	}
	applied := make(chan seen, 1)
	local := Define(Plugin[NoConfig]{Name: "local", Apply: func(c *Ctx, _ NoConfig) error {
		var weather struct {
			Today func(ctx context.Context) (string, error)
		}
		withdraw := c.Provide("local", textWeather{})
		s := seen{use: c.Use("local", &weather)}
		s.there, s.get = c.Get("local", &weather)
		withdraw()
		s.useWithdrawn = c.Use("local", &weather)
		_, s.getWithdrawn = c.Get("local", &weather)
		applied <- s
		return nil
	}})
	h := startFakeHost(t, nil, local)
	if err := h.load("local", "local"); err != nil {
		t.Fatal(err)
	}
	got := testwait.Within(t, "the row applies", applied)
	if got.use != nil || !got.there || got.get != nil {
		t.Fatalf("while provided: use %v, get %v %v", got.use, got.there, got.get)
	}
	if got.useWithdrawn == nil || got.useWithdrawn.Error() != `service local is not available to this plugin, which does not declare it in Inject: add "local" to its Inject` {
		t.Fatal(got.useWithdrawn)
	}
	if got.getWithdrawn == nil || got.getWithdrawn.Error() != `service local is not available to this plugin, which does not declare it: add "local" to its Optional (or to its Inject, if it cannot run without it)` {
		t.Fatal(got.getWithdrawn)
	}
}
