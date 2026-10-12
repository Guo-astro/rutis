// Package weather is the runtime conformance plugin (rutis_bridge::runtime::testing::runtime)
// for the Go runtime: it injects `clock` and provides `weather`. What calls on
// it return is in crates/rutis-bridge/src/runtime/testing/weather.expected.json.
package weather

import (
	"context"
	"fmt"
	"os"
	"time"

	"github.com/arcships/rutis/go/rutis"
)

type Config struct {
	City string `json:"city,omitempty" doc:"the city to report on"`
}

type Clock struct {
	Now func(ctx context.Context) (int, error)
}

type Weather struct {
	clock Clock
	city  string
}

func (w *Weather) Today(ctx context.Context) (string, error) {
	now, err := w.clock.Now(ctx)
	if err != nil {
		return "", err
	}
	return fmt.Sprintf("%s at %d", w.city, now), nil
}

func (w *Weather) Later(ctx context.Context) string {
	time.Sleep(10 * time.Millisecond)
	return w.city + " later"
}

// Each calls back during the call.
func (w *Weather) Each(ctx context.Context, callback func(ctx context.Context, day string) (string, error)) ([]string, error) {
	var out []string
	for _, day := range []string{"mon", "tue"} {
		value, err := callback(ctx, day)
		if err != nil {
			return nil, err
		}
		out = append(out, value)
	}
	return out, nil
}

// Echo returns the value it was given.
func (w *Weather) Echo(ctx context.Context, value any) any { return value }

func (w *Weather) Crash() { os.Exit(17) }

var Plugin = rutis.Define(rutis.Plugin[Config]{
	Name:     "weather",
	Inject:   []string{"clock"},
	Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Today", "Each", "Echo", "Crash"))},
	Apply: func(ctx *rutis.Ctx, config Config) error {
		var clock Clock
		if err := ctx.Use("clock", &clock); err != nil {
			return err
		}
		city := config.City
		if city == "" {
			city = "Oslo"
		}
		ctx.Provide("weather", &Weather{clock: clock, city: city})
		ctx.Effect(func(context.Context) error {
			fmt.Println("weather: bye")
			return nil
		})
		return nil
	},
})
