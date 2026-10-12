// A plugin that lingers, alone in a binary: it starts work nothing stops,
// and its cleanup writes a marker and, when told to, never returns
// (crates/rutis-bridge/tests/host_gone.rs).
package main

import (
	"context"
	"os"
	"time"

	"github.com/arcships/rutis/go/rutis"
)

// Config says where the cleanup writes its marker, and whether it then
// hangs.
type Config struct {
	Marker string `json:"marker"`
	Hang   bool   `json:"hang"`
}

var plugin = rutis.Define(rutis.Plugin[Config]{
	Name: "lingering",
	Apply: func(ctx *rutis.Ctx, config Config) error {
		// Work nothing stops, like a timer nobody clears.
		go func() {
			for {
				time.Sleep(time.Hour)
			}
		}()
		ctx.Effect(func(context.Context) error {
			if err := os.WriteFile(config.Marker, []byte("cleaned up"), 0o644); err != nil {
				return err
			}
			if config.Hang {
				select {}
			}
			return nil
		})
		return nil
	},
})

func main() { rutis.Serve(plugin) }
