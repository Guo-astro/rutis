package rutistest_test

// Guarantee: sdk/testing-matches-runtime
// What the test tool says when the row is not running (#933): the line `dev`
// prints, the tool's own words for a name the running row has not got, and
// its limit on a call that never returns.

import (
	"testing"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/rutistest"
)

// A row that waits for a service nothing provides says what `dev` says, and
// starts once it arrives.
func TestARowThatWaitsSaysWhatDevSays(t *testing.T) {
	loaded := rutistest.Load(t, rutis.Define(rutis.Plugin[rutis.NoConfig]{
		Name:   "gated",
		Inject: []string{"later"},
		Apply:  func(*rutis.Ctx, rutis.NoConfig) error { return nil },
	}), nil, nil)
	// The row is not up: the host's line, the one `dev` prints.
	if line := loaded.Waiting(); line != "gated: waiting for later (no row provides it)" {
		t.Fatalf("waiting: %q", line)
	}
}
