// Package conformance is the plugin conformance-events (#464,
// docs/design-events-protocol-2026-10-18.md §13.1): each row is one
// participant of an event expectation, a listener or an emitter, and logs
// what it does through the host's synchronous service events-log.
package conformance

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/arcships/rutis/go/rutis"
)

// Participant is a row's configuration: a listener, or an emitter.
type Participant struct {
	Label   string          `json:"label"`
	Emitter bool            `json:"emitter"`
	Name    string          `json:"name"`
	Log     json.RawMessage `json:"log"`
	Does    json.RawMessage `json:"does"`
	Options struct {
		Prepend bool `json:"prepend"`
		Global  bool `json:"global"`
	} `json:"options"`
	// A service of this name the row provides besides (rutis/service).
	Provides string `json:"provides"`
}

// Spec is an emission (§13.2).
type Spec struct {
	Mode   string `json:"mode"`
	Name   string `json:"name"`
	Args   []any  `json:"args"`
	Filter *struct {
		Keep []string `json:"keep"`
	} `json:"filter"`
	Terminal *string `json:"terminal"`
	Cancel   *struct {
		When string `json:"when"`
	} `json:"cancel"`
}

type logService struct {
	Add func(ctx context.Context, entry string) error
}

// logged lets an emitter of this process wait for a listener of this
// process to log (cancel.when).
var logged = struct {
	sync.Mutex
	entries []string
	changed chan struct{}
}{changed: make(chan struct{})}

func note(entry string) {
	logged.Lock()
	logged.entries = append(logged.entries, entry)
	close(logged.changed)
	logged.changed = make(chan struct{})
	logged.Unlock()
}

// waitLogged waits until `entry` was logged in this process.
func waitLogged(ctx context.Context, entry string) error {
	for {
		logged.Lock()
		for _, e := range logged.entries {
			if e == entry {
				logged.Unlock()
				return nil
			}
		}
		changed := logged.changed
		logged.Unlock()
		select {
		case <-changed:
		case <-ctx.Done():
			return ctx.Err()
		}
	}
}

type row struct {
	c   *rutis.Ctx
	p   Participant
	log logService
}

func (r *row) add(ctx context.Context, entry string) {
	note(entry)
	_ = r.log.Add(context.WithoutCancel(ctx), entry)
}

// named is an error with the name `does.throws` gives.
type named struct{ name, message string }

func (e *named) Error() string { return e.message }
func (e *named) Name() string  { return e.name }

// Emitter is the service emitter-<label>.
type Emitter struct{ r *row }

// Dispatch emits `spec` from the row, natively, and returns the result.
func (e *Emitter) Dispatch(ctx context.Context, spec Spec) (any, error) {
	return e.r.dispatch(ctx, spec)
}

// DispatchSync is Dispatch for `bail`, as a synchronous method.
func (e *Emitter) DispatchSync(ctx context.Context, spec Spec) (any, error) {
	return e.r.dispatch(ctx, spec)
}

// Present is the service a row provides with `provides`.
type Present struct{}

func (Present) Ping() string { return "pong" }

func (r *row) dispatch(ctx context.Context, spec Spec) (any, error) {
	if spec.Cancel != nil {
		var cancel context.CancelFunc
		ctx, cancel = context.WithCancel(ctx)
		defer cancel()
		go func() {
			if waitLogged(ctx, spec.Cancel.When) == nil {
				cancel()
			}
		}()
	}
	emitter := struct {
		emit      func(context.Context, string, ...any) error
		parallel  func(context.Context, string, ...any) error
		serial    func(context.Context, string, ...any) (any, error)
		waterfall func(context.Context, string, rutis.Next, ...any) (any, error)
		bail      func(context.Context, string, ...any) (any, error)
	}{r.c.Emit, r.c.Parallel, r.c.Serial, r.c.Waterfall, r.c.Bail}
	if spec.Filter != nil {
		keep := spec.Filter.Keep
		filtered := r.c.Filter(func(l rutis.Listener) bool {
			r.add(ctx, "filter:"+l.Row)
			for _, label := range keep {
				if label == l.Row {
					return true
				}
			}
			return false
		})
		emitter.emit, emitter.parallel, emitter.serial, emitter.waterfall, emitter.bail = filtered.Emit, filtered.Parallel, filtered.Serial, filtered.Waterfall, filtered.Bail
	}
	switch spec.Mode {
	case "emit":
		return nil, emitter.emit(ctx, spec.Name, spec.Args...)
	case "parallel":
		return nil, emitter.parallel(ctx, spec.Name, spec.Args...)
	case "serial":
		return emitter.serial(ctx, spec.Name, spec.Args...)
	case "bail":
		return emitter.bail(ctx, spec.Name, spec.Args...)
	case "waterfall":
		var terminal rutis.Next
		if spec.Terminal != nil {
			text := *spec.Terminal
			terminal = func(ctx context.Context) (any, error) { r.add(ctx, "terminal"); return text, nil }
		}
		return emitter.waterfall(ctx, spec.Name, terminal, spec.Args...)
	}
	return nil, fmt.Errorf("conformance-events: mode %s is not the Go runtime's to emit", spec.Mode)
}

// entry is what a listener logs when it starts (§13.2 `log`).
func (r *row) entry(args []any) (string, bool) {
	label := r.p.Label
	log := strings.TrimSpace(string(r.p.Log))
	switch {
	case log == "" || log == `"label"`:
		return label, true
	case log == "false":
		return "", false
	case log == `"arg"`:
		var first any
		if len(args) > 0 {
			first = args[0]
		}
		return fmt.Sprintf("%s:%v", label, text(first)), true
	}
	var fields []string
	if err := json.Unmarshal(r.p.Log, &fields); err != nil {
		return label, true
	}
	record, _ := first(args).(map[string]any)
	values := make([]string, len(fields))
	for i, field := range fields {
		values[i] = text(record[field])
	}
	return label + ":" + strings.Join(values, ","), true
}

func first(args []any) any {
	if len(args) > 0 {
		return args[0]
	}
	return nil
}

// text is a logged value: numbers without a fraction as integers.
func text(v any) string {
	switch x := v.(type) {
	case float64:
		if x == float64(int64(x)) {
			return fmt.Sprint(int64(x))
		}
	case nil:
		return "null"
	}
	return fmt.Sprint(v)
}

func (r *row) listen(ctx context.Context, args ...any) (any, error) {
	if entry, ok := r.entry(args); ok {
		r.add(ctx, entry)
	}
	return r.does(ctx, r.p.Does, args)
}

func (r *row) does(ctx context.Context, raw json.RawMessage, args []any) (any, error) {
	var does map[string]json.RawMessage
	if err := json.Unmarshal(raw, &does); err != nil {
		return nil, fmt.Errorf("conformance-events: does %s: %w", raw, err)
	}
	next := func() rutis.Next {
		if len(args) == 0 {
			return nil
		}
		next, _ := args[len(args)-1].(rutis.Next)
		return next
	}
	if v, ok := does["returns"]; ok {
		var value any
		err := json.Unmarshal(v, &value)
		return value, err
	}
	if _, ok := does["returnsNothing"]; ok {
		return nil, nil
	}
	if _, ok := does["echo"]; ok {
		return first(args), nil
	}
	if v, ok := does["throws"]; ok {
		var thrown struct{ Name, Message string }
		_ = json.Unmarshal(v, &thrown)
		return nil, &named{thrown.Name, thrown.Message}
	}
	if v, ok := does["after"]; ok {
		var ms int
		_ = json.Unmarshal(v, &ms)
		select {
		case <-time.After(time.Duration(ms) * time.Millisecond):
		case <-ctx.Done():
			return nil, ctx.Err()
		}
		return r.does(ctx, does["then"], args)
	}
	if v, ok := does["next"]; ok {
		var prefix string
		_ = json.Unmarshal(v, &prefix)
		n := next()
		if n == nil {
			return nil, fmt.Errorf("conformance-events: %s has no next in this dispatch", r.p.Label)
		}
		rest, err := n(ctx)
		if err != nil {
			return nil, err
		}
		if rest == nil {
			rest = ""
		}
		return prefix + text(rest), nil
	}
	if v, ok := does["veto"]; ok {
		var value any
		err := json.Unmarshal(v, &value)
		return value, err
	}
	if _, ok := does["nextTwice"]; ok {
		n := next()
		if n == nil {
			return nil, fmt.Errorf("conformance-events: %s has no next in this dispatch", r.p.Label)
		}
		if _, err := n(ctx); err != nil {
			return nil, err
		}
		return n(ctx)
	}
	if _, ok := does["waits"]; ok {
		<-ctx.Done()
		r.add(ctx, r.p.Label+":cancelled")
		return nil, nil
	}
	if _, ok := does["calls"]; ok {
		fn, ok := first(args).(*rutis.Function)
		if !ok {
			return nil, fmt.Errorf("conformance-events: %s needs a function, got %T", r.p.Label, first(args))
		}
		var out any
		err := fn.Call(ctx, &out, "x")
		return out, err
	}
	if v, ok := does["emits"]; ok {
		var spec Spec
		if err := json.Unmarshal(v, &spec); err != nil {
			return nil, err
		}
		if spec.Name == "" {
			spec.Name = r.p.Name
		}
		return r.dispatch(ctx, spec)
	}
	return nil, fmt.Errorf("conformance-events: unknown does %s", raw)
}

// Plugin is conformance-events.
var Plugin = rutis.Define(rutis.Plugin[Participant]{
	Name:     "conformance-events",
	Inject:   []string{"events-log"},
	Provides: rutis.Provides{},
	Apply: func(c *rutis.Ctx, p Participant) error {
		r := &row{c: c, p: p}
		if err := c.Use("events-log", &r.log); err != nil {
			return err
		}
		if p.Emitter {
			c.Provide("emitter-"+p.Label, &Emitter{r})
		} else {
			var options []rutis.ListenOption
			if p.Options.Prepend {
				options = append(options, rutis.Prepend())
			}
			if p.Options.Global {
				options = append(options, rutis.Global())
			}
			c.On(p.Name, r.listen, options...)
		}
		if p.Provides != "" {
			c.Provide(p.Provides, Present{})
		}
		return nil
	},
})
