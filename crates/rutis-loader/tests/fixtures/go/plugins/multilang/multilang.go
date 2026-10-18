// Package multilang holds the Go plugins of the runtime conformance suite
// (crates/rutis-loader/tests/multilang_go.rs): the same plugins as the
// Python and JavaScript ones, using and providing services across the
// three runtimes.
package multilang

import (
	"context"
	"strings"

	"github.com/arcships/rutis/go/rutis"
)

type Probe struct {
	Record func(ctx context.Context, line string) error
}

// Weather is the `go_weather` service.
type Weather struct{}

func (Weather) Today() string                    { return "go sunny" }
func (Weather) Later(ctx context.Context) string { return "go later" }

// Each calls back into the caller during a synchronous call.
func (Weather) Each(ctx context.Context, days []string, callback func(ctx context.Context, day string) (string, error)) ([]string, error) {
	out := make([]string, 0, len(days))
	for _, day := range days {
		value, err := callback(ctx, day)
		if err != nil {
			return nil, err
		}
		out = append(out, value)
	}
	return out, nil
}

func probe(ctx *rutis.Ctx) (Probe, error) {
	var p Probe
	err := ctx.Use("probe", &p)
	return p, err
}

func bye(ctx *rutis.Ctx, p Probe, line string) {
	ctx.Effect(func(c context.Context) error { return p.Record(c, line) })
}

var Provider = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:     "go_provider",
	Inject:   []string{"probe"},
	Provides: rutis.Provides{"go_weather": rutis.MethodsOf[*Weather](rutis.Sync("Today", "Each"))},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		p, err := probe(ctx)
		if err != nil {
			return err
		}
		ctx.Provide("go_weather", &Weather{})
		bye(ctx, p, "go provider: bye")
		return p.Record(ctx, "go provider: start")
	},
})

// Remote is another language's weather service.
type Remote struct {
	Today func(ctx context.Context) (string, error)
	Later func(ctx context.Context) (string, error)
}

var Consumer = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:   "go_consumer",
	Inject: []string{"js_weather", "py_weather", "probe"},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		p, err := probe(ctx)
		if err != nil {
			return err
		}
		var lines []string
		for _, name := range []string{"js_weather", "py_weather"} {
			var weather Remote
			if err := ctx.Use(name, &weather); err != nil {
				return err
			}
			today, err := weather.Today(ctx)
			if err != nil {
				return err
			}
			later, err := weather.Later(ctx)
			if err != nil {
				return err
			}
			lines = append(lines, today+" / "+later)
		}
		bye(ctx, p, "go consumer: bye")
		return p.Record(ctx, "go consumer: "+strings.Join(lines, " / "))
	},
})

var Local = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:   "go_local",
	Inject: []string{"go_weather", "probe"},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		p, err := probe(ctx)
		if err != nil {
			return err
		}
		var weather *rutis.Service
		if err := ctx.Use("go_weather", &weather); err != nil {
			return err
		}
		kind := "proxy"
		if weather.Local() {
			kind = "native"
		}
		bye(ctx, p, "go local: bye")
		return p.Record(ctx, "go local: "+kind)
	},
})

var Gated = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:   "go_gated",
	Inject: []string{"llm", "probe"},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		p, err := probe(ctx)
		if err != nil {
			return err
		}
		var llm struct {
			Ask func(ctx context.Context) (string, error)
		}
		if err := ctx.Use("llm", &llm); err != nil {
			return err
		}
		answer, err := llm.Ask(ctx)
		if err != nil {
			return err
		}
		bye(ctx, p, "go gated: bye")
		return p.Record(ctx, "go gated: "+answer)
	},
})

// Timeline is an instance's `go_timeline`.
type Timeline struct{ title string }

func (t *Timeline) Title(ctx context.Context) string { return "go " + t.title }

// Row uses its instance's `tools` and provides its instance's `go_timeline`.
var Row = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:     "go_row",
	Inject:   []string{"tools", "probe"},
	Provides: rutis.Provides{"go_timeline": rutis.MethodsOf[*Timeline]()},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		p, err := probe(ctx)
		if err != nil {
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
		if err := p.Record(ctx, "go sees "+title); err != nil {
			return err
		}
		ctx.Provide("go_timeline", &Timeline{title: title})
		return nil
	},
})

// RegistrantConfig names what a registrant registers.
type RegistrantConfig struct {
	Name string `json:"name"`
}

// Registrant registers its config's name with the host's `ledger`,
// which sees the row that calls it (crates/rutis-loader/tests/caller_rows.rs).
var Registrant = rutis.Define(rutis.Plugin[RegistrantConfig]{
	Name:   "go_registrant",
	Inject: []string{"ledger"},
	Apply: func(ctx *rutis.Ctx, config RegistrantConfig) error {
		var ledger struct {
			Register func(ctx context.Context, name string) error
		}
		if err := ctx.Use("ledger", &ledger); err != nil {
			return err
		}
		return ledger.Register(ctx, config.Name)
	},
})
