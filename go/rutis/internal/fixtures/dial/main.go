// The dial conformance plugin (crates/rutis-bridge/tests/dial_conformance.rs)
// for the Go runtime: it says on stderr when its row loads and unloads,
// with its config, so the suite sees each session's rows come and go.
package main

import (
	"context"
	"fmt"
	"os"

	"github.com/arcships/rutis/go/rutis"
)

var marker = rutis.Define(rutis.Plugin[string]{
	Name: "marker",
	Apply: func(ctx *rutis.Ctx, config string) error {
		fmt.Fprintf(os.Stderr, "conformance: loaded %s\n", config)
		ctx.Effect(func(context.Context) error {
			fmt.Fprintf(os.Stderr, "conformance: unloaded %s\n", config)
			return nil
		})
		return nil
	},
})

func main() { rutis.Serve(marker) }
