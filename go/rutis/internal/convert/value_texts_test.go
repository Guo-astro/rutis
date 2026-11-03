package convert

import (
	"encoding/json"
	"errors"
	"math"
	"reflect"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// What a value that cannot cross says (design §5.3, docs/design-value-errors-
// 2026-11-03.md): one frame, one sentence of reason, and `data.reason` for a
// program. The reasons are rutis's own; the sentences are the ones design
// §4.2 sets.
func TestAValueThatCannotCrossSaysWhy(t *testing.T) {
	self := make([]any, 1)
	self[0] = self
	node := &probeCycle{}
	node.Self = node
	deep := any(1)
	for i := 0; i < 70; i++ {
		deep = []any{deep}
	}
	cases := []struct {
		value  any
		reason string
		says   string
	}{
		{math.NaN(), peer.ReasonNonFinite, "the number NaN is not finite"},
		{math.Inf(-1), peer.ReasonNonFinite, "the number -Infinity is not finite"},
		{math.Inf(1), peer.ReasonNonFinite, "the number Infinity is not finite"},
		{self, peer.ReasonCycle, "the value holds itself (a cycle)"},
		{node, peer.ReasonCycle, "the value holds itself (a cycle)"},
		{deep, peer.ReasonTooDeep, "the value nests deeper than 64 levels"},
		{struct{ C chan int }{make(chan int)}, peer.ReasonType, "a chan int cannot cross between processes"},
	}
	for _, c := range cases {
		_, err := ToPeer(c.value)
		if err == nil {
			t.Fatalf("%#v crossed", c.value)
		}
		if err.Error() != c.says {
			t.Errorf("said %q, want %q", err, c.says)
		}
		var reasoned peer.Reasoned
		if !errors.As(err, &reasoned) || reasoned.Reason() != c.reason {
			t.Errorf("%v: reason %q, want %q", err, reasonOf(err), c.reason)
		}
	}
}

// A value of one shape in two places is not a cycle (design D3).
func TestAValueSharedIsNotACycle(t *testing.T) {
	shared := []any{1}
	if _, err := ToPeer([]any{shared, shared}); err != nil {
		t.Fatal(err)
	}
	one := map[string]any{"a": 1}
	if _, err := ToPeer([]any{one, one}); err != nil {
		t.Fatal(err)
	}
}

type probeCycle struct{ Self *probeCycle }

func reasonOf(err error) string {
	var reasoned peer.Reasoned
	if errors.As(err, &reasoned) {
		return reasoned.Reason()
	}
	return ""
}

// The decoder's own sentence, with only the position prefix (design §4.5).
func TestAnArgumentThatDoesNotDecodeKeepsTheDecodersSentence(t *testing.T) {
	c := Callee{Service: "weather", Method: "today"}
	var text string
	textType := func() reflect.Type { return reflect.TypeOf(text) }
	err := Decode(json.RawMessage(`{"text":"cloudy"}`), &text)
	if err == nil {
		t.Fatal("an object decoded as a string")
	}
	wrapped := badArgument(c, 0, textType(), json.RawMessage(`{"text":"cloudy"}`), err)
	if want := "weather.today: argument 1: json: cannot unmarshal object into Go value of type string"; wrapped.Error() != want {
		t.Errorf("said %q, want %q", wrapped, want)
	}
	if code := wrapped.(interface{ Code() string }).Code(); code != "bad-argument" {
		t.Errorf("code %q", code)
	}
}
