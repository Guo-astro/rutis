package rutistest_test

import (
	"context"
	"errors"
	"testing"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/advanced"
	"github.com/arcships/rutis/go/rutis/rutistest"
)

type cache struct{ values map[string]int }

func (c *cache) Set(key string, value int) { c.values[key] = value }
func (c *cache) Get(key string) int        { return c.values[key] }

// warmer takes an object as a struct of function fields, or as any.
type warmer struct{ kept *advanced.Service }

func (w *warmer) Warm(ctx context.Context, c struct {
	Set func(ctx context.Context, key string, value int) error
	Get func(ctx context.Context, key string) (int, error)
}) (int, error) {
	if err := c.Set(ctx, "a", 1); err != nil {
		return 0, err
	}
	return c.Get(ctx, "a")
}

func (w *warmer) Keep(ctx context.Context, c *advanced.Service) { w.kept = c }
func (w *warmer) Bump(ctx context.Context, m map[string]int) int {
	m["n"] = 2
	return m["n"]
}
func (w *warmer) Back(ctx context.Context) *advanced.Service { return w.kept }
func (w *warmer) Missing(ctx context.Context) string {
	var remote *rutis.RemoteError
	if err := w.kept.Call(ctx, "gett", nil); errors.As(err, &remote) {
		return remote.Code + ": " + remote.Message
	}
	return "answered"
}

// objects-case(go): strict-testing-tool
// objects-case(go): data-stays-data
// objects-case(go): no-method
// The testing tool passes an object (rutis.Ref) across its session, as
// between processes (#687): the plugin calls the original, keeps it as a
// *advanced.Service, passes it back as the original; a method the object
// lacks is no-method; data is copied.
func TestAnObjectCrossesAsBetweenProcesses(t *testing.T) {
	plugin := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name: "warmer",
		Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
			ctx.Provide("warmer", &warmer{})
			return nil
		},
	})
	loaded := rutistest.Load(t, plugin, rutis.NoConfig{}, nil)
	ctx := context.Background()
	original := &cache{values: map[string]int{}}
	service := loaded.Service("warmer")
	var got int
	if err := service.Call(ctx, "warm", &got, rutis.Ref(original)); err != nil || got != 1 || original.values["a"] != 1 {
		t.Fatalf("%v %d %v", err, got, original.values)
	}
	if err := service.Call(ctx, "keep", nil, rutis.Ref(original)); err != nil {
		t.Fatal(err)
	}
	var back any
	if err := service.Call(ctx, "back", &back); err != nil || back != any(original) {
		t.Fatalf("%v %T %v", err, back, back)
	}
	var missing string
	if err := service.Call(ctx, "missing", &missing); err != nil || missing != "no-method: cache has no method gett (methods: get, set)" {
		t.Fatalf("%v %q", err, missing)
	}
	data := map[string]int{"n": 1}
	if err := service.Call(ctx, "bump", &got, data); err != nil || got != 2 || data["n"] != 1 {
		t.Fatalf("data is copied: %v %d %v", err, got, data)
	}
}
