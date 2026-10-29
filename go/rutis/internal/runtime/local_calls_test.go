package rutis

import (
	"context"
	"strings"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/convert"
)

// caller takes functions where it takes any value: as one of another
// process arrives, a *advanced.Function.
type caller struct{}

func (caller) Call(ctx context.Context, fn any, text string) (string, error) {
	f, ok := fn.(*convert.Function)
	if !ok {
		return "", nil
	}
	var out string
	err := f.Call(ctx, &out, text)
	return out, err
}

func (caller) CallIn(ctx context.Context, args []any) (string, error) {
	var out string
	err := args[0].(*convert.Function).Call(ctx, &out, args[1])
	return out, err
}

// A row calling a service of another row of this process with a function
// argument: where the method takes any, the function arrives as a
// *advanced.Function (alone or inside a list), as from another process,
// and calling it calls the original.
func TestAFunctionArgumentOfACallWithinTheProcessIsAFunction(t *testing.T) {
	var user *Ctx
	provider := Define(Plugin[NoConfig]{Name: "provider", Apply: func(c *Ctx, _ NoConfig) error {
		c.Provide("caller", caller{})
		return nil
	}})
	using := Define(Plugin[NoConfig]{Name: "using", Inject: []string{"caller"}, Apply: func(c *Ctx, _ NoConfig) error {
		user = c
		return nil
	}})
	rt := newRuntime([]*Definition{provider, using})
	ctx := context.Background()
	t.Cleanup(func() { _ = rt.dispose(ctx) })
	if err := rt.load(ctx, "p", "provider", nil, nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	if err := rt.load(ctx, "u", "using", nil, nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	var service *Service
	if err := user.Use("caller", &service); err != nil {
		t.Fatal(err)
	}
	if !service.Local() {
		t.Fatal("the service is not this process's")
	}
	upper := func(_ context.Context, text string) (string, error) { return strings.ToUpper(text), nil }
	var got string
	if err := service.Call(ctx, "call", &got, upper, "x"); err != nil || got != "X" {
		t.Fatalf("a function argument: %q %v", got, err)
	}
	if err := service.Call(ctx, "callIn", &got, []any{upper, "y"}); err != nil || got != "Y" {
		t.Fatalf("a function in a list: %q %v", got, err)
	}
}
