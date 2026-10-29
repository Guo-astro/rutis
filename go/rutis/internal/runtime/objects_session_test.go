package rutis

import (
	"context"
	"errors"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// hostCalls calls a service the runtime published, from the host.
type hostCalls struct {
	h    *fakeHost
	name string
}

func (c *hostCalls) Call(method string, result any, args ...any) error {
	converted, err := convert.ToPeerArgs(args)
	if err != nil {
		return err
	}
	var handle string
	testwait.Eventually(c.h.t, "the service "+c.name, func() bool {
		c.h.mu.Lock()
		defer c.h.mu.Unlock()
		handle = c.h.handles[c.name]
		return handle != ""
	}, nil)
	ctx, cancel := context.WithTimeout(context.Background(), testwait.HangGuard)
	defer cancel()
	value, err := c.h.host.Call(ctx, handle, method, converted, false)
	if err == nil {
		value, err = peer.Settle(ctx, value)
	}
	if err != nil {
		return err
	}
	return convert.Decode(value, result)
}

type cache struct{ values map[string]int }

func (c *cache) Set(key string, value int) { c.values[key] = value }
func (c *cache) Get(key string) int        { return c.values[key] }

// warmer takes an object as a struct of function fields, or as any.
type warmer struct{ kept *Service }

func (w *warmer) Warm(ctx context.Context, c struct {
	Set func(ctx context.Context, key string, value int) error
	Get func(ctx context.Context, key string) (int, error)
}) (int, error) {
	if err := c.Set(ctx, "a", 1); err != nil {
		return 0, err
	}
	return c.Get(ctx, "a")
}

func (w *warmer) Keep(ctx context.Context, c *Service) { w.kept = c }
func (w *warmer) Bump(ctx context.Context, m map[string]int) int {
	m["n"] = 2
	return m["n"]
}
func (w *warmer) Back(ctx context.Context) *Service { return w.kept }
func (w *warmer) Missing(ctx context.Context) string {
	var remote *RemoteError
	if err := w.kept.Call(ctx, "gett", nil); errors.As(err, &remote) {
		return remote.Code + ": " + remote.Message
	}
	return "answered"
}

// objects-case(go): data-stays-data
// objects-case(go): no-method
// An object (Ref) crosses the session to the runtime (#687): the plugin
// calls the original, keeps it as a *advanced.Service, passes it back as
// the original; a method the object lacks is no-method; data is copied.
func TestAnObjectCrossesTheSession(t *testing.T) {
	plugin := Define(Plugin[NoConfig]{
		Name: "warmer",
		Apply: func(ctx *Ctx, _ NoConfig) error {
			ctx.Provide("warmer", &warmer{})
			return nil
		},
	})
	h := startFakeHost(t, nil, plugin)
	if err := h.load("warmer", "warmer"); err != nil {
		t.Fatal(err)
	}
	original := &cache{values: map[string]int{}}
	service := &hostCalls{h, "warmer"}
	var got int
	if err := service.Call("warm", &got, Ref(original)); err != nil || got != 1 || original.values["a"] != 1 {
		t.Fatalf("%v %d %v", err, got, original.values)
	}
	if err := service.Call("keep", nil, Ref(original)); err != nil {
		t.Fatal(err)
	}
	var back any
	if err := service.Call("back", &back); err != nil || back != any(original) {
		t.Fatalf("%v %T %v", err, back, back)
	}
	var missing string
	if err := service.Call("missing", &missing); err != nil || missing != "no-method: cache has no method gett (methods: get, set)" {
		t.Fatalf("%v %q", err, missing)
	}
	data := map[string]int{"n": 1}
	if err := service.Call("bump", &got, data); err != nil || got != 2 || data["n"] != 1 {
		t.Fatalf("data is copied: %v %d %v", err, got, data)
	}
}
