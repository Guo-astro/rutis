// Package values holds the Go plugins of the plugin-value cases
// (crates/rutis-loader/tests/plugin_values.rs): the same as the JavaScript
// and Python ones there. A row hands a plugin definition to a Rust service
// in Apply; rutis mounts copies of it, which run here.
package values

import (
	"context"
	"fmt"
	"os"
	"time"

	"github.com/arcships/rutis/go/rutis"
)

type probe struct {
	Record func(ctx context.Context, line string) error
}

type partConfig struct {
	N string `json:"n"`
}

// part is a session part using what the origin's Apply prepared.
func part(tag, prepared string) *rutis.Definition {
	return rutis.Define(rutis.Plugin[partConfig]{
		Inject: []string{"tools", "probe"},
		Apply: func(ctx *rutis.Ctx, config partConfig) error {
			var p probe
			if err := ctx.Use("probe", &p); err != nil {
				return err
			}
			var tools struct {
				Title func(ctx context.Context) (string, error)
			}
			if err := ctx.Use("tools", &tools); err != nil {
				return err
			}
			title, err := tools.Title(ctx)
			if err != nil {
				return err
			}
			ctx.Effect(func(c context.Context) error { return p.Record(c, fmt.Sprintf("%s %s down", tag, config.N)) })
			return p.Record(ctx, fmt.Sprintf("%s %s sees %s with %s", tag, config.N, title, prepared))
		},
	})
}

// Maker is the `maker` service: Make returns another part.
type Maker struct{ prepared string }

func (m *Maker) Make(ctx context.Context) (*rutis.Definition, error) {
	return part("made", m.prepared), nil
}

// Origin hands a part to `parts`, tries to hand one to a data parameter,
// and provides `maker`.
var Origin = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:   "pv_origin",
	Inject: []string{"parts", "probe"},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		var p probe
		if err := ctx.Use("probe", &p); err != nil {
			return err
		}
		var parts struct {
			Register  func(ctx context.Context, scope string, def *rutis.Definition) error
			TakesData func(ctx context.Context, value *rutis.Definition) (string, error)
		}
		if err := ctx.Use("parts", &parts); err != nil {
			return err
		}
		prepared := "prepared in apply"
		if err := parts.Register(ctx, "session", part("part", prepared)); err != nil {
			return err
		}
		if _, err := parts.TakesData(ctx, part("data", prepared)); err != nil {
			if err := p.Record(ctx, "data refused: "+err.Error()); err != nil {
				return err
			}
		} else if err := p.Record(ctx, "data took a definition"); err != nil {
			return err
		}
		ctx.Provide("maker", &Maker{prepared})
		ctx.Effect(func(c context.Context) error { return p.Record(c, "origin down") })
		return nil
	},
})

// Crash ends its runtime's process soon after it loads.
var Crash = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name: "pv_crash",
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		go func() {
			time.Sleep(200 * time.Millisecond)
			os.Exit(17)
		}()
		return nil
	},
})
