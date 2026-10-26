package peer

import (
	"context"
	"errors"
	"strings"
	"testing"
)

type definition struct{ name string }

// A plugin definition crosses as a `plugin` reference only to a far end
// that declared `plugins`; elsewhere only that call fails
// (docs/design-plugin-values-2026-10-25.md §5).
func TestAPluginDefinitionNeedsTheFarEndToTakeIt(t *testing.T) {
	a, _ := endpointPair(t, nil, echo)
	_, err := a.Call(context.Background(), "x", "echo", []any{Plugin{Value: &definition{"part"}}}, true)
	if err == nil || !strings.Contains(err.Error(), "did not declare the capability plugins") {
		t.Fatalf("%v", err)
	}
	var refused *ValueError
	if !errors.As(err, &refused) {
		t.Fatalf("%T", err)
	}
	if value, err := a.Call(context.Background(), "x", "echo", []any{"still"}, true); err != nil || text(value) != "still" {
		t.Fatalf("%v %v", value, err)
	}
}

// Sent to a far end that takes it, it comes back home as the same value;
// the same definition sent twice is one reference.
func TestAPluginDefinitionComesBackHomeAsItself(t *testing.T) {
	var received []*RemotePlugin
	var home any
	a, b := endpointPair(t, []string{"plugins"}, func(ctx context.Context, target, method string, args []any) (any, error) {
		switch method {
		case "keep":
			for _, arg := range args {
				received = append(received, arg.(*RemotePlugin))
			}
		case "home":
			home = args[0]
		}
		return nil, nil
	})
	b.opts.Plugins = true
	def := &definition{"part"}
	if _, err := a.Call(context.Background(), "x", "keep", []any{Plugin{Value: def}, Plugin{Value: def}}, true); err != nil {
		t.Fatal(err)
	}
	if len(received) != 2 || received[0] != received[1] {
		t.Fatalf("%v", received)
	}
	// b hands it back: a has its own value.
	if _, err := b.Call(context.Background(), "x", "home", []any{received[0]}, true); err != nil {
		t.Fatal(err)
	}
	if home != def {
		t.Fatalf("%v", home)
	}
}

// A side that does not take plugin definitions refuses one from the far
// end, as it refuses objects.
func TestAPluginReferenceFromTheFarEndIsRefusedByDefault(t *testing.T) {
	a, _ := endpointPair(t, []string{"plugins"}, echo)
	if _, err := a.Call(context.Background(), "x", "echo", []any{Plugin{Value: &definition{"part"}}}, true); err == nil {
		t.Fatal("taken")
	}
}
