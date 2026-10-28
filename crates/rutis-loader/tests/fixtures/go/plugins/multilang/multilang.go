// Package multilang holds the Go plugins of the runtime conformance suite
// (crates/rutis-loader/tests/multilang_go.rs): the same plugins as the
// Python and JavaScript ones, using and providing services across the
// three runtimes.
package multilang

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/advanced"
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
		var weather *advanced.Service
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
			Add    func(ctx context.Context, entry map[string]any) (int, error)
			OnDay  func(ctx context.Context, day int) (map[string]any, error)
			Caller func(ctx context.Context) (string, error)
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
		caller, err := calendar.Caller(ctx)
		if err != nil {
			return err
		}
		if err := p.Record(ctx, "go caller: "+caller); err != nil {
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

// Taker is the `go_taker` service of the bad-value cases
// (crates/rutis-loader/tests/bad_values.rs, #675).
type Taker struct{}

// Take says what it received: a value that cannot reach Go fails before.
func (Taker) Take(ctx context.Context, value any) string { return fmt.Sprintf("%T", value) }

// Relay calls the caller back and fails as the callback failed: it throws
// on what it caught, whatever was thrown (`return err`).
func (Taker) Relay(ctx context.Context, callback func(ctx context.Context) (any, error)) (any, error) {
	return callback(ctx)
}

func (Taker) Ping() string { return "go ok" }

// GoTaker provides `go_taker`.
var GoTaker = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:     "go_taker",
	Provides: rutis.Provides{"go_taker": rutis.MethodsOf[*Taker](rutis.Sync("Take", "Relay", "Ping"))},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		ctx.Provide("go_taker", &Taker{})
		return nil
	},
})

// MakerConfig says where the Python rows' modules are.
type MakerConfig struct {
	Dir string `json:"dir"`
}

// Maker runs the scenario of crates/rutis-loader/tests/loader_service.rs
// through the host's `loader` (#185), recording what each call returned or
// threw: once started, as the calls wait for the loader's operation that
// starts it.
var Maker = rutis.Define(rutis.Plugin[MakerConfig]{
	Name:   "go_maker",
	Inject: []string{"loader", "probe"},
	Apply: func(ctx *rutis.Ctx, config MakerConfig) error {
		p, err := probe(ctx)
		if err != nil {
			return err
		}
		var loader struct {
			Create func(ctx context.Context, options map[string]any) (string, error)
			Reload func(ctx context.Context, id string) error
			Status func(ctx context.Context) (map[string]any, error)
			Remove func(ctx context.Context, id string) error
		}
		if err := ctx.Use("loader", &loader); err != nil {
			return err
		}
		record := func(label string, value string, err error) {
			var remote *rutis.RemoteError
			switch {
			case err == nil:
				_ = p.Record(ctx, strings.TrimSpace(label+": ok "+value))
			case errors.As(err, &remote):
				row := ""
				if data, ok := remote.Data.(map[string]any); ok {
					row, _ = data["row"].(string)
				}
				_ = p.Record(ctx, fmt.Sprintf("%s: %s %s %s | %s", label, remote.Name, remote.Code, row, remote.Message))
			default:
				_ = p.Record(ctx, label+": "+err.Error())
			}
		}
		write := func(name, text string) { _ = os.WriteFile(filepath.Join(config.Dir, name), []byte(text), 0o644) }
		create := func(label string, options map[string]any) {
			id, err := loader.Create(ctx, options)
			record(label, id, err)
		}
		row := func(name string) map[string]any { return map[string]any{"id": name, "name": "py:" + name} }
		states := func(label string) {
			status, err := loader.Status(ctx)
			if err != nil {
				record(label, "", err)
				return
			}
			var parts []string
			rows, _ := status["rows"].([]any)
			for _, item := range rows {
				if item, ok := item.(map[string]any); ok {
					parts = append(parts, fmt.Sprintf("%v %v", item["id"], item["state"]))
				}
			}
			_ = p.Record(ctx, label+": "+strings.Join(parts, ", "))
		}
		go func() {
			write("weather.py", makerV1)
			create("create", row("weather"))
			create("create again", row("weather"))
			create("create unknown", row("wether"))
			write("fails.py", makerFails)
			create("create fails", row("fails"))
			write("syntax.py", makerSyntax)
			create("create syntax", row("syntax"))
			write("strict.py", makerStrict)
			create("create strict", map[string]any{"id": "strict", "name": "py:strict", "config": map[string]any{"units": "kelvin"}})
			create("create no id", map[string]any{"name": "py:tiny"})
			write("weather.py", makerSyntax)
			record("reload syntax", "", loader.Reload(ctx, "weather"))
			write("weather.py", makerFails)
			record("reload fails", "", loader.Reload(ctx, "weather"))
			write("weather.py", makerV2)
			record("reload", "", loader.Reload(ctx, "weather"))
			record("reload unknown", "", loader.Reload(ctx, "wether"))
			states("status")
			record("remove", "", loader.Remove(ctx, "weather"))
			record("remove unknown", "", loader.Remove(ctx, "weather"))
			states("status after")
		}()
		return nil
	},
})

const (
	makerV1     = "inject = [\"probe\"]\n\n\ndef apply(ctx, config):\n    ctx.use(\"probe\").record(\"weather v1\")\n"
	makerSyntax = "inject = [\"probe\"]\n\ndef apply(ctx, config)\n    pass\n"
	makerV2     = "inject = [\"probe\"]\n\n\ndef apply(ctx, config):\n    ctx.use(\"probe\").record(\"weather version 2\")\n"
	makerFails  = "def apply(ctx, config):\n    raise KeyError(\"units\")\n"
	makerStrict = "from rutis import define_plugin\n\n\ndef apply(ctx, config):\n    pass\n\n\nplugin = define_plugin(apply, config={\"type\": \"object\", \"properties\": {\"units\": {\"enum\": [\"metric\", \"imperial\"]}}})\n"
)
