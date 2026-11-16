package rutis

import (
	"context"
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
