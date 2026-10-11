// Package probe is a plugin that exercises what crosses the session:
// cancellation, error names, panics, objects by reference.
package probe

import (
	"context"
	"sync"

	"github.com/arcships/rutis/go/rutis"
)

// Work is the `work` service.
type Work struct {
	mu      sync.Mutex
	started bool
	aborted bool
	counter *Counter
}

// Wait blocks until its caller cancels.
func (w *Work) Wait(ctx context.Context) error {
	w.mu.Lock()
	w.started = true
	w.mu.Unlock()
	<-ctx.Done()
	w.mu.Lock()
	w.aborted = true
	w.mu.Unlock()
	return ctx.Err()
}

// Started reports that Wait runs (a test drops its call after that).
func (w *Work) Started() bool {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.started
}

func (w *Work) Aborted() bool {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.aborted
}

func (w *Work) Ping() string { return "pong" }

// Quota is an error with a type name.
type Quota struct{ Left int }

func (q *Quota) Error() string { return "quota exhausted" }

// Fail returns a *Quota.
func (w *Work) Fail() error { return &Quota{} }

// Named returns an error that names itself.
func (w *Work) Named() error { return named{} }

type named struct{}

func (named) Error() string { return "no such city" }
func (named) Name() string  { return "NotFound" }

// Boom panics.
func (w *Work) Boom() string { panic("boom") }

// Counter returns the same counter, by reference.
func (w *Work) Counter() any { return rutis.Ref(w.counter) }

// Record returns data.
func (w *Work) Record() Point { return Point{X: 1, Y: 2} }

// Sum adds what it is given.
func (w *Work) Sum(ctx context.Context, values []int) int {
	total := 0
	for _, v := range values {
		total += v
	}
	return total
}

// Point is plain data.
type Point struct {
	X int `json:"x"`
	Y int `json:"y"`
}

// Counter is an object that crosses by reference.
type Counter struct {
	mu    sync.Mutex
	Total int `json:"total"`
}

func (c *Counter) Add(n int) int {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.Total += n
	return c.Total
}

var Plugin = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name: "probe",
	Provides: rutis.Provides{"work": rutis.MethodsOf[*Work](
		rutis.Sync("Started", "Aborted", "Ping", "Fail", "Named", "Boom", "Counter", "Record", "Sum"),
	)},
	Apply: func(ctx *rutis.Ctx, _ rutis.NoConfig) error {
		ctx.Provide("work", &Work{counter: &Counter{}})
		return nil
	},
})
