package rutis_test

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/arcships/rutis/go/rutis"
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
	if message := loadFails(t, sneaky, map[string]any{"llm": fakeLLM{}}); !strings.Contains(message, "without declaring it in Inject") {
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
	data, err := rutis.Manifest(weather)
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
	if manifest.Manifest != 1 || manifest.SDK != rutis.Version || manifest.PluginAPI != rutis.PluginAPI || manifest.Runtime != "rutis-go-runtime:1" {
		t.Fatalf("%s", data)
	}
	plugin := manifest.Plugins["weather"]
	if fmt.Sprint(plugin["inject"]) != "[llm]" || plugin["config"] == nil {
		t.Fatalf("%s", data)
	}
}

func TestTwoPluginsMayNotShareAName(t *testing.T) {
	err := rutis.ServeArgs([]string{"--rutis-manifest"}, weather, weather)
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
	data, err := rutis.Manifest(undeclared)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(data), `"announces":true`) {
		t.Fatalf("the manifest says it announces: %s", data)
	}
}
