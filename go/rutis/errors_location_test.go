package rutis_test

import (
	"context"
	"encoding/json"
	"errors"
	"regexp"
	"strings"
	"testing"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/rutistest"
)

type locating struct{}

func (*locating) Fail(context.Context) error { return errors.New("no forecast") }

func (*locating) Panic(context.Context) error { panic("bad region") }

type byValue struct{}

func (byValue) Fail(context.Context) error { return errors.New("by value") }

// rootStack is the stack of the root error of a received error's graph.
func rootStack(t *testing.T, err error) string {
	t.Helper()
	var remote *rutis.RemoteError
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

// guarantee: errors/location (an error a Go service method returns crosses
// with the method's declaration, file:line, as its stack's frame; a panic
// with its whole stack: #677)
func TestAServiceErrorCrossesWithItsMethodsPosition(t *testing.T) {
	plugin := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name: "locating",
		Apply: func(c *rutis.Ctx, _ rutis.NoConfig) error {
			c.Provide("locating", &locating{})
			c.Provide("byvalue", &byValue{})
			return nil
		},
	})
	loaded := rutistest.Load(t, plugin, nil, nil)
	ctx := context.Background()

	stack := rootStack(t, loaded.Service("locating").Call(ctx, "fail", nil))
	frame := regexp.MustCompile(`^Error: no forecast\n    at locating\.Fail \((.+):(\d+)\)$`)
	match := frame.FindStringSubmatch(stack)
	if match == nil || !strings.HasSuffix(match[1], "errors_location_test.go") || match[2] != "17" {
		t.Fatalf("stack: %q", stack)
	}

	// A method of T offered through *T: T's own declaration.
	stack = rootStack(t, loaded.Service("byvalue").Call(ctx, "fail", nil))
	if !regexp.MustCompile(`^Error: by value\n    at byValue\.Fail \(.+errors_location_test\.go:23\)$`).MatchString(stack) {
		t.Fatalf("stack: %q", stack)
	}

	// A panic keeps its whole stack.
	stack = rootStack(t, loaded.Service("locating").Call(ctx, "panic", nil))
	if !strings.HasPrefix(stack, "Panic: panic: bad region\n") || !strings.Contains(stack, "goroutine") || strings.Contains(stack, "    at locating.Panic") {
		t.Fatalf("stack: %q", stack)
	}
}
