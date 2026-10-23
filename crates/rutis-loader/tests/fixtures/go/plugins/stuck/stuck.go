// Package stuck provides the `stuck` service, whose plugin never finishes
// its cleanup: a runtime serving it does not stop by itself.
package stuck

import (
	"context"
	"os"
	"time"

	"github.com/arcships/rutis/go/rutis"
)

type Stuck struct{}

// Pid is the runtime's process id: which process serves the call.
func (Stuck) Pid() int { return os.Getpid() }

// Plugin is the `stuck` plugin.
var Plugin = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:     "stuck",
	Provides: rutis.Provides{"stuck": rutis.MethodsOf[Stuck](rutis.Sync("Pid"))},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		ctx.Provide("stuck", Stuck{})
		// Ignores its context: only killing the process ends it.
		ctx.Effect(func(context.Context) error {
			time.Sleep(time.Hour)
			return nil
		})
		return nil
	},
})
