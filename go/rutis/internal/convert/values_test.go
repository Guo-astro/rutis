package convert

import (
	"bytes"
	"context"
	"encoding/json"
	"math/big"
	"net"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
	"github.com/arcships/rutis/go/rutis/internal/values"
)

// session is a host session calling Go functions on a runtime session,
// across a real session: `call(name, result, args...)` calls
// functions[name] there and decodes its result into `result`.
func session(t *testing.T, functions map[string]any) func(name string, result any, args ...any) error {
	t.Helper()
	a, b := net.Pipe()
	host, err := peer.New(peer.NewLines(a), peer.Options{Host: true})
	if err != nil {
		t.Fatal(err)
	}
	runtime, err := peer.New(peer.NewLines(b), peer.Options{Dispatch: func(ctx context.Context, target, method string, args []any) (any, error) {
		return Call(ctx, reflect.ValueOf(functions[method]), args)
	}})
	if err != nil {
		t.Fatal(err)
	}
	started := make(chan error, 2)
	for _, p := range []*peer.Peer{host, runtime} {
		go func(p *peer.Peer) { started <- p.Start() }(p)
	}
	for range 2 {
		if err := testwait.Within(t, "the session to start", started); err != nil {
			t.Fatal(err)
		}
	}
	for _, p := range []*peer.Peer{host, runtime} {
		testwait.Within(t, "the handshake", p.Ready())
	}
	t.Cleanup(func() {
		host.Close(nil)
		runtime.Close(nil)
		a.Close()
		b.Close()
	})
	return func(name string, result any, args ...any) error {
		converted, err := ToPeerArgs(args)
		if err != nil {
			return err
		}
		value, err := host.Call(context.Background(), "x", name, converted, true)
		if err != nil {
			return err
		}
		return Decode(value, result)
	}
}

type Reading struct {
	At      time.Time           `json:"at"`
	Raw     []byte              `json:"raw"`
	Day     values.Date         `json:"day"`
	Total   *big.Int            `json:"total"`
	Price   values.Decimal      `json:"price"`
	ID      values.UUID         `json:"id"`
	ByHour  map[int]float64     `json:"byHour"`
	Tags    map[string]struct{} `json:"tags"`
	Skipped string              `json:"-"`
	Empty   string              `json:"empty,omitempty"`
	Note    *string             `json:"note"`
}

func TestStructFieldsCrossAsTheirValues(t *testing.T) {
	var got Reading
	oslo := time.FixedZone("Oslo", 2*3600)
	total, _ := new(big.Int).SetString("123456789012345678901234567890", 10)
	sent := Reading{
		At:      time.Date(2026, 10, 15, 5, 22, 15, 123456789, oslo),
		Raw:     []byte("hello"),
		Day:     values.Date{Year: 2026, Month: 10, Day: 15},
		Total:   total,
		Price:   "1.10",
		ID:      "0f8fad5b-d9cb-469f-a165-70867728950e",
		ByHour:  map[int]float64{9: 1.5, 10: 2},
		Tags:    map[string]struct{}{"a": {}, "b": {}},
		Skipped: "not sent",
	}
	call := session(t, map[string]any{"echo": func(r Reading) Reading { return r }})
	if err := call("echo", &got, sent); err != nil {
		t.Fatal(err)
	}
	if !got.At.Equal(sent.At) || got.At.Location() != time.UTC || got.At.Nanosecond() != 123456789 {
		t.Errorf("an instant in UTC to the nanosecond: %v", got.At)
	}
	if !bytes.Equal(got.Raw, sent.Raw) || got.Day != sent.Day || got.Total.Cmp(total) != 0 || got.Price != "1.10" || got.ID != sent.ID {
		t.Errorf("%+v", got)
	}
	if !reflect.DeepEqual(got.ByHour, sent.ByHour) || !reflect.DeepEqual(got.Tags, sent.Tags) || got.Skipped != "" {
		t.Errorf("%+v", got)
	}

	// What crosses: a record with tagged fields, not encoding/json text.
	value, err := ToPeer(sent)
	if err != nil {
		t.Fatal(err)
	}
	record := value.(map[string]any)
	if _, ok := record["at"].(time.Time); !ok {
		t.Errorf("at: %#v", record["at"])
	}
	if _, ok := record["raw"].([]byte); !ok {
		t.Errorf("raw: %#v", record["raw"])
	}
	if hours := record["byHour"].(peer.Map); len(hours) != 2 || hours[0].Key != int64(9) {
		t.Errorf("a map with integer keys crosses as a map, keys in order: %#v", hours)
	}
	if tags := record["tags"].(peer.Set); !reflect.DeepEqual(tags, peer.Set{"a", "b"}) {
		t.Errorf("map[K]struct{} crosses as a set: %#v", tags)
	}
	if _, ok := record["empty"]; ok {
		t.Error("omitempty")
	}
	if _, ok := record["Skipped"]; ok {
		t.Error(`json:"-"`)
	}
	if record["note"] != nil {
		t.Errorf("a nil pointer is null: %#v", record["note"])
	}
}

type Embedded struct {
	Name string `json:"name"`
	Deep int
}

type Outer struct {
	Embedded
	Deep  string      // shallower: hides Embedded.Deep
	Count int         `json:"count,string"`
	Zero  values.Date `json:"zero,omitzero"`
}

type Celsius float64

func (c Celsius) MarshalJSON() ([]byte, error) { return []byte(`{"celsius": 21}`), nil }

func TestStructsAreWrittenAsEncodingJSONWritesThem(t *testing.T) {
	value, err := ToPeer(Outer{Embedded: Embedded{Name: "n", Deep: 1}, Deep: "d", Count: 3})
	if err != nil {
		t.Fatal(err)
	}
	if want := map[string]any{"name": "n", "Deep": "d", "count": `3`}; !reflect.DeepEqual(value, want) {
		t.Errorf("%#v", value)
	}
	value, err = ToPeer(Celsius(21))
	if err != nil || string(value.(json.RawMessage)) != `{"celsius":21}` {
		t.Errorf("a MarshalJSON writes itself: %#v %v", value, err)
	}
	// A big integer written by a MarshalJSON is checked too.
	if _, err := ToPeer(json.Number("9007199254740993")); err == nil {
		t.Error("json.Number beyond 2^53")
	}
}

func TestMapsAndSetsDecodeIntoTheirDeclaredTypes(t *testing.T) {
	call := session(t, map[string]any{
		"echo": func(v any) any { return v },
		"keys": func() map[any]any { return map[any]any{"1": "text", values.Decimal("1"): "decimal"} },
		"big":  func() *big.Int { return big.NewInt(42) },
		"set":  func() map[values.Date]struct{} { return map[values.Date]struct{}{{Year: 2026, Month: 10, Day: 15}: {}} },
		"when": func() values.DateTime {
			return values.DateTime{Date: values.Date{Year: 2026, Month: 10, Day: 15}, Time: values.TimeOfDay{Hour: 14, Minute: 30}}
		},
	})
	var anything any
	if err := call("echo", &anything, map[int]string{1: "a", 2: "b"}); err != nil {
		t.Fatal(err)
	}
	if want := map[any]any{float64(1): "a", float64(2): "b"}; !reflect.DeepEqual(anything, want) {
		t.Errorf("a map into any is map[any]any: %#v", anything)
	}
	var typed map[int]string
	if err := call("echo", &typed, map[int]string{1: "a", 2: "b"}); err != nil || typed[2] != "b" {
		t.Errorf("%v %v", typed, err)
	}
	// "1" and Decimal("1") are two keys; as map[string]string they are one.
	var texts map[string]string
	err := call("keys", &texts)
	if err == nil || !strings.Contains(err.Error(), "would lose entries") {
		t.Errorf("%v %v", texts, err)
	}
	var both map[any]string
	if err := call("keys", &both); err != nil || len(both) != 2 || both[values.Decimal("1")] != "decimal" {
		t.Errorf("%v %v", both, err)
	}
	var n int64
	if err := call("big", &n); err != nil || n != 42 {
		t.Errorf("a bigint into an integer it fits: %v %v", n, err)
	}
	var small int8
	if err := call("echo", &small, big.NewInt(300)); err == nil {
		t.Error("a bigint into an integer it does not fit")
	}
	var dates []values.Date
	if err := call("set", &dates); err != nil || len(dates) != 1 || dates[0].Day != 15 {
		t.Errorf("a set into a slice: %v %v", dates, err)
	}
	var dateSet map[values.Date]struct{}
	if err := call("set", &dateSet); err != nil || len(dateSet) != 1 {
		t.Errorf("a set into map[K]struct{}: %v %v", dateSet, err)
	}
	var text string
	if err := call("when", &text); err != nil || text != "2026-10-15T14:30:00" {
		t.Errorf("a datetime into a string is its text: %q %v", text, err)
	}
	var instant time.Time
	if err := call("when", &instant); err == nil {
		t.Error("a datetime is not an instant (V5)")
	}
	var fixed [5]byte
	if err := call("echo", &fixed, []byte("hello")); err != nil || string(fixed[:]) != "hello" {
		t.Errorf("bytes into [N]byte: %v %v", fixed, err)
	}
	var blob []byte
	if err := call("echo", &blob, [3]byte{1, 2, 3}); err != nil || !bytes.Equal(blob, []byte{1, 2, 3}) {
		t.Errorf("[N]byte crosses as bytes: %v %v", blob, err)
	}
	var pointer *time.Time
	at := time.Date(2026, 1, 2, 3, 4, 5, 6, time.UTC)
	if err := call("echo", &pointer, &at); err != nil || pointer == nil || !pointer.Equal(at) {
		t.Errorf("*time.Time: %v %v", pointer, err)
	}
}
