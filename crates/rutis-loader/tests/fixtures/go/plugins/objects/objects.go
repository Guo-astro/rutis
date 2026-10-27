// Package objects holds the Go owner and holder of the object cases
// (crates/rutis-loader/tests/objects_cross.rs, #687): the same as the
// JavaScript and Python ones there. The owner hands its counter (by
// reference: rutis.Ref) to the holder's service in another runtime; the
// holder keeps what it is given as a *rutis.Service and calls it.
package objects

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"time"

	"github.com/arcships/rutis/go/rutis"
)

// Counter is the owner's object.
type Counter struct{ n int }

func (c *Counter) Inc() int   { c.n++; return c.n }
func (c *Counter) Value() int { return c.n }

// Pinger is an object with one method.
type Pinger struct{}

func (Pinger) Ping() string { return "pong" }

var made = &Counter{}

// holderService is the holder's service as the owner uses it.
type holderService struct {
	Take   func(ctx context.Context, counter any) (string, error)
	Inc    func(ctx context.Context) (int, error)
	Listed func(ctx context.Context, items []any) (string, error)
	Back   func(ctx context.Context) (any, error)
	Rust   func(ctx context.Context) (int, error)
	Nope   func(ctx context.Context) (string, error)
	Plain  func(ctx context.Context, thing any) (string, error)
}

// Owner is the `owner` service.
type Owner struct{ ctx *rutis.Ctx }

func (o *Owner) Run(ctx context.Context) (string, error) {
	var holder holderService
	if err := o.ctx.Use("holder", &holder); err != nil {
		return "", err
	}
	if _, err := holder.Take(ctx, rutis.Ref(made)); err != nil {
		return "", err
	}
	for range 2 {
		if _, err := holder.Inc(ctx); err != nil {
			return "", err
		}
	}
	listed, err := holder.Listed(ctx, []any{map[string]any{"data": 1}, rutis.Ref(made)})
	if err != nil {
		return "", err
	}
	back, err := holder.Back(ctx)
	if err != nil {
		return "", err
	}
	viaRust, err := holder.Rust(ctx)
	if err != nil {
		return "", err
	}
	missing, err := holder.Nope(ctx)
	if err != nil {
		return "", err
	}
	plain, err := holder.Plain(ctx, rutis.Ref(Pinger{}))
	if err != nil {
		return "", err
	}
	return fmt.Sprintf("%d %s %t %d %s | %s", made.n, listed, back == made, viaRust, plain, missing), nil
}

// Die ends this runtime's process, as a crash does.
func (o *Owner) Die(ctx context.Context) string {
	go func() {
		time.Sleep(20 * time.Millisecond)
		os.Exit(1)
	}()
	return "dying"
}

func (o *Owner) Announce(ctx context.Context) (string, error) {
	return "sent", o.ctx.Emit(ctx, "counter", rutis.Ref(made))
}

// GoOwner provides `owner`.
var GoOwner = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:   "go_owner",
	Inject: []string{"holder"},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		ctx.Provide("owner", &Owner{ctx: ctx})
		return nil
	},
})

// said is an error as the holder answers it: `<code>: <message>`.
func said(err error) string {
	var remote *rutis.RemoteError
	if errors.As(err, &remote) {
		return remote.Code + ": " + remote.Message
	}
	return err.Error()
}

// Holder is the `holder` service: what it is given is a *rutis.Service.
type Holder struct {
	ctx  *rutis.Ctx
	kept *rutis.Service
}

func (h *Holder) Take(ctx context.Context, counter *rutis.Service) string {
	h.kept = counter
	return "taken"
}

func (h *Holder) Inc(ctx context.Context) (int, error) {
	var n int
	err := h.kept.Call(ctx, "inc", &n)
	return n, err
}

func (h *Holder) Listed(ctx context.Context, items []any) string {
	data, _ := json.Marshal(items[0])
	return fmt.Sprintf("%s %t", data, items[1] == any(h.kept))
}

func (h *Holder) Back(ctx context.Context) *rutis.Service { return h.kept }

func (h *Holder) Rust(ctx context.Context) (int, error) {
	var poker struct {
		Poke func(ctx context.Context, object *rutis.Service) (int, error)
	}
	if err := h.ctx.Use("poker", &poker); err != nil {
		return 0, err
	}
	return poker.Poke(ctx, h.kept)
}

func (h *Holder) Nope(ctx context.Context) string {
	if err := h.kept.Call(ctx, "nope", nil); err != nil {
		return said(err)
	}
	return "answered"
}

// Plain takes its object as a struct of function fields.
func (h *Holder) Plain(ctx context.Context, thing struct {
	Ping func(ctx context.Context) (string, error)
}) (string, error) {
	return thing.Ping(ctx)
}

func (h *Holder) After(ctx context.Context) string {
	var n int
	if err := h.kept.Call(ctx, "inc", &n); err != nil {
		return said(err)
	}
	return fmt.Sprint(n)
}

// Tally has a data field: it cannot take an object.
type Tally struct {
	Inc func(ctx context.Context) (int, error)
	N   int
}

// Wrong takes a Tally.
func (h *Holder) Wrong(ctx context.Context, counter Tally) string {
	return "taken"
}

// Text takes a string, which cannot take an object.
func (h *Holder) Text(ctx context.Context, text string) string { return text }

// GoHolder provides `holder`, and hears `counter` events.
var GoHolder = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:   "go_holder",
	Inject: []string{"poker", "probe"},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		holder := &Holder{ctx: ctx}
		var probe struct {
			Record func(ctx context.Context, line string) error
		}
		if err := ctx.Use("probe", &probe); err != nil {
			return err
		}
		ctx.On("counter", func(c context.Context, args ...any) (any, error) {
			counter, _ := args[0].(*rutis.Service)
			var n int
			if err := counter.Call(c, "value", &n); err != nil {
				return nil, err
			}
			return nil, probe.Record(c, fmt.Sprintf("event: %t %d", counter == holder.kept, n))
		})
		ctx.Provide("holder", holder)
		return nil
	},
})
