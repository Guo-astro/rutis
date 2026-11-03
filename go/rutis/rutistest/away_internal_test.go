package rutistest

// The tool's own words (#933): the failure for a call that runs out of its
// limit, and the nearest name a misspelt service has.

import (
	"errors"
	"testing"
	"time"
)

func TestACallThatRunsOutOfItsLimit(t *testing.T) {
	err := timeoutError("stuck", "wait", 50*time.Millisecond)
	var failed *Error
	if !errors.As(err, &failed) {
		t.Fatalf("not a *Error: %#v", err)
	}
	if failed.Code != "timeout" {
		t.Fatalf("code: %q", failed.Code)
	}
	// Its own words: an underlying error of the plugin's is never rewritten.
	if failed.Line != "stuck.wait() did not return within 0.1s" {
		t.Fatalf("line: %q", failed.Line)
	}
	if failed.Data["service"] != "stuck" || failed.Data["method"] != "wait" {
		t.Fatalf("data: %+v", failed.Data)
	}
	if got := seconds(30 * time.Second); got != "30s" {
		t.Fatalf("seconds: %q", got)
	}
}

func TestTheNearestNameIsTheOneRutisWouldName(t *testing.T) {
	known := []string{"echo", "later", "weather"}
	cases := map[string]string{
		"ecoh":    "echo",    // a swap of neighbours, one edit
		"waether": "weather", // two edits, for 7 characters
		"nothing": "",
		"ec":      "",
	}
	for name, want := range cases {
		if got := closest(name, known); got != want {
			t.Fatalf("closest(%q) = %q, want %q", name, got, want)
		}
	}
}
