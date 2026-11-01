package rutis

import (
	"bytes"
	"context"
	"strconv"
	"strings"
	"testing"
)

// Tally counts, as a service of the test's own.
type Tally struct{ N int }

func (t Tally) Count() int { return t.N }

// readTally reads `name` through `c`: the number, or what failed.
func readTally(c *Ctx, name string) string {
	var service *Service
	if err := c.Use(name, &service); err != nil {
		return "use: " + err.Error()
	}
	var n int
	if err := service.Call(context.Background(), "count", &n); err != nil {
		return "call: " + err.Error()
	}
	return "count " + strconv.Itoa(n)
}

// A cleanup registered after `provide` reads what its row provides: the
// withdrawal is an entry of the same stack, behind it. One registered
// before runs after the withdrawal and no longer counts it as its own
// (Rust's drain_effects; declared services D4).
func TestACleanupRegisteredAfterProvideReadsIt(t *testing.T) {
	var seen []string
	plugin := Define(Plugin[NoConfig]{Name: "order", Apply: func(c *Ctx, _ NoConfig) error {
		c.Effect(func(context.Context) error { seen = append(seen, "before: "+readTally(c, "tally")); return nil })
		c.Provide("tally", Tally{N: 7})
		c.Effect(func(context.Context) error { seen = append(seen, "after: "+readTally(c, "tally")); return nil })
		return nil
	}})
	rt := newRuntime([]*Definition{plugin})
	ctx := context.Background()
	t.Cleanup(func() { _ = rt.dispose(ctx) })
	if err := rt.load(ctx, "o", "order", nil, nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	if err := rt.unload(ctx, "o"); err != nil {
		t.Fatal(err)
	}
	want := []string{
		"after: count 7",
		`before: use: service tally is not available to this plugin, which does not declare it in Inject: add "tally" to its Inject`,
	}
	if strings.Join(seen, "\n") != strings.Join(want, "\n") {
		t.Fatalf("cleanups saw:\n%s\nwant:\n%s", strings.Join(seen, "\n"), strings.Join(want, "\n"))
	}
}

// Another row loses what a row provides when that row starts unloading,
// not when its withdrawal's place on the stack comes; the row itself
// still reads it then (Rust's read_binding_as).
func TestAnotherRowLosesItWhenTheUnloadStarts(t *testing.T) {
	var user *Ctx
	var own, other string
	provider := Define(Plugin[NoConfig]{Name: "provider", Apply: func(c *Ctx, _ NoConfig) error {
		c.Provide("tally", Tally{N: 2})
		// Registered after the provide: it runs before the withdrawal.
		c.Effect(func(context.Context) error {
			own = readTally(c, "tally")
			other = readTally(user, "tally")
			return nil
		})
		return nil
	}})
	using := Define(Plugin[NoConfig]{Name: "using", Inject: []string{"tally"}, Apply: func(c *Ctx, _ NoConfig) error {
		user = c
		return nil
	}})
	rt := newRuntime([]*Definition{provider, using})
	ctx := context.Background()
	t.Cleanup(func() { _ = rt.dispose(ctx) })
	if err := rt.load(ctx, "p", "provider", nil, nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	if err := rt.load(ctx, "u", "using", nil, nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	if got := readTally(user, "tally"); got != "count 2" {
		t.Fatalf("before the unload: %q", got)
	}
	if err := rt.unload(ctx, "p"); err != nil {
		t.Fatal(err)
	}
	if own != "count 2" {
		t.Fatalf("the provider's own cleanup: %q", own)
	}
	if other != "use: service tally is not available" {
		t.Fatalf("another row during the unload: %q", other)
	}
	if got := readTally(user, "tally"); got != "use: service tally is not available" {
		t.Fatalf("another row after the unload: %q", got)
	}
}

// A cleanup registering a cleanup is refused, as Rust's InactiveEffect
// (ctx.rs), and says so: the stack it would join is the one the unload
// took.
func TestACleanupRegisteringOneIsRefused(t *testing.T) {
	var written bytes.Buffer
	saved := rowOutput
	rowOutput = &written
	t.Cleanup(func() { rowOutput = saved })
	ran := false
	plugin := Define(Plugin[NoConfig]{Name: "nested", Apply: func(c *Ctx, _ NoConfig) error {
		c.Effect(func(context.Context) error {
			c.Effect(func(context.Context) error { ran = true; return nil })
			return nil
		})
		return nil
	}})
	rt := newRuntime([]*Definition{plugin})
	ctx := context.Background()
	t.Cleanup(func() { _ = rt.dispose(ctx) })
	if err := rt.load(ctx, "nested", "nested", nil, nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	if err := rt.unload(ctx, "nested"); err != nil {
		t.Fatal(err)
	}
	if got := written.String(); got != "rutis: row nested is not active: cleanup is not registered\n" {
		t.Fatalf("output: %q", got)
	}
	if ran {
		t.Fatal("the cleanup registered while unloading ran")
	}
}

// Withdrawing by the returned function and providing the name again with an
// equal value: the older withdrawal (called again, and on the stack) leaves
// the newer registration alone.
func TestAnOldWithdrawalLeavesANewRegistrationAlone(t *testing.T) {
	var during, seen string
	plugin := Define(Plugin[NoConfig]{Name: "again", Apply: func(c *Ctx, _ NoConfig) error {
		withdraw := c.Provide("tally", Tally{N: 5})
		withdraw()
		c.Provide("tally", Tally{N: 5})
		withdraw()
		during = readTally(c, "tally")
		c.Effect(func(context.Context) error { seen = readTally(c, "tally"); return nil })
		return nil
	}})
	rt := newRuntime([]*Definition{plugin})
	ctx := context.Background()
	t.Cleanup(func() { _ = rt.dispose(ctx) })
	if err := rt.load(ctx, "a", "again", nil, nil, nil, nil); err != nil {
		t.Fatal(err)
	}
	if err := rt.unload(ctx, "a"); err != nil {
		t.Fatal(err)
	}
	if during != "count 5" || seen != "count 5" {
		t.Fatalf("after the old withdrawal: %q; in the cleanup: %q", during, seen)
	}
}
