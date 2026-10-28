package rutis

import (
	"encoding/json"
	"reflect"
	"testing"
)

type nested struct {
	Port int `json:"port"`
}

type sample struct {
	City    string             `json:"city,omitempty" doc:"the city"`
	Days    int                `json:"days"`
	Tags    []string           `json:"tags,omitempty"`
	Limits  map[string]float64 `json:"limits,omitempty"`
	Server  nested             `json:"server"`
	Home    *nested            `json:"home"`
	Skipped string             `json:"-"`
	Plain   bool
}

// No `required` (a field not written is its zero value, design §3.3); a
// pointer also takes null.
func TestSchemaFromAStruct(t *testing.T) {
	schema, err := schemaOf(reflect.TypeFor[sample]())
	if err != nil {
		t.Fatal(err)
	}
	got, _ := json.Marshal(schema)
	want := `{"properties":{"Plain":{"type":"boolean"},"city":{"description":"the city","type":"string"},"days":{"type":"integer"},"home":{"properties":{"port":{"type":"integer"}},"type":["object","null"]},"limits":{"additionalProperties":{"type":"number"},"type":"object"},"server":{"properties":{"port":{"type":"integer"}},"type":"object"},"tags":{"items":{"type":"string"},"type":"array"}},"type":"object"}`
	if string(got) != want {
		t.Fatalf("got  %s\nwant %s", got, want)
	}
}

type custom struct{}

func (custom) JSONSchema() map[string]any {
	return map[string]any{"type": "string", "enum": []string{"a"}}
}

func TestASchemerGivesItsOwnSchema(t *testing.T) {
	schema, err := schemaOf(reflect.TypeFor[custom]())
	if err != nil || schema["type"] != "string" {
		t.Fatalf("%v %v", schema, err)
	}
}

type selfDecoding []string

func (s *selfDecoding) UnmarshalJSON(data []byte) error { return nil }

type textDecoding struct{ value string }

func (t *textDecoding) UnmarshalText(data []byte) error { t.value = string(data); return nil }

// A type that decodes itself keeps its own rules: its schema says only
// what encoding/json requires (config schema design §3.3).
func TestATypeThatDecodesItselfIsNotConstrainedByItsShape(t *testing.T) {
	type config struct {
		Does selfDecoding  `json:"does"`
		At   textDecoding  `json:"at"`
		When *selfDecoding `json:"when"`
	}
	schema, err := schemaOf(reflect.TypeFor[config]())
	if err != nil {
		t.Fatal(err)
	}
	got, _ := json.Marshal(schema)
	want := `{"properties":{"at":{"type":"string"},"does":{},"when":{}},"type":"object"}`
	if string(got) != want {
		t.Fatalf("got  %s\nwant %s", got, want)
	}
}

type Named string

type innerConfig struct {
	Lat float64 `json:"lat"`
}

type promoted struct {
	Units string `json:"units"`
}

// Fields as encoding/json decodes them (config schema design §3.3): a byte
// slice is a base64 string, a json.Number a number or a string, a
// `,string` field a string; a tagged embedded struct is one field, an
// untagged one's fields are promoted, an embedded type that is not a
// struct is a field named by its type; integer keys are keys.
func TestFieldsAreWhatEncodingJSONDecodes(t *testing.T) {
	type config struct {
		Bytes  []byte      `json:"bytes"`
		Num    json.Number `json:"num"`
		Port   int         `json:"port,string"`
		Maybe  *bool       `json:"maybe,string"`
		Inner  innerConfig `json:"inner"`
		ByCode map[int]string
		promoted
		Named
	}
	schema, err := schemaOf(reflect.TypeFor[config]())
	if err != nil {
		t.Fatal(err)
	}
	got, _ := json.Marshal(schema)
	want := `{"properties":{"ByCode":{"additionalProperties":{"type":"string"},"type":"object"},"Named":{"type":"string"},"bytes":{"type":"string"},"inner":{"properties":{"lat":{"type":"number"}},"type":"object"},"maybe":{"type":["string","null"]},"num":{"type":["number","string"]},"port":{"type":"string"},"units":{"type":"string"}},"type":"object"}`
	if string(got) != want {
		t.Fatalf("got  %s\nwant %s", got, want)
	}
}

type withName struct {
	Name string `json:"Name"`
}

type otherName struct {
	Name int
}

type embedsTagged struct {
	innerConfig `json:"inner"`
}

// A tagged embedded struct is one field; of two fields of a name at one
// depth the tagged one wins; a pointer to a schema with several types
// also takes null.
func TestEmbeddedFieldsFollowEncodingJSON(t *testing.T) {
	type config struct {
		embedsTagged
		withName
		otherName
		Num *json.Number `json:"num"`
	}
	schema, err := schemaOf(reflect.TypeFor[config]())
	if err != nil {
		t.Fatal(err)
	}
	got, _ := json.Marshal(schema)
	want := `{"properties":{"Name":{"type":"string"},"inner":{"properties":{"lat":{"type":"number"}},"type":"object"},"num":{"type":["number","string","null"]}},"type":"object"}`
	if string(got) != want {
		t.Fatalf("got  %s\nwant %s", got, want)
	}
}
