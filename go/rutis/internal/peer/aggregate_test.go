package peer

import (
	"encoding/json"
	"errors"
	"testing"
)

// A received AggregateError becomes an *AggregateError again: its message,
// and its failures in order, each with its name, message and code.
func TestAReceivedAggregateKeepsItsFailuresInOrder(t *testing.T) {
	sent := &AggregateError{Message: "2 listeners of t/par failed", Errors: []error{
		Errorf("AErr", "a"),
		&RemoteError{Name: "BErr", Message: "b"},
	}}
	wire, err := json.Marshal(encodeError(sent))
	if err != nil {
		t.Fatal(err)
	}
	received := decodeError(wire)
	if received.(*RemoteError).Message != sent.Message {
		t.Fatalf("message %q", received.(*RemoteError).Message)
	}
	var aggregate *AggregateError
	if !errors.As(Aggregated(received), &aggregate) || aggregate.Message != sent.Message || len(aggregate.Errors) != 2 {
		t.Fatalf("got %v", Aggregated(received))
	}
	for i, name := range []string{"AErr", "BErr"} {
		if ErrorName(aggregate.Errors[i]) != name || !errors.Is(aggregate.Errors[i], NewSentinel(name)) {
			t.Fatalf("failure %d: %v", i, aggregate.Errors[i])
		}
	}
	// Re-thrown, it crosses whole again.
	again, _ := json.Marshal(encodeError(aggregate))
	if back := Aggregated(decodeError(again)).(*AggregateError); len(back.Errors) != 2 || ErrorName(back.Errors[1]) != "BErr" {
		t.Fatalf("re-thrown: %v", back)
	}
	if other := errors.New("x"); Aggregated(other) != other {
		t.Fatal("another error changed")
	}
}
