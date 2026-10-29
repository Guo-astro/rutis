// guarantee: sdk/error-positions
package rutis

import (
	"context"
	"encoding/json"
	"errors"
	"sync"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// futureHost is the host's end of a session that answers every
// events.dispatch as the Rust host answers a parallel one: with a future,
// which fails with `failure` when awaited.
type futureHost struct {
	peer.Channel
	failure  string
	mu       sync.Mutex
	pending  map[string]bool
	awaited  bool
	answered chan struct{}
}

const futureID = 9001

func (f *futureHost) Recv() ([]byte, error) {
	for {
		message, err := f.Channel.Recv()
		if err != nil {
			return nil, err
		}
		var head struct {
			Op        string          `json:"op"`
			ID        json.RawMessage `json:"id"`
			Method    string          `json:"method"`
			Reference json.RawMessage `json:"reference"`
		}
		_ = json.Unmarshal(message, &head)
		switch {
		case head.Op == "invoke" && head.Method == "events.dispatch":
			f.mu.Lock()
			f.pending[string(head.ID)] = true
			f.mu.Unlock()
		case head.Op == "await" && string(head.Reference) == "9001":
			f.mu.Lock()
			f.awaited = true
			f.mu.Unlock()
			if err := f.Channel.Send([]byte(`{"op":"throw","id":` + string(head.ID) + `,"error":` + f.failure + `}`)); err != nil {
				return nil, err
			}
			continue
		case head.Op == "release" && string(head.Reference) == "9001":
			continue
		}
		return message, nil
	}
}

func (f *futureHost) Send(message []byte) error {
	var head struct {
		Op string          `json:"op"`
		ID json.RawMessage `json:"id"`
	}
	_ = json.Unmarshal(message, &head)
	if head.Op == "return" {
		f.mu.Lock()
		dispatch := f.pending[string(head.ID)]
		delete(f.pending, string(head.ID))
		f.mu.Unlock()
		if dispatch {
			message = []byte(`{"op":"return","id":` + string(head.ID) + `,"value":{"type":"reference","value":{"id":9001,"kind":"future","home":false,"origin":[]}}}`)
		}
	}
	return f.Channel.Send(message)
}

// guarantee: sdk/leaf-events (a Parallel the host answers with a future
// that fails with an AggregateError returns an *AggregateError, its
// failures in order: #677)
func TestParallelThroughAFutureReturnsAnAggregateError(t *testing.T) {
	failure := `{"name":"AggregateError","message":"2 listeners of t/par failed","graph":{"root":{"type":"reference","value":0},"nodes":[` +
		`{"type":"error","name":"AggregateError","message":"2 listeners of t/par failed","errors":[{"type":"reference","value":1},{"type":"reference","value":2}]},` +
		`{"type":"error","name":"TypeError","message":"first","stack":"TypeError: first"},` +
		`{"type":"error","name":"Busy","message":"second","code":"Busy","data":{"type":"data","value":7}}]}}`
	var host *futureHost
	var c *Ctx
	def := Define(Plugin[NoConfig]{Name: "parallel", Apply: func(ctx *Ctx, _ NoConfig) error { c = ctx; return nil }})
	h := startFakeHostWith(t, func(channel peer.Channel) peer.Channel {
		host = &futureHost{Channel: channel, failure: failure, pending: map[string]bool{}}
		return host
	}, nil, def)
	if err := h.load("p", "parallel"); err != nil {
		t.Fatal(err)
	}
	err := c.Parallel(context.Background(), "t/par")
	host.mu.Lock()
	awaited := host.awaited
	host.mu.Unlock()
	if !awaited {
		t.Fatalf("the dispatch's future was not awaited: %v", err)
	}
	var aggregate *peer.AggregateError
	if !errors.As(err, &aggregate) {
		t.Fatalf("not an *AggregateError: %#v", err)
	}
	if aggregate.Message != "2 listeners of t/par failed" || len(aggregate.Errors) != 2 {
		t.Fatalf("aggregate: %v", aggregate)
	}
	one, two := aggregate.Errors[0].(*RemoteError), aggregate.Errors[1].(*RemoteError)
	if one.Name != "TypeError" || one.Message != "first" || two.Name != "Busy" || two.Code != "Busy" || two.Data != float64(7) {
		t.Fatalf("failures lost or reordered: %#v %#v", one, two)
	}
}
