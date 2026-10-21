// Package multilang holds the Go plugins of the runtime conformance suite
// (crates/rutis-loader/tests/multilang_go.rs): the same plugins as the
// Python and JavaScript ones, using and providing services across the
// three runtimes.
package multilang

import (
	"context"
	"errors"
	"fmt"
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

// Fail fails with an error that has a code and data (#440).
func (Weather) Fail(ctx context.Context) error { return stormy{} }

type stormy struct{}

func (stormy) Error() string { return "too windy" }
func (stormy) Name() string  { return "WeatherError" }
func (stormy) Code() string  { return "Stormy" }
func (stormy) Data() any     { return map[string]any{"wind": uint64(1) << 60} }

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

// CalendarUser calls the host's `calendar`, a Rust type marked
// #[rutis::service] (crates/rutis-loader/tests/marked_services.rs).
var CalendarUser = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:   "go_calendar",
	Inject: []string{"calendar", "probe"},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		p, err := probe(ctx)
		if err != nil {
			return err
		}
		var calendar struct {
			Add   func(ctx context.Context, entry map[string]any) (int, error)
			OnDay func(ctx context.Context, day int) (map[string]any, error)
		}
		if err := ctx.Use("calendar", &calendar); err != nil {
			return err
		}
		count, err := calendar.Add(ctx, map[string]any{"title": "go", "day": 4})
		if err != nil {
			return err
		}
		entry, err := calendar.OnDay(ctx, 4)
		if err != nil {
			return err
		}
		_, err = calendar.OnDay(ctx, 99)
		var remote *rutis.RemoteError
		if !errors.As(err, &remote) {
			return fmt.Errorf("expected a remote error, got %v", err)
		}
		return p.Record(ctx, fmt.Sprintf("go calendar: %t %v / %s %s %s", count >= 1, entry["title"], remote.Name, remote.Code, remote.Message))
	},
})
