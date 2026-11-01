// guarantee: sdk/optional-deps
package rutis

import (
	"context"
	"testing"
)

// What a row of this process provides is its object itself, when the target
// can hold it: an interface the object implements, its own type, any
// (docs/design-go-local-objects-2026-11-01.md).

// Canvas is a service of the test's own whose methods do not cross (a
// function argument, two results): only this process can call them.
type Canvas interface {
	Draw(width int, onPixel func(x, y int)) (painted int, ok bool)
	Title() string
}

type canvas struct{ title string }

func (c *canvas) Draw(width int, onPixel func(x, y int)) (int, bool) {
	for x := 0; x < width; x++ {
		onPixel(x, 0)
	}
	return width, true
}

func (c *canvas) Title() string { return c.title }

// Shapes names a method the canvas lacks; Other one it has with another type.
type Shapes interface {
	Title() string
	Resize(width int) error
}

type Other interface {
	Title() string
	Draw(width int) int
}

// loadPair loads a row that provides `canvas` and one that reads it, and
// returns the reading row's error.
func loadPair(t *testing.T, provide func(c *Ctx), use func(c *Ctx) error) error {
	t.Helper()
	writer := Define(Plugin[NoConfig]{Name: "writer", Apply: func(c *Ctx, _ NoConfig) error {
		provide(c)
		return nil
	}})
	reader := Define(Plugin[NoConfig]{Name: "reader", Inject: []string{"canvas"}, Apply: func(c *Ctx, _ NoConfig) error {
		return use(c)
	}})
	rt := newRuntime([]*Definition{writer, reader})
	ctx := context.Background()
	t.Cleanup(func() { _ = rt.dispose(ctx) })
	if err := rt.load(ctx, "w", "writer", nil, nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	return rt.load(ctx, "r", "reader", nil, nil, nil, nil)
}

// An interface target gets the provider's own object: the same pointer, its
// methods called as they are.
func TestAnInterfaceTargetIsTheProvidersObject(t *testing.T) {
	painted := &canvas{title: "board"}
	var got Canvas
	err := loadPair(t, func(c *Ctx) { c.Provide("canvas", painted) }, func(c *Ctx) error {
		return c.Use("canvas", &got)
	})
	if err != nil {
		t.Fatal(err)
	}
	if got != Canvas(painted) {
		t.Fatalf("use gave %T %v, not the object provided", got, got)
	}
	pixels := 0
	if n, ok := got.Draw(3, func(x, y int) { pixels++ }); !ok || n != 3 || pixels != 3 {
		t.Fatalf("draw: %d %v, %d pixels", n, ok, pixels)
	}
	if got.Title() != "board" {
		t.Fatalf("title: %q", got.Title())
	}
}

// `any` in this process is the object; for a service of another process it
// is the proxy, as an object of another process received as any is.
func TestAnyIsTheObjectOrTheProxy(t *testing.T) {
	painted := &canvas{title: "board"}
	var local any
	if err := loadPair(t, func(c *Ctx) { c.Provide("canvas", painted) }, func(c *Ctx) error {
		return c.Use("canvas", &local)
	}); err != nil {
		t.Fatal(err)
	}
	if local != any(painted) {
		t.Fatalf("any in this process: %T %v", local, local)
	}

	rt := newRuntime(nil)
	rt.hosts["canvas"] = map[string]string{"title": "async"}
	row := &row{key: "r", name: "r", def: &Definition{inject: []string{"canvas"}}}
	c := &Ctx{Context: context.Background(), rt: rt, row: row}
	var host any
	if err := c.Use("canvas", &host); err != nil {
		t.Fatal(err)
	}
	if _, ok := host.(*Service); !ok {
		t.Fatalf("any for a host service: %T", host)
	}
}

// An interface the object does not implement says which method, in the
// compiler's words.
func TestAnInterfaceTheObjectDoesNotImplementSaysWhichMethod(t *testing.T) {
	var shapes Shapes
	err := loadPair(t, func(c *Ctx) { c.Provide("canvas", &canvas{}) }, func(c *Ctx) error {
		return c.Use("canvas", &shapes)
	})
	want := `use canvas: *rutis.canvas does not implement rutis.Shapes (missing method Resize)`
	if err == nil || err.Error() != want {
		t.Fatalf("missing: got %v, want %q", err, want)
	}

	var other Other
	err = loadPair(t, func(c *Ctx) { c.Provide("canvas", &canvas{}) }, func(c *Ctx) error {
		return c.Use("canvas", &other)
	})
	want = `use canvas: *rutis.canvas does not implement rutis.Other (wrong type for method Draw)`
	if err == nil || err.Error() != want {
		t.Fatalf("wrong type: got %v, want %q", err, want)
	}

	// Provided as a value whose methods are on its pointer.
	var byValue Canvas
	err = loadPair(t, func(c *Ctx) { c.Provide("canvas", canvas{}) }, func(c *Ctx) error {
		return c.Use("canvas", &byValue)
	})
	want = `use canvas: rutis.canvas does not implement rutis.Canvas (method Draw has pointer receiver)`
	if err == nil || err.Error() != want {
		t.Fatalf("pointer receiver: got %v, want %q", err, want)
	}
}

// An interface target for a service of another process: methods and no Go
// type, so say what to use instead.
func TestAnInterfaceTargetForAnotherProcessSaysWhatToUse(t *testing.T) {
	rt := newRuntime(nil)
	rt.hosts["canvas"] = map[string]string{"title": "async"}
	row := &row{key: "r", name: "r", def: &Definition{inject: []string{"canvas"}}}
	c := &Ctx{Context: context.Background(), rt: rt, row: row}
	var got Canvas
	err := c.Use("canvas", &got)
	want := "use canvas: rutis.Canvas is an interface, which only a service provided in this process fills: for one of another process, use a struct of function fields or *advanced.Service"
	if err == nil || err.Error() != want {
		t.Fatalf("got %v, want %q", err, want)
	}
}

// A struct of function fields in this process takes the provider's own
// method when the types are the same, even when it does not end with an
// error.
func TestAFunctionFieldTakesTheMethodItself(t *testing.T) {
	var target struct {
		Title func() string
	}
	if err := loadPair(t, func(c *Ctx) { c.Provide("canvas", &canvas{title: "board"}) }, func(c *Ctx) error {
		return c.Use("canvas", &target)
	}); err != nil {
		t.Fatal(err)
	}
	if target.Title == nil || target.Title() != "board" {
		t.Fatalf("title: nil %v", target.Title == nil)
	}
}

// Get reads the object too, for a plugin that lists it in Optional.
func TestGetReadsTheObject(t *testing.T) {
	painted := &canvas{title: "board"}
	writer := Define(Plugin[NoConfig]{Name: "writer", Apply: func(c *Ctx, _ NoConfig) error {
		c.Provide("canvas", painted)
		return nil
	}})
	var got Canvas
	var found bool
	reader := Define(Plugin[NoConfig]{Name: "reader", Optional: []string{"canvas"}, Apply: func(c *Ctx, _ NoConfig) error {
		var err error
		found, err = c.Get("canvas", &got)
		return err
	}})
	rt := newRuntime([]*Definition{writer, reader})
	ctx := context.Background()
	t.Cleanup(func() { _ = rt.dispose(ctx) })
	if err := rt.load(ctx, "w", "writer", nil, nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	if err := rt.load(ctx, "r", "reader", nil, nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	if !found || got != Canvas(painted) {
		t.Fatalf("get: %v %T", found, got)
	}
}
