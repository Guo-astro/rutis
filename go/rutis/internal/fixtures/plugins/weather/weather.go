// Package weather is the runtime conformance plugin (rutis_bridge::runtime::testing::runtime)
// for the Go runtime: it injects `clock` and provides `weather`. What calls on
// it return is in crates/rutis-bridge/src/runtime/testing/weather.expected.json.
package weather

import (
	"context"
	"fmt"
	"math/big"
	"os"
	"sort"
	"sync"
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
	mu    sync.RWMutex
	clock Clock
	city  string
}

func (w *Weather) Today(ctx context.Context) (string, error) {
	now, err := w.clock.Now(ctx)
	if err != nil {
		return "", err
	}
	w.mu.RLock()
	defer w.mu.RUnlock()
	return fmt.Sprintf("%s at %d", w.city, now), nil
}

func (w *Weather) Later(ctx context.Context) string {
	time.Sleep(10 * time.Millisecond)
	w.mu.RLock()
	defer w.mu.RUnlock()
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

// Reading is a struct with fields encoding/json would have turned into
// text: they cross as their values (V9).
type Reading struct {
	Raw []byte     `json:"raw,omitempty"`
	At  *time.Time `json:"at,omitempty"`
	N   int64      `json:"n,omitempty"`
}

var oslo = time.FixedZone("Oslo", 2*3600)

// samples are values only Go makes, by name (`samples` in the
// expectations).
var samples = map[string]func() any{
	"go/value-instant":      func() any { return time.Date(2026, 10, 15, 5, 22, 15, 123456789, oslo) },
	"go/value-struct-bytes": func() any { return Reading{Raw: []byte("hello")} },
	"go/value-struct-time": func() any {
		at := time.Date(2026, 10, 15, 5, 22, 15, 0, oslo)
		return Reading{At: &at}
	},
	"go/value-int-keys": func() any { return map[int]string{2: "b", 1: "a"} },
	"go/value-set":      func() any { return map[string]struct{}{"b": {}, "a": {}} },
	"go/value-bigint": func() any {
		n, _ := new(big.Int).SetString("-123456789012345678901", 10)
		return n
	},
	"go/value-constructed": func() any {
		return []any{
			rutis.DateTime{Date: rutis.Date{Year: 2026, Month: 10, Day: 15}, Time: rutis.TimeOfDay{Hour: 14, Minute: 30, Second: 5, Nanosecond: 250_000_000}},
			rutis.Date{Year: 2026, Month: 10, Day: 15},
			rutis.TimeOfDay{Hour: 14, Minute: 30, Second: 5, Nanosecond: 250_000_000},
			rutis.Decimal("1.10"),
			rutis.UUID("0f8fad5b-d9cb-469f-a165-70867728950e"),
		}
	},
	"go/big-int-in-struct": func() any { return Reading{N: 1 << 60} },
	"go/big-int-keys":      func() any { return map[uint64]string{1<<64 - 1: "max", 1: "one"} },
}

// Samples names the values only Go makes.
func (w *Weather) Samples() []string {
	names := make([]string, 0, len(samples))
	for name := range samples {
		names = append(names, name)
	}
	sort.Strings(names)
	return names
}

// Sample returns the value `name`.
func (w *Weather) Sample(name string) (any, error) {
	sample, ok := samples[name]
	if !ok {
		return nil, fmt.Errorf("no sample %s", name)
	}
	return sample(), nil
}

var Plugin = rutis.Define(rutis.Plugin[Config]{
	Name:     "weather",
	Inject:   []string{"clock"},
	Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather](rutis.Sync("Today", "Each", "Echo", "Crash", "Samples", "Sample"))},
	Apply: func(ctx *rutis.Ctx, config Config) error {
		var clock Clock
		if err := ctx.Use("clock", &clock); err != nil {
			return err
		}
		city := config.City
		if city == "" {
			city = "Oslo"
		}
		w := &Weather{clock: clock, city: city}
		ctx.Provide("weather", w)
		ctx.On("weather/city", func(_ context.Context, args ...any) error {
			if len(args) != 1 {
				return fmt.Errorf("weather/city needs one city")
			}
			city, ok := args[0].(string)
			if !ok {
				return fmt.Errorf("weather/city needs a string")
			}
			w.mu.Lock()
			w.city = city
			w.mu.Unlock()
			return nil
		})
		ctx.Effect(func(context.Context) error {
			fmt.Println("weather: bye")
			return nil
		})
		return nil
	},
})
