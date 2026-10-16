package convert

import (
	"context"
	"encoding/json"
	"math"
	"reflect"
	"strings"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

func TestWireName(t *testing.T) {
	for goName, wire := range map[string]string{
		"Today": "today", "ID": "id", "URLFor": "urlFor", "HTTPServer": "httpServer",
		"GetURL": "getURL", "ListModels": "listModels", "ID2": "id2", "already": "already",
	} {
		if got := WireName(goName); got != wire {
			t.Errorf("WireName(%q) = %q, want %q", goName, got, wire)
		}
	}
}

type point struct {
	X int `json:"x"`
	Y int `json:"y"`
}

func TestDataCrossesValueByValue(t *testing.T) {
	value, err := ToPeer(point{1, 2})
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(value, map[string]any{"x": int64(1), "y": int64(2)}) {
		t.Fatalf("%#v", value)
	}
	var back point
	if err := Decode(json.RawMessage(`{"x":1,"y":2}`), &back); err != nil || back != (point{1, 2}) {
		t.Fatalf("%v %v", back, err)
	}
	if _, err := ToPeer(int64(1) << 60); err == nil {
		t.Fatal("an integer beyond 2^53 must not cross")
	}
	// Inside a struct too (V9): encoding/json would not have checked it.
	_, err = ToPeer(struct{ N uint64 }{1 << 60})
	if err == nil || !strings.Contains(err.Error(), "send it as a *big.Int") {
		t.Fatalf("%v", err)
	}
	if _, err := ToPeer(struct{ C chan int }{make(chan int)}); err == nil {
		t.Fatal("a channel cannot cross")
	}
	if _, err := ToPeer([]float64{math.NaN()}); err == nil {
		t.Fatal("NaN cannot cross")
	}
}

func TestFunctionsCrossAndAreCalledWithWireArguments(t *testing.T) {
	value, err := ToPeer(func(ctx context.Context, a int, b string) (string, error) {
		return b + string(rune('0'+a)), nil
	})
	if err != nil {
		t.Fatal(err)
	}
	fn, ok := value.(peer.Func)
	if !ok {
		t.Fatalf("a Go function crosses as a peer.Func, not %T", value)
	}
	result, err := fn(context.Background(), []any{json.RawMessage("3"), json.RawMessage(`"n"`)})
	if err != nil {
		t.Fatal(err)
	}
	if result != "n3" {
		t.Fatalf("%#v", result)
	}
}

func TestCallStripsTrailingUndefinedAndRejectsExtraArguments(t *testing.T) {
	fn := reflect.ValueOf(func(a int) int { return a + 1 })
	if _, err := Call(context.Background(), fn, []any{json.RawMessage("1"), json.RawMessage("2")}); err == nil {
		t.Fatal("extra arguments are an error")
	}
	value, err := Call(context.Background(), fn, nil)
	if err != nil || value != int64(1) {
		t.Fatalf("a missing argument is its zero value: %v %v", value, err)
	}
}

func TestPanicsBecomeErrors(t *testing.T) {
	_, err := Call(context.Background(), reflect.ValueOf(func() { panic("boom") }), nil)
	if err == nil || err.Error() != "panic: boom" {
		t.Fatalf("%v", err)
	}
}
