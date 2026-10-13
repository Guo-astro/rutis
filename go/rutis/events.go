package rutis

import (
	"context"
	"fmt"
	"slices"
	"strings"
	"sync"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// AggregateError reports every failed listener of a host emission, in
// registration order. Unwrap lets errors.Is/As inspect each failure.
type AggregateError = peer.AggregateError

type listener struct {
	row  *row
	call func(context.Context, ...any) error
}

func eventName(name string) error {
	if strings.HasPrefix(name, "internal/") {
		return fmt.Errorf("event %s is internal to the runtime", name)
	}
	return nil
}

// On listens on the process's shared event bus until this row unloads or
// the returned disposer is called. Each registration is independent.
// Reserved names, nil listeners and an inactive row panic, as Provide does
// for invalid registrations (in Apply, loading fails).
func (c *Ctx) On(name string, call func(context.Context, ...any) error) func() {
	if err := eventName(name); err != nil {
		panic(err)
	}
	if call == nil {
		panic("rutis: On needs a listener")
	}
	c.rt.mu.Lock()
	defer c.rt.mu.Unlock()
	if c.rt.rows[c.row.key] != c.row {
		panic(fmt.Sprintf("rutis: row %s is not active", c.row.key))
	}
	entry := &listener{row: c.row, call: call}
	c.rt.events[name] = append(c.rt.events[name], entry)
	return func() {
		c.rt.mu.Lock()
		defer c.rt.mu.Unlock()
		for i, current := range c.rt.events[name] {
			if current == entry {
				c.rt.events[name] = slices.Delete(c.rt.events[name], i, i+1)
				if len(c.rt.events[name]) == 0 {
					delete(c.rt.events, name)
				}
				break
			}
		}
	}
}

// Emit calls listeners synchronously in registration order, returning the
// first error. Pass the context your caller supplied so listener calls back
// to the host retain the synchronous wait chain. Unlike JS/Python's void
// emit, every Go listener finishes before the next starts.
func (c *Ctx) Emit(ctx context.Context, name string, args ...any) error {
	if ctx == nil {
		panic("rutis: a nil context: pass the context.Context your caller gave you")
	}
	if err := eventName(name); err != nil {
		return err
	}
	for _, entry := range c.rt.listeners(name) {
		if err := entry.invoke(ctx, args); err != nil {
			return err
		}
	}
	return nil
}

func (rt *runtimeState) listeners(name string) []*listener {
	rt.mu.Lock()
	defer rt.mu.Unlock()
	return append([]*listener(nil), rt.events[name]...)
}

func (entry *listener) invoke(caller context.Context, args []any) error {
	ctx, cancel := context.WithCancel(caller)
	stop := context.AfterFunc(entry.row.ctx, cancel)
	defer stop()
	defer cancel()
	if entry.row.ctx.Err() != nil {
		cancel()
	}
	_, err := peer.Protect(func() (any, error) { return nil, entry.call(ctx, append([]any(nil), args...)...) })
	return err
}

func (rt *runtimeState) emit(ctx context.Context, name string, args []any) error {
	listeners := rt.listeners(name)
	errs := make([]error, len(listeners))
	var done sync.WaitGroup
	for i, entry := range listeners {
		done.Add(1)
		go func() { defer done.Done(); errs[i] = entry.invoke(ctx, args) }()
	}
	done.Wait()
	errs = slices.DeleteFunc(errs, func(err error) bool { return err == nil })
	if len(errs) != 0 {
		return &AggregateError{Errors: errs}
	}
	return nil
}

// forgetListeners runs with rt.mu held, before cancelling the row. A
// dispatch already in progress retains its snapshot and sees cancellation.
func (rt *runtimeState) forgetListeners(r *row) {
	for name, listeners := range rt.events {
		listeners = slices.DeleteFunc(listeners, func(entry *listener) bool { return entry.row == r })
		if len(listeners) == 0 {
			delete(rt.events, name)
		} else {
			rt.events[name] = listeners
		}
	}
}
