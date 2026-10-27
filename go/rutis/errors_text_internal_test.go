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
	for _, err := range []error{&RemoteError{Name: "E", Message: "m"}, &AggregateError{}, ErrBinding} {
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
	if reflect.TypeOf(ErrSyncWaitCycle) == nil || !errors.Is(&RemoteError{Name: "SyncWaitCycle"}, ErrSyncWaitCycle) {
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
