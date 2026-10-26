package rutistest

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/fixtures/plugins/weather"
	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/values"
)

// The expectations the real runtimes are checked against
// (rutis_bridge::runtime::testing::runtime): rutistest is checked against
// the same list (#206, design V12).
const expectations = "../../../crates/rutis-bridge/src/runtime/testing/weather.expected.json"

type clock struct{ n atomic.Int64 }

func (c *clock) Now() int64 { return c.n.Add(1) - 1 }

type named struct{ name, message string }

func (e *named) Error() string { return e.message }
func (e *named) Name() string  { return e.name }

// argument is an argument as the expectations write it: data, or a
// callback.
func argument(t *testing.T, raw json.RawMessage) any {
	var spec struct {
		Callback json.RawMessage `json:"$callback"`
	}
	if json.Unmarshal(raw, &spec) != nil || spec.Callback == nil {
		return raw
	}
	if string(spec.Callback) == `"upper"` {
		return func(_ context.Context, day string) (string, error) { return strings.ToUpper(day), nil }
	}
	var thrown struct {
		Throws struct{ Name, Message string }
	}
	if err := json.Unmarshal(spec.Callback, &thrown); err != nil {
		t.Fatalf("unknown callback %s", spec.Callback)
	}
	return func(context.Context, string) (string, error) {
		return "", &named{thrown.Throws.Name, thrown.Throws.Message}
	}
}

// written is a peer value as the expectations write it: data as JSON, a
// tagged value as {"$<tag>": <its wire text>}.
func written(t *testing.T, value any) any {
	tagged := func(tag string, value any) any { return map[string]any{"$" + tag: value} }
	switch v := value.(type) {
	case nil, peer.UndefinedType:
		return nil
	case json.RawMessage:
		var out any
		if err := json.Unmarshal(v, &out); err != nil {
			t.Fatal(err)
		}
		return out
	case []any:
		out := make([]any, len(v))
		for i, item := range v {
			out[i] = written(t, item)
		}
		return out
	case map[string]any:
		out := map[string]any{}
		for key, item := range v {
			out[key] = written(t, item)
		}
		return out
	case peer.Map:
		out := make([]any, len(v))
		for i, entry := range v {
			out[i] = []any{written(t, entry.Key), written(t, entry.Value)}
		}
		return tagged("map", out)
	case peer.Set:
		return tagged("set", written(t, []any(v)))
	case time.Time:
		text, err := values.FormatInstant(v)
		if err != nil {
			t.Fatal(err)
		}
		return tagged("instant", text)
	case values.DateTime:
		return tagged("datetime", v.String())
	case values.Date:
		return tagged("date", v.String())
	case values.TimeOfDay:
		return tagged("time", v.String())
	case []byte:
		return tagged("bytes", values.FormatBytes(v))
	case *big.Int:
		return tagged("bigint", v.String())
	case values.Decimal:
		return tagged("decimal", string(v))
	case values.UUID:
		return tagged("uuid", string(v))
	}
	t.Fatalf("not data: %#v", value)
	return nil
}

type entry struct {
	Call    string            `json:"call"`
	Sample  string            `json:"sample"`
	Args    []json.RawMessage `json:"args"`
	Returns json.RawMessage   `json:"returns"`
	Throws  *struct {
		Name    *string `json:"name"`
		Message *string `json:"message"`
	} `json:"throws"`
	Ignore  []string `json:"ignore"`
	Allowed string   `json:"allowed"`
	Why     string   `json:"why"`
}

// check is that `value, err` is what `e` expects.
func check(t *testing.T, what string, e entry, value any, err error) {
	t.Helper()
	switch {
	case e.Returns != nil:
		if err != nil {
			t.Errorf("%s (%s): expected %s, got %v", what, e.Why, e.Returns, err)
			return
		}
		got := written(t, value)
		if object, ok := got.(map[string]any); ok {
			for _, key := range e.Ignore {
				delete(object, key)
			}
		}
		var want any
		_ = json.Unmarshal(e.Returns, &want)
		if !reflect.DeepEqual(got, want) {
			data, _ := json.Marshal(got)
			t.Errorf("%s (%s): got %s, want %s", what, e.Why, data, e.Returns)
		}
	case e.Throws != nil:
		if err == nil {
			t.Errorf("%s (%s): expected an error, got %v", what, e.Why, value)
			return
		}
		var remote *peer.RemoteError
		if e.Throws.Name == nil && e.Throws.Message == nil {
			return
		}
		if !errors.As(err, &remote) {
			t.Errorf("%s (%s): expected an error from the plugin, got %v", what, e.Why, err)
			return
		}
		if e.Throws.Name != nil && remote.Name != *e.Throws.Name {
			t.Errorf("%s (%s): the error's name %s, want %s", what, e.Why, remote.Name, *e.Throws.Name)
		}
		if e.Throws.Message != nil && remote.Message != *e.Throws.Message {
			t.Errorf("%s (%s): the error's message %s, want %s", what, e.Why, remote.Message, *e.Throws.Message)
		}
	default:
		t.Fatalf("%s: the expectations say neither returns nor throws", what)
	}
}

// call calls `method` and returns the value as it crossed the session.
func (s *Service) call(ctx context.Context, method string, args ...any) (any, error) {
	converted, err := convert.ToPeerArgs(args)
	if err != nil {
		return nil, err
	}
	value, err := s.session.Call(ctx, s.handle, method, converted, s.kinds[method] == "sync")
	if err != nil {
		return nil, err
	}
	return peer.Settle(ctx, value)
}

func TestRutistestMeetsTheRuntimeExpectations(t *testing.T) {
	data, err := os.ReadFile(filepath.FromSlash(expectations))
	if err != nil {
		t.Skipf("the expectations are not in this checkout: %v", err)
	}
	var expected struct {
		Service string            `json:"service"`
		Methods map[string]string `json:"methods"`
		Calls   []entry           `json:"calls"`
		Samples []entry           `json:"samples"`
		Events  []struct {
			Emit   string            `json:"emit"`
			Args   []json.RawMessage `json:"args"`
			Then   *entry            `json:"then"`
			Throws json.RawMessage   `json:"throws"`
			Why    string            `json:"why"`
		} `json:"events"`
		Undeclared struct {
			Service string            `json:"service"`
			Methods map[string]string `json:"methods"`
			Calls   []entry           `json:"calls"`
		} `json:"undeclared"`
	}
	if err := json.Unmarshal(data, &expected); err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	loaded := Load(t, weather.Plugin, nil, map[string]any{"clock": &clock{}})
	service := loaded.Service(expected.Service)
	for method, kind := range expected.Methods {
		if service.Methods()[method] != kind {
			t.Errorf("%s.%s is %s, want %s", expected.Service, method, service.Methods()[method], kind)
		}
	}
	for i, e := range expected.Calls {
		if e.Allowed != "" {
			continue
		}
		args := make([]any, len(e.Args))
		for j, arg := range e.Args {
			args[j] = argument(t, arg)
		}
		value, err := service.call(ctx, e.Call, args...)
		check(t, fmt.Sprintf("calls[%d] %s", i, e.Call), e, value, err)
	}
	// Values only Go makes.
	var names []string
	if err := service.Call(ctx, "samples", &names); err != nil {
		t.Fatal(err)
	}
	if len(names) == 0 {
		t.Fatal("the Go plugin names its samples")
	}
	for _, name := range names {
		var found *entry
		for i := range expected.Samples {
			if expected.Samples[i].Sample == name {
				found = &expected.Samples[i]
			}
		}
		if found == nil {
			t.Fatalf("the expectations list no sample %s", name)
		}
		value, err := service.call(ctx, "sample", name)
		check(t, "sample "+name, *found, value, err)
	}
	for _, event := range expected.Events {
		args := make([]any, len(event.Args))
		for j, arg := range event.Args {
			args[j] = argument(t, arg)
		}
		if event.Then == nil {
			// The declared-emits list is gone (#464): every name can be
			// emitted, so an entry about refusing an undeclared one does
			// not apply.
			continue
		}
		// As a host plugin emits: Parallel waits for the listeners.
		err := loaded.Parallel(ctx, event.Emit, args...)
		if err != nil {
			t.Fatalf("emit %s: %v", event.Emit, err)
		}
		then := *event.Then
		then.Why = event.Why
		value, err := service.call(ctx, then.Call)
		check(t, "after "+event.Emit, then, value, err)
	}
	// A service the plugin does not declare: its methods inferred, all
	// asynchronous.
	undeclared := expected.Undeclared
	if _, declared := describe(t, weather.Plugin, weather.Plugin.Name()).Provides[undeclared.Service]; declared {
		t.Fatalf("the plugin declares %s", undeclared.Service)
	}
	forecast := loaded.Service(undeclared.Service)
	if !reflect.DeepEqual(forecast.Methods(), undeclared.Methods) {
		t.Errorf("%s: methods %v, want %v", undeclared.Service, forecast.Methods(), undeclared.Methods)
	}
	for i, e := range undeclared.Calls {
		args := make([]any, len(e.Args))
		for j, arg := range e.Args {
			args[j] = argument(t, arg)
		}
		value, err := forecast.call(ctx, e.Call, args...)
		check(t, fmt.Sprintf("undeclared.calls[%d] %s", i, e.Call), e, value, err)
	}
}
