// Package relay holds the Go plugin of the wait-cycle suite
// (crates/rutis-loader/tests/sync_cycles.rs): a synchronous call through
// Go from one Node runtime to another.
package relay

import (
	"context"
	"errors"

	"github.com/arcships/rutis/go/rutis"
)

type Work struct {
	Work func(ctx context.Context) (string, error)
}

// Relay is the `g_relay` service.
type Relay struct{ work Work }

// Work calls `b_work` synchronously, on the caller's chain.
func (r *Relay) Work(ctx context.Context) (string, error) {
	value, err := r.work.Work(ctx)
	if errors.Is(err, rutis.ErrSyncWaitCycle) {
		return "g_relay: SyncWaitCycle", nil
	}
	if err != nil {
		return "", err
	}
	return "g_relay relays " + value, nil
}

var Plugin = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:     "go_relay",
	Inject:   []string{"b_work"},
	Provides: rutis.Provides{"g_relay": rutis.MethodsOf[*Relay](rutis.Sync("Work"))},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		var work Work
		if err := ctx.Use("b_work", &work); err != nil {
			return err
		}
		ctx.Provide("g_relay", &Relay{work: work})
		return nil
	},
})
