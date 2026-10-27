package config

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"testing"
)

// The shared case list (config schema design §8).
type testCase struct {
	Name     string          `json:"name"`
	Schema   json.RawMessage `json:"schema"`
	Config   json.RawMessage `json:"config"`
	Then     map[string]json.RawMessage
	Warnings []string `json:"warnings"`
}

func cases(t *testing.T) []testCase {
	t.Helper()
	data, err := os.ReadFile(filepath.Join("..", "..", "..", "..", "specs", "config-schema", "cases.json"))
	if err != nil {
		t.Fatal(err)
	}
	var list struct {
		Cases []testCase `json:"cases"`
	}
	if err := json.Unmarshal(data, &list); err != nil {
		t.Fatal(err)
	}
	return list.Cases
}

func decoded(t *testing.T, data json.RawMessage) any {
	t.Helper()
	if data == nil {
		return nil
	}
	value, err := Decode(data)
	if err != nil {
		t.Fatal(err)
	}
	return value
}

// strictlyEqual is JSON equality that also tells integers from fractions.
func strictlyEqual(a, b any) bool {
	switch x := a.(type) {
	case json.Number:
		y, ok := b.(json.Number)
		return ok && integerText(x) == integerText(y) && same(x, y)
	case []any:
		y, ok := b.([]any)
		if !ok || len(x) != len(y) {
			return false
		}
		for i := range x {
			if !strictlyEqual(x[i], y[i]) {
				return false
			}
		}
		return true
	case map[string]any:
		y, ok := b.(map[string]any)
		if !ok || len(x) != len(y) {
			return false
		}
		for key := range x {
			if !strictlyEqual(x[key], y[key]) {
				return false
			}
		}
		return true
	}
	return reflect.DeepEqual(a, b)
}

func lines(issues []Issue) []string {
	out := []string{}
	for _, problem := range issues {
		out = append(out, "config: "+problem.Message)
	}
	return out
}

// touch changes `value` in place everywhere: a key added to every object,
// an item to every array.
func touch(value any) any {
	switch v := value.(type) {
	case map[string]any:
		for key, child := range v {
			v[key] = touch(child)
		}
		v["__touched__"] = true
		return v
	case []any:
		for i, child := range v {
			v[i] = touch(child)
		}
		return append(v, "touched")
	}
	return value
}

// guarantee: sdk/config-schema (the shared case list)
func TestEveryCase(t *testing.T) {
	for _, c := range cases(t) {
		t.Run(c.Name, func(t *testing.T) {
			schema, config := decoded(t, c.Schema), decoded(t, c.Config)
			checked := Check(schema, config, true)
			warnings := c.Warnings
			if warnings == nil {
				warnings = []string{}
			}
			if got := lines(checked.Warnings); !slices.Equal(got, warnings) {
				t.Errorf("warnings %q, want %q", got, warnings)
			}
			if errors, failing := c.Then["errors"]; failing {
				var want []string
				if err := json.Unmarshal(errors, &want); err != nil {
					t.Fatal(err)
				}
				if got := Lines(checked.Errors); !slices.Equal(got, want) {
					t.Errorf("errors %q, want %q", got, want)
				}
				return
			}
			want := decoded(t, c.Then["config"])
			if len(checked.Errors) > 0 {
				t.Fatalf("errors %q", Lines(checked.Errors))
			}
			if !strictlyEqual(checked.Config, want) {
				t.Errorf("config %#v, want %#v", checked.Config, want)
			}
			// Defaults are copies (case 21): changing the first result
			// changes nothing of the second.
			touch(checked.Config)
			if again := Check(schema, config, true); !strictlyEqual(again.Config, want) {
				t.Errorf("second config %#v, want %#v", again.Config, want)
			}
		})
	}
}

func TestTheGivenConfigIsNotChanged(t *testing.T) {
	schema := decoded(t, json.RawMessage(`{"type":"object","properties":{"home":{"type":"object","properties":{"lat":{"default":1}}}}}`))
	config := decoded(t, json.RawMessage(`{"home":{}}`))
	checked := Check(schema, config, true)
	if want := decoded(t, json.RawMessage(`{"home":{"lat":1}}`)); !strictlyEqual(checked.Config, want) {
		t.Fatalf("%#v", checked.Config)
	}
	if want := decoded(t, json.RawMessage(`{"home":{}}`)); !strictlyEqual(config, want) {
		t.Fatalf("the given config changed: %#v", config)
	}
}

func TestRequiredCanBeLeftOut(t *testing.T) {
	schema := decoded(t, json.RawMessage(`{"type":"object","required":["city"]}`))
	if got := Check(schema, nil, false); len(got.Errors) != 0 {
		t.Fatalf("%q", Lines(got.Errors))
	}
}

func TestNumbersAreWrittenAsJavaScriptWritesThem(t *testing.T) {
	for f, text := range map[float64]string{1: "1", 1.5: "1.5", 1e21: "1e+21", 1.5e21: "1.5e+21", 1e-7: "1e-7", 0.000001: "0.000001", 123.456: "123.456", -2.5: "-2.5", 0.1: "0.1", 1.5e20: "150000000000000000000"} {
		if got := FloatText(f); got != text {
			t.Errorf("%v: %q, want %q", f, got, text)
		}
	}
}
