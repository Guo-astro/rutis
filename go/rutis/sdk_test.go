package rutis_test

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/advanced"
	"github.com/arcships/rutis/go/rutis/rutistest"
)

type Config struct {
	City string `json:"city,omitempty"`
}

type LLM struct {
	Ask func(ctx context.Context, question string) (string, error)
}

type Weather struct {
	llm  LLM
	city string
}

func (w *Weather) Today(ctx context.Context) (string, error) {
	answer, err := w.llm.Ask(ctx, "weather in "+w.city)
	if err != nil {
		return "", err
	}
	return answer + " in " + w.city, nil
}

func (w *Weather) Each(ctx context.Context, days []string, callback func(ctx context.Context, day string) (string, error)) ([]string, error) {
	var out []string
	for _, day := range days {
		value, err := callback(ctx, day)
		if err != nil {
			return nil, err
		}
		out = append(out, value)
	}
	return out, nil
}

func (w *Weather) Unit() string { return "celsius" }

var (
	cleanedUp atomic.Bool
	taskEnded atomic.Bool
)

var weather = rutis.Define(rutis.Plugin[Config]{
	Name:     "weather",
	Inject:   []string{"llm"},
	Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Unit"))},
	Apply: func(ctx *rutis.Ctx, config Config) error {
		var llm LLM
		if err := ctx.Use("llm", &llm); err != nil {
			return err
		}
		ctx.Provide("weather", &Weather{llm: llm, city: config.City})
		ctx.Effect(func(context.Context) error {
			cleanedUp.Store(true)
			return nil
		})
		ctx.Go(func(c context.Context) error {
			<-c.Done()
			taskEnded.Store(true)
			return c.Err()
		})
		return nil
	},
})

type fakeLLM struct{}

func (fakeLLM) Ask(question string) string {
	if !strings.HasPrefix(question, "weather in ") {
		return "?"
	}
	return "sunny"
}

func TestAPluginUsesAndProvidesServices(t *testing.T) {
	cleanedUp.Store(false)
	taskEnded.Store(false)
	loaded := rutistest.Load(t, weather, Config{City: "Oslo"}, map[string]any{"llm": fakeLLM{}})
	ctx := context.Background()
	service := loaded.Service("weather")
	if got := service.Methods(); got["today"] != "async" || got["unit"] != "sync" || got["each"] != "async" {
		t.Fatalf("shapes: %v", got)
	}
	var today string
	if err := service.Call(ctx, "today", &today); err != nil || today != "sunny in Oslo" {
		t.Fatalf("today: %q %v", today, err)
	}
	var days []string
	upper := func(_ context.Context, day string) (string, error) { return strings.ToUpper(day), nil }
	if err := service.Call(ctx, "each", &days, []string{"mon", "tue"}, upper); err != nil || fmt.Sprint(days) != "[MON TUE]" {
		t.Fatalf("each: %v %v", days, err)
	}
	var client struct {
		Unit func(ctx context.Context) (string, error)
	}
	if err := service.Bind(&client); err != nil {
		t.Fatal(err)
	}
	if unit, err := client.Unit(ctx); err != nil || unit != "celsius" {
		t.Fatalf("unit: %q %v", unit, err)
	}
	loaded.Unload()
	if !cleanedUp.Load() || !taskEnded.Load() {
		t.Fatalf("unloading runs the cleanups (%v) and ends the tasks (%v)", cleanedUp.Load(), taskEnded.Load())
	}
}

// fatal records a test failure and stops the loading.
type fatal struct {
	testing.TB
	message string
}

type stop struct{}

func (f *fatal) Fatalf(format string, args ...any) {
	f.message = fmt.Sprintf(format, args...)
	panic(stop{})
}
func (f *fatal) Fatal(args ...any) { f.Fatalf("%s", fmt.Sprint(args...)) }
func (f *fatal) Helper()           {}

func loadFails(t *testing.T, plugin *rutis.Definition, services map[string]any) string {
	t.Helper()
	recorder := &fatal{TB: t}
	func() {
		defer func() {
			if recovered := recover(); recovered != nil {
				if _, ok := recovered.(stop); !ok {
					panic(recovered)
				}
			}
		}()
		rutistest.Load(recorder, plugin, nil, services)
	}()
	if recorder.message == "" {
		t.Fatal("the load should fail")
	}
	return recorder.message
}

func TestAPluginMayUseOnlyWhatItInjects(t *testing.T) {
	sneaky := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name: "sneaky",
		Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
			var llm LLM
			return ctx.Use("llm", &llm)
		},
	})
	// guarantee: sdk/undeclared-use
	want := `service llm is not available to this plugin, which does not declare it in Inject: add "llm" to its Inject`
	if message := loadFails(t, sneaky, map[string]any{"llm": fakeLLM{}}); !strings.Contains(message, want) {
		t.Fatal(message)
	}
}

func TestAProvidedServiceHasItsDeclaredType(t *testing.T) {
	wrong := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name:     "wrong",
		Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather]()},
		Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
			ctx.Provide("weather", fakeLLM{})
			return nil
		},
	})
	if message := loadFails(t, wrong, nil); !strings.Contains(message, "is not the *rutis_test.Weather Provides declares") {
		t.Fatal(message)
	}
}

func TestABindingMatchesDeclaredMethods(t *testing.T) {
	strict := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name:   "strict",
		Inject: []string{"llm"},
		Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
			var llm struct {
				Forecast func(ctx context.Context) (string, error)
			}
			return ctx.Use("llm", &llm)
		},
	})
	if message := loadFails(t, strict, map[string]any{"llm": fakeLLM{}}); !strings.Contains(message, "calls forecast, which the service does not declare") {
		t.Fatal(message)
	}
}

func TestTheManifestDescribesEveryPlugin(t *testing.T) {
	data, err := advanced.Manifest(weather)
	if err != nil {
		t.Fatal(err)
	}
	var manifest struct {
		Manifest  int                       `json:"manifest"`
		SDK       string                    `json:"sdk"`
		PluginAPI int                       `json:"pluginApi"`
		Runtime   string                    `json:"runtime"`
		Plugins   map[string]map[string]any `json:"plugins"`
	}
	if err := json.Unmarshal(data, &manifest); err != nil {
		t.Fatal(err)
	}
	if manifest.Manifest != 1 || manifest.SDK != advanced.Version || manifest.PluginAPI != advanced.PluginAPI || manifest.Runtime != "rutis-go-runtime:1" {
		t.Fatalf("%s", data)
	}
	plugin := manifest.Plugins["weather"]
	if fmt.Sprint(plugin["inject"]) != "[llm]" || plugin["config"] == nil {
		t.Fatalf("%s", data)
	}
}

func TestTwoPluginsMayNotShareAName(t *testing.T) {
	err := advanced.ServeArgs([]string{"--rutis-manifest"}, weather, weather)
	if err == nil || !strings.Contains(err.Error(), "two plugins are named weather") {
		t.Fatal(err)
	}
}

func TestDefineRefusesBrokenDeclarations(t *testing.T) {
	defer func() {
		if recovered := recover(); recovered == nil || !strings.Contains(fmt.Sprint(recovered), `Sync("Tomorrow")`) {
			t.Fatalf("%v", recovered)
		}
	}()
	rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name:     "broken",
		Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Tomorrow"))},
		Apply:    func(*rutis.Ctx, rutis.NoConfig) error { return nil },
	})
}

// A service Provides does not declare reaches rutis all the same (#442):
// its methods are the value's exported ones, all asynchronous, and the
// manifest says the plugin may provide services it does not declare.
func TestAnUndeclaredServiceIsAnnounced(t *testing.T) {
	undeclared := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name: "undeclared",
		Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
			ctx.Provide("weather", &Weather{})
			return nil
		},
	})
	loaded := rutistest.Load(t, undeclared, nil, nil)
	weather := loaded.Service("weather")
	if kinds := weather.Methods(); len(kinds) != 3 || kinds["unit"] != "async" || kinds["today"] != "async" {
		t.Fatalf("methods %v", kinds)
	}
	var unit string
	if err := weather.Call(context.Background(), "unit", &unit); err != nil || unit != "celsius" {
		t.Fatal(unit, err)
	}
	data, err := advanced.Manifest(undeclared)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(data), `"announces":true`) {
		t.Fatalf("the manifest says it announces: %s", data)
	}
}

// summary uses `llm` when the host has it as the row starts: its service
// `summary` answers with what `llm` says, or the text cut short.
var summary = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:     "summary",
	Inject:   []string{"notes"},
	Optional: []string{"llm"},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		var llm LLM
		ok, err := ctx.Get("llm", &llm)
		if err != nil {
			return err
		}
		used := ctx.Use("llm", &LLM{})
		_, undeclared := ctx.Get("search", &struct{}{})
		ctx.Provide("summary", &summarizer{llm: llm, ok: ok, used: used, undeclared: undeclared})
		return nil
	},
})

type summarizer struct {
	llm              LLM
	ok               bool
	used, undeclared error
}

func (s *summarizer) Of(ctx context.Context, text string) (string, error) {
	if !s.ok {
		return text[:min(len(text), 5)], nil
	}
	return s.llm.Ask(ctx, "summarize: "+text)
}

// Errors are what Use and Get said in Apply, as text.
func (s *summarizer) Errors() []string {
	text := func(err error) string {
		if err == nil {
			return ""
		}
		return err.Error()
	}
	return []string{text(s.used), text(s.undeclared)}
}

type echoLLM struct{}

func (echoLLM) Ask(question string) string { return "re: " + question }

// guarantee: sdk/optional-deps (rutistest leases an optional service when
// the test gives one and loads without it otherwise; Get then reports it
// absent; Use reads only what Inject lists, as Rust's `require`, so it says
// to add an Optional one there; Get of a name declared nowhere says to add
// it to Optional)
func TestAnOptionalServiceIsUsedWhenGiven(t *testing.T) {
	ctx := context.Background()
	var of string
	var errs []string

	with := rutistest.Load(t, summary, nil, map[string]any{"notes": fakeLLM{}, "llm": echoLLM{}})
	if err := with.Service("summary").Call(ctx, "of", &of, "a long text"); err != nil || of != "re: summarize: a long text" {
		t.Fatalf("with llm: %q %v", of, err)
	}
	notInject := `service llm is not available to this plugin, which does not declare it in Inject: add "llm" to its Inject`
	if err := with.Service("summary").Call(ctx, "errors", &errs); err != nil || errs[0] != notInject {
		t.Fatalf("use with llm: %q %v", errs, err)
	}
	with.Unload()

	without := rutistest.Load(t, summary, nil, map[string]any{"notes": fakeLLM{}})
	if err := without.Service("summary").Call(ctx, "of", &of, "a long text"); err != nil || of != "a lon" {
		t.Fatalf("without llm: %q %v", of, err)
	}
	if err := without.Service("summary").Call(ctx, "errors", &errs); err != nil {
		t.Fatal(err)
	}
	want := []string{
		notInject,
		`service search is not available to this plugin, which does not declare it: add "search" to its Optional (or to its Inject, if it cannot run without it)`,
	}
	if fmt.Sprintf("%q", errs) != fmt.Sprintf("%q", want) {
		t.Fatalf("errors %q", errs)
	}
}

// guarantee: sdk/optional-deps (rutistest refuses a name in both Inject and
// Optional before loading, with the host's message)
func TestANameIsNotBothRequiredAndOptional(t *testing.T) {
	both := rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name:     "both",
		Inject:   []string{"llm"},
		Optional: []string{"llm"},
		Apply:    func(*rutis.Ctx, rutis.NoConfig) error { return nil },
	})
	// The host refuses the row, saying why.
	if message := loadFails(t, both, map[string]any{"llm": fakeLLM{}}); !strings.HasSuffix(message, ": llm is in both inject and optional; keep it in one") {
		t.Fatal(message)
	}
}

// guarantee: sdk/optional-deps (the manifest reports optional, [] when the
// plugin declares none)
func TestTheManifestReportsOptionalServices(t *testing.T) {
	data, err := advanced.Manifest(weather, summary)
	if err != nil {
		t.Fatal(err)
	}
	var manifest struct {
		Plugins map[string]map[string]json.RawMessage `json:"plugins"`
	}
	if err := json.Unmarshal(data, &manifest); err != nil {
		t.Fatal(err)
	}
	if got := string(manifest.Plugins["weather"]["optional"]); got != "[]" {
		t.Fatalf("weather: %s", got)
	}
	if got := string(manifest.Plugins["summary"]["optional"]); got != `["llm"]` {
		t.Fatalf("summary: %s", got)
	}
}
