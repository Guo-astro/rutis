package rutis

import (
	"bytes"
	"context"
	"testing"
)

type oldHostService struct{}

func (oldHostService) Get() int { return 1 }

// Under a host older than 0.12 (its mount has no `announce`), a plugin
// providing a service Provides does not declare is told, once per name; a
// declared or keyed one, or under a host that takes them, nothing.
func TestAnUndeclaredServiceUnderAnOlderHostIsToldOncePerName(t *testing.T) {
	plugin := Define(Plugin[NoConfig]{
		Name:     "old-host",
		Provides: Provides{"declared": MethodsOf[oldHostService]()},
		Apply: func(ctx *Ctx, _ NoConfig) error {
			ctx.Provide("declared", oldHostService{})
			ctx.Provide("weather", oldHostService{})
			ctx.Provide("weather#1", oldHostService{})
			return nil
		},
	})
	told := func(mount map[string]any) string {
		var written bytes.Buffer
		saved := oldHostOutput
		oldHostOutput = &written
		defer func() { oldHostOutput = saved }()
		rt := newRuntime([]*Definition{plugin})
		ctx := context.Background()
		if _, err := rt.control(ctx, "mount", []any{mount}); err != nil {
			t.Fatal(err)
		}
		exports := map[string]map[string]string{"declared": {"Get": "sync"}}
		for _, key := range []string{"a", "b"} {
			if err := rt.load(ctx, key, "old-host", nil, exports, nil, nil); err != nil {
				t.Fatal(err)
			}
			if err := rt.unload(ctx, key); err != nil {
				t.Fatal(err)
			}
		}
		return written.String()
	}
	want := "rutis: service weather is not declared in provides, and this host (older than 0.12) does not take undeclared services; declare it or upgrade rutis-host\n"
	if got := told(map[string]any{}); got != want {
		t.Fatalf("an older host: %q", got)
	}
	if got := told(map[string]any{"announce": true}); got != "" {
		t.Fatalf("a host taking them: %q", got)
	}
}
