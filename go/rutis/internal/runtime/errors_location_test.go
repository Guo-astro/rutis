package rutis

import (
	"context"
	"encoding/json"
	"errors"
	"regexp"
	"strings"
	"testing"
)

// Errors a Go service method returns, as they cross the session to the
// host (#677).

type locating struct{}

func (*locating) Fail(context.Context) error { return errors.New("no forecast") }

func (*locating) Panic(context.Context) error { panic("bad region") }

type byValue struct{}

func (byValue) Fail(context.Context) error { return errors.New("by value") }

// rootStack is the stack of the root error of a received error's graph.
func rootStack(t *testing.T, err error) string {
	t.Helper()
	var remote *RemoteError
	if !errors.As(err, &remote) {
		t.Fatalf("not a received error: %#v", err)
	}
	var graph struct {
		Root  struct{ Value int }
		Nodes []struct{ Stack string }
	}
	if json.Unmarshal(remote.Graph, &graph) != nil || graph.Root.Value >= len(graph.Nodes) {
		t.Fatalf("no graph: %s", remote.Graph)
	}
	return graph.Nodes[graph.Root.Value].Stack
}

// guarantee: sdk/error-positions (an error a Go service method returns crosses
// with the method's declaration, file:line, as its stack's frame; a panic
// with its whole stack: #677)
func TestAServiceErrorCrossesWithItsMethodsPosition(t *testing.T) {
	plugin := Define(Plugin[NoConfig]{
		Name: "locating",
		Apply: func(c *Ctx, _ NoConfig) error {
			c.Provide("locating", &locating{})
			c.Provide("byvalue", &byValue{})
			return nil
		},
	})
	h := startFakeHost(t, nil, plugin)
	if err := h.load("locating", "locating"); err != nil {
		t.Fatal(err)
	}

	stack := rootStack(t, h.service("locating", "fail"))
	frame := regexp.MustCompile(`^Error: no forecast\n    at locating\.Fail \((.+):(\d+)\)$`)
	match := frame.FindStringSubmatch(stack)
	if match == nil || !strings.HasSuffix(match[1], "errors_location_test.go") || match[2] != "17" {
		t.Fatalf("stack: %q", stack)
	}

	// A method of T offered through *T: T's own declaration.
	stack = rootStack(t, h.service("byvalue", "fail"))
	if !regexp.MustCompile(`^Error: by value\n    at byValue\.Fail \(.+errors_location_test\.go:23\)$`).MatchString(stack) {
		t.Fatalf("stack: %q", stack)
	}

	// A panic keeps its whole stack.
	stack = rootStack(t, h.service("locating", "panic"))
	if !strings.HasPrefix(stack, "Panic: panic: bad region\n") || !strings.Contains(stack, "goroutine") || strings.Contains(stack, "    at locating.Panic") {
		t.Fatalf("stack: %q", stack)
	}
}

type relaying struct{ received *RemoteError }

func (r *relaying) Relay(context.Context) error { return r.received }

// receivedGraph is a received error's graph whose root carries rethrown.
func receivedGraph(rethrown []string) json.RawMessage {
	items, _ := json.Marshal(rethrown)
	return json.RawMessage(`{"root":{"type":"reference","value":0},"nodes":[` +
		`{"type":"error","name":"Fail","message":"far","stack":"Fail: far\n    at far.js:3:9","code":"Gone","data":{"type":"data","value":{"id":7}},"rethrown":` + string(items) + `}]}`)
}

// rethrownGraph is the root node of a received error's graph, without its
// rethrown, and that rethrown.
func rethrownGraph(t *testing.T, err error) (map[string]any, []string) {
	t.Helper()
	var remote *RemoteError
	if !errors.As(err, &remote) {
		t.Fatalf("not a received error: %#v", err)
	}
	var graph struct {
		Root  struct{ Value int }
		Nodes []map[string]any
	}
	if json.Unmarshal(remote.Graph, &graph) != nil || graph.Root.Value >= len(graph.Nodes) {
		t.Fatalf("no graph: %s", remote.Graph)
	}
	root := graph.Nodes[graph.Root.Value]
	var rethrown []string
	if items, ok := root["rethrown"].([]any); ok {
		for _, item := range items {
			rethrown = append(rethrown, item.(string))
		}
	}
	delete(root, "rethrown")
	return root, rethrown
}

// guarantee: sdk/error-rethrown (a received error a Go service method returns
// crosses with the graph it came with; the method's declaration is
// appended to its root node's rethrown, at most 8 items: #677)
func TestAReceivedErrorThrownOnRecordsTheMethodInRethrown(t *testing.T) {
	received := &RemoteError{Name: "Fail", Message: "far", Graph: receivedGraph([]string{"    at first (a.js:1:1)"})}
	full := make([]string, 8)
	for i := range full {
		full[i] = "    at caller (c.js:1:1)"
	}
	capped := &RemoteError{Name: "Fail", Message: "far", Graph: receivedGraph(full)}
	plugin := Define(Plugin[NoConfig]{
		Name: "relaying",
		Apply: func(c *Ctx, _ NoConfig) error {
			c.Provide("relaying", &relaying{received})
			c.Provide("capped", &relaying{capped})
			return nil
		},
	})
	h := startFakeHost(t, nil, plugin)
	if err := h.load("relaying", "relaying"); err != nil {
		t.Fatal(err)
	}
	stored := string(received.Graph)

	// Twice: each crossing appends once; the stored graph is not changed.
	for range 2 {
		err := h.service("relaying", "relay")
		var remote *RemoteError
		if !errors.As(err, &remote) || remote.Name != "Fail" || remote.Message != "far" || remote.Code != "Gone" {
			t.Fatalf("received: %#v", err)
		}
		root, rethrown := rethrownGraph(t, err)
		if root["stack"] != "Fail: far\n    at far.js:3:9" || root["code"] != "Gone" || root["name"] != "Fail" {
			t.Fatalf("root: %v", root)
		}
		if data, _ := json.Marshal(remote.Data); string(data) != `{"id":7}` {
			t.Fatalf("data: %s", data)
		}
		if len(rethrown) != 2 || rethrown[0] != "    at first (a.js:1:1)" ||
			!regexp.MustCompile(`^    at relaying\.Relay \(.+errors_location_test\.go:\d+\)$`).MatchString(rethrown[1]) {
			t.Fatalf("rethrown: %q", rethrown)
		}
	}
	if string(received.Graph) != stored {
		t.Fatalf("the stored graph changed: %s", received.Graph)
	}

	// At most 8 items: no more are appended.
	_, rethrown := rethrownGraph(t, h.service("capped", "relay"))
	if len(rethrown) != 8 || rethrown[7] != "    at caller (c.js:1:1)" {
		t.Fatalf("rethrown: %q", rethrown)
	}
}
