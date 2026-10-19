package events

import (
	"encoding/json"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// guarantee: events/no-result (§1: nil, undefined, null and false are no
// result, in Go and as received)
func TestWhatIsAResult(t *testing.T) {
	none := []any{nil, false, peer.Undefined, json.RawMessage("null"), json.RawMessage(" false "), (*int)(nil), []int(nil), map[string]any(nil)}
	for _, v := range none {
		if HasResult(v) {
			t.Errorf("%#v is a result", v)
		}
	}
	some := []any{0, "", true, json.RawMessage("0"), json.RawMessage(`""`), []int{}, struct{}{}}
	for _, v := range some {
		if !HasResult(v) {
			t.Errorf("%#v is no result", v)
		}
	}
}

func TestScopeIsAPrefixByID(t *testing.T) {
	t1 := []Scope{{ID: "17", Group: "tenant"}}
	s1 := []Scope{{ID: "17", Group: "tenant"}, {ID: "42", Group: "session"}}
	t2 := []Scope{{ID: "18", Group: "tenant"}}
	cases := []struct {
		listener, emitter []Scope
		hears             bool
	}{{nil, s1, true}, {t1, s1, true}, {s1, s1, true}, {s1, t1, false}, {t2, s1, false}, {t1, nil, false}}
	for _, c := range cases {
		if Within(c.listener, c.emitter) != c.hears {
			t.Errorf("%v hears %v: %v", c.listener, c.emitter, !c.hears)
		}
	}
}

func TestADescriptorCrossesAsTheProtocolWritesIt(t *testing.T) {
	data, err := json.Marshal(Listener{Name: "session/created", Plugin: "chat-ui", Row: "chat", Scope: []Scope{{ID: "17", Group: "tenant"}}})
	if err != nil {
		t.Fatal(err)
	}
	want := `{"name":"session/created","prefix":false,"plugin":"chat-ui","row":"chat","runtime":null,"node":null,"scope":[{"id":"17","group":"tenant"}],"global":false}`
	if string(data) != want {
		t.Fatalf("got %s", data)
	}
	var back Listener
	if err := json.Unmarshal([]byte(`{"name":"n","prefix":true,"plugin":"p","row":null,"runtime":"py","node":"b","scope":[],"global":true}`), &back); err != nil {
		t.Fatal(err)
	}
	if back.Row != "" || back.Runtime != "py" || back.Node != "b" || !back.Prefix || !back.Global {
		t.Fatalf("got %+v", back)
	}
}
