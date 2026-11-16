package rutis

import (
	"context"
	"errors"
	"strings"
	"testing"
)

// rule: events/cancel, events/registration ("撤下之后不再开始", #630).
// A listener withdrawn while one of its emits is still queued starts no
// invocation: the queued one re-checks that its listener is live after the
// gate the test holds opens, and answers nothing, the withdrawal it raced
// (#1138).
func TestAQueuedEmitDoesNotStartAfterItsListenerIsWithdrawn(t *testing.T) {
	rt, rows := eventRows(t, "")
	ran := make(chan struct{}, 1)
	dispose := rows[0].On("n", func(context.Context, ...any) (any, error) {
		ran <- struct{}{}
		return nil, nil
	})
	rt.mu.Lock()
	listener := rt.listeners["n"][0]
	rt.mu.Unlock()

	// Hold the listener's queue: the emit below is accepted, stays queued,
	// and cannot start until the test opens the gate. No timer decides this.
	gate := make(chan struct{})
	listener.queue.Push(func() { <-gate })
	wait := rt.eventCall(context.Background(), []any{map[string]any{
		"listener": listener.id, "name": "n", "mode": "emit", "args": []any{},
	}})
	// Withdrawn while its emit is still queued.
	dispose()
	close(gate)
	_, err := wait()
	if err != nil {
		t.Fatalf("the queued emit answered %v", err)
	}
	select {
	case <-ran:
		t.Fatal("the listener started after off returned")
	default:
	}
}

// rule: EVP-4.4-4, EVP-5.1-3 ("撤下之后不再开始", #630).
// A dispatcher's snapshot can name a listener that `off` withdrew before
// the invocation started: Begin re-checks that it is live under rt.mu and
// starts nothing, and a call already on its way answers as one that
// returns nothing (#1138).
func TestAListenerWithdrawnBeforeItStartsRunsNothing(t *testing.T) {
	rt, rows := eventRows(t, "")
	ran := make(chan struct{}, 1)
	dispose := rows[0].On("n", func(context.Context, ...any) (any, error) {
		ran <- struct{}{}
		return nil, nil
	})
	rt.mu.Lock()
	listener := rt.listeners["n"][0]
	rt.mu.Unlock()

	// The snapshot was taken before off returned; starting from it now must
	// not call the listener.
	dispose()
	if call, ok := listener.Begin(); ok {
		_, _ = call(context.Background(), "serial", nil, nil)
		t.Fatal("the listener started after off returned")
	}
	// An events.call already on its way answers as one that returns nothing
	// (#1138), and starts no invocation.
	for _, mode := range []string{"serial", "parallel", "bail"} {
		value, err := rt.eventCall(context.Background(), []any{map[string]any{
			"listener": listener.id, "name": "n", "mode": mode, "args": []any{},
		}})()
		if err != nil || value != nil {
			t.Fatalf("%s: the withdrawn call answered %v, %v", mode, value, err)
		}
	}
	// An id this process never registered is still the protocol failure.
	_, err := rt.eventCall(context.Background(), []any{map[string]any{
		"listener": "999", "name": "n", "mode": "serial", "args": []any{},
	}})()
	if !errors.Is(err, ErrBinding) || !strings.Contains(err.Error(), "is withdrawn") {
		t.Fatalf("an unknown listener: %v", err)
	}
	select {
	case <-ran:
		t.Fatal("the listener ran after off returned")
	default:
	}
}
