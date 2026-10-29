package rutis

import (
	"encoding/json"
	"errors"
	"reflect"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// badConfigOf is `err` as the bad-config BindingError it should be.
func badConfigOf(t *testing.T, err error) *peer.Coded {
	t.Helper()
	var coded *peer.Coded
	if !errors.As(err, &coded) || coded.Name() != "BindingError" || coded.Code() != "bad-config" {
		t.Fatalf("not a bad-config BindingError: %#v", err)
	}
	return coded
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

// What a typed config gets: the config as the host sent it (validated,
// its defaults filled in), decoded; a field not written is its zero
// value; a pointer tells it apart; an extra key passes (the host warns).
// A value of the wrong type is said without the decoder's text.
func TestATypedConfigIsDecoded(t *testing.T) {
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
	err := def.apply(nil, json.RawMessage(`{"days":1.5}`))
	if coded := badConfigOf(t, err); coded.Error() != "config: days is not a value the plugin accepts" {
		t.Fatalf("%q", coded.Error())
	}
	data := badConfigOf(t, err).Data()
	if !reflect.DeepEqual(data, map[string]any{"issues": []any{
		map[string]any{"path": []any{"days"}, "message": "days is not a value the plugin accepts"},
	}}) {
		t.Fatalf("%#v", data)
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
// told: the fields down to the first array or map.
func TestDecoderPaths(t *testing.T) {
	def := Define(Plugin[deepConfig]{
		Name: "weather",
		// The schema lets anything through, so the decoder decides.
		Schema: map[string]any{"type": "object", "properties": map[string]any{"home": map[string]any{}, "homes": map[string]any{}, "byKey": map[string]any{}, "count": map[string]any{"type": "integer"}}},
		Apply:  func(*Ctx, deepConfig) error { return nil },
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
