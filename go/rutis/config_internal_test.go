package rutis

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/config"
	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// The shared case list (config schema design §8), each case through what
// the runtime runs before Apply: a definition's apply with the case's
// schema as its Schema.
// guarantee: sdk/config-schema (the shared case list)
func TestEveryConfigCaseBeforeApply(t *testing.T) {
	data, err := os.ReadFile(filepath.Join("..", "..", "specs", "config-schema", "cases.json"))
	if err != nil {
		t.Fatal(err)
	}
	var list struct {
		Cases []struct {
			Name   string                     `json:"name"`
			Schema map[string]any             `json:"schema"`
			Config json.RawMessage            `json:"config"`
			Then   map[string]json.RawMessage `json:"then"`
		} `json:"cases"`
	}
	if err := json.Unmarshal(data, &list); err != nil {
		t.Fatal(err)
	}
	for _, c := range list.Cases {
		t.Run(c.Name, func(t *testing.T) {
			var got json.RawMessage
			applied := false
			def := Define(Plugin[json.RawMessage]{
				Name:   "weather",
				Schema: c.Schema,
				Apply: func(_ *Ctx, config json.RawMessage) error {
					got, applied = config, true
					return nil
				},
			})
			var given any
			if c.Config != nil {
				given = c.Config
			}
			err := def.apply(nil, given)
			if errors, failing := c.Then["errors"]; failing {
				var want []string
				if err := json.Unmarshal(errors, &want); err != nil {
					t.Fatal(err)
				}
				if applied {
					t.Fatal("Apply ran")
				}
				coded := badConfigOf(t, err)
				if coded.Error() != strings.Join(want, "\n") {
					t.Fatalf("%q, want %q", coded.Error(), strings.Join(want, "\n"))
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			if !applied {
				t.Fatal("Apply did not run")
			}
			want, _ := config.Decode(c.Then["config"])
			if !sameJSON(t, got, want) {
				t.Fatalf("Apply got %s, want %s", got, c.Then["config"])
			}
		})
	}
}

// badConfigOf is `err` as the bad-config BindingError it should be.
func badConfigOf(t *testing.T, err error) *peer.Coded {
	t.Helper()
	var coded *peer.Coded
	if !errors.As(err, &coded) || coded.Name() != "BindingError" || coded.Code() != "bad-config" {
		t.Fatalf("not a bad-config BindingError: %#v", err)
	}
	return coded
}

// sameJSON is JSON equality that also tells integers from fractions.
func sameJSON(t *testing.T, got json.RawMessage, want any) bool {
	t.Helper()
	if len(got) == 0 {
		// null decodes into a RawMessage as nothing.
		return want == nil
	}
	value, err := config.Decode(got)
	if err != nil {
		t.Fatal(err)
	}
	return reflect.DeepEqual(value, want)
}

type forecastConfig struct {
	City  string        `json:"city"`
	Days  int           `json:"days"`
	Home  *forecastHome `json:"home"`
	Units string        `json:"units"`
}

type forecastHome struct {
	Lat float64 `json:"lat"`
}

// What a typed config gets (design §3.3): no `required`, a field not
// written is its zero value; a pointer tells it apart; a default in a
// hand-written schema is filled; an extra key passes (the host warns).
func TestATypedConfigIsCheckedThenDecoded(t *testing.T) {
	var got forecastConfig
	def := Define(Plugin[forecastConfig]{
		Name:  "weather",
		Apply: func(_ *Ctx, config forecastConfig) error { got = config; return nil },
	})
	if err := def.apply(nil, json.RawMessage(`{"city":"Oslo","cty":"Oslo"}`)); err != nil {
		t.Fatal(err)
	}
	if got != (forecastConfig{City: "Oslo"}) {
		t.Fatalf("%+v", got)
	}
	if err := def.apply(nil, nil); err != nil {
		t.Fatal(err)
	}
	if err := def.apply(nil, json.RawMessage(`{"home":null}`)); err != nil || got.Home != nil {
		t.Fatalf("%+v %v", got, err)
	}
	err := def.apply(nil, json.RawMessage(`{"days":1.5,"home":{"lat":"north"}}`))
	if coded := badConfigOf(t, err); coded.Error() != "config: days should be an integer, got 1.5\nconfig: home.lat should be a number, got \"north\"" {
		t.Fatalf("%q", coded.Error())
	}
	data := badConfigOf(t, err).Data()
	if !reflect.DeepEqual(data, map[string]any{"issues": []any{
		map[string]any{"path": []any{"days"}, "message": "days should be an integer, got 1.5"},
		map[string]any{"path": []any{"home", "lat"}, "message": `home.lat should be a number, got "north"`},
	}}) {
		t.Fatalf("%#v", data)
	}

	withDefault := Define(Plugin[forecastConfig]{
		Name: "weather",
		Schema: map[string]any{"type": "object", "properties": map[string]any{
			"units": map[string]any{"enum": []string{"metric", "imperial"}, "default": "metric"},
			"days":  map[string]any{"type": "integer", "minimum": 1, "default": 3},
		}},
		Apply: func(_ *Ctx, config forecastConfig) error { got = config; return nil },
	})
	if err := withDefault.apply(nil, json.RawMessage(`{}`)); err != nil || got.Units != "metric" || got.Days != 3 {
		t.Fatalf("%+v %v", got, err)
	}
	err = withDefault.apply(nil, json.RawMessage(`{"units":"kelvin","days":0}`))
	if coded := badConfigOf(t, err); coded.Error() != "config: days should be at least 1, got 0\nconfig: units should be \"metric\" or \"imperial\", got \"kelvin\"" {
		t.Fatalf("%q", coded.Error())
	}
}

// A config the schema passes that the type does not decode (the schema and
// the type disagree) is said without the decoder's text (design §3.3).
func TestAConfigTheTypeDoesNotTakeIsSaidPlainly(t *testing.T) {
	def := Define(Plugin[forecastConfig]{
		Name:   "weather",
		Schema: map[string]any{"type": "object", "properties": map[string]any{"days": map[string]any{"type": "string"}}},
		Apply:  func(*Ctx, forecastConfig) error { return nil },
	})
	err := def.apply(nil, json.RawMessage(`{"days":"three"}`))
	if coded := badConfigOf(t, err); coded.Error() != "config: days is not a value the plugin accepts" {
		t.Fatalf("%q", coded.Error())
	}
}

// A plugin without config takes what it is given, as before.
func TestNoConfigTakesWhatItIsGiven(t *testing.T) {
	def := Define(Plugin[NoConfig]{Name: "quiet", Apply: func(*Ctx, NoConfig) error { return nil }})
	if def.schema != nil {
		t.Fatalf("%v", def.schema)
	}
	if err := def.apply(nil, json.RawMessage(`{"anything":1}`)); err != nil {
		t.Fatal(err)
	}
}

type deepConfig struct {
	Home  forecastHome            `json:"home"`
	Homes []forecastHome          `json:"homes"`
	ByKey map[string]forecastHome `json:"byKey"`
	Count uint                    `json:"count"`
}

// A decoder error below the top level names the path as far as it can be
// told: the fields down to the first array or map; `-0` is 0.
func TestDecoderPathsAndNegativeZero(t *testing.T) {
	var got deepConfig
	def := Define(Plugin[deepConfig]{
		Name: "weather",
		// The schema lets anything through, so the decoder decides.
		Schema: map[string]any{"type": "object", "properties": map[string]any{"home": map[string]any{}, "homes": map[string]any{}, "byKey": map[string]any{}, "count": map[string]any{"type": "integer"}}},
		Apply:  func(_ *Ctx, config deepConfig) error { got = config; return nil },
	})
	for given, want := range map[string]string{
		`{"home":{"lat":"north"}}`:          "config: home.lat is not a value the plugin accepts",
		`{"homes":[{},{"lat":"north"}]}`:    "config: homes is not a value the plugin accepts",
		`{"byKey":{"a.b":{"lat":"north"}}}`: "config: byKey is not a value the plugin accepts",
	} {
		err := def.apply(nil, json.RawMessage(given))
		if coded := badConfigOf(t, err); coded.Error() != want {
			t.Errorf("%s: %q, want %q", given, coded.Error(), want)
		}
	}
	if err := def.apply(nil, json.RawMessage(`{"count":-0}`)); err != nil || got.Count != 0 {
		t.Fatalf("%+v %v", got, err)
	}
	if text := config.ValueText(json.Number("-0")); text != "0" {
		t.Fatalf("%q", text)
	}
}

type promotedHome struct {
	forecastHome
}

// A field promoted from an embedded struct is named by its key.
func TestADecoderErrorInAPromotedFieldNamesIt(t *testing.T) {
	def := Define(Plugin[promotedHome]{
		Name:   "weather",
		Schema: map[string]any{"type": "object", "properties": map[string]any{"lat": map[string]any{}}},
		Apply:  func(*Ctx, promotedHome) error { return nil },
	})
	err := def.apply(nil, json.RawMessage(`{"lat":"north"}`))
	if coded := badConfigOf(t, err); coded.Error() != "config: lat is not a value the plugin accepts" {
		t.Fatalf("%q", coded.Error())
	}
}
