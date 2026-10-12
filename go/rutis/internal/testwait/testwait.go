// Package testwait holds the wait helpers of the Go tests and of rutistest
// (quality standard Q7.1, #231).
//
// A timeout here only guards against a hang (HangGuard); it never stands in
// for an event. Wait for what the test is about: a channel, a reply, a
// state. Poll (Eventually) only where nothing can be waited on. See
// docs/design-test-clocks-2026-10-11.md §3.
package testwait

import (
	"testing"
	"time"
)

// HangGuard is how long a wait lasts before the test fails as hung. It
// only guards against hangs, and is never what a test measures.
const HangGuard = 10 * time.Second

// poll is how often Eventually looks again.
const poll = 10 * time.Millisecond

// Within is the first value received from ch, unless none comes within
// HangGuard: then the test fails, naming what.
func Within[T any](t testing.TB, what string, ch <-chan T) T {
	t.Helper()
	timer := time.NewTimer(HangGuard)
	defer timer.Stop()
	select {
	case value := <-ch:
		return value
	case <-timer.C:
		t.Fatalf("timed out (%v): %s", HangGuard, what)
		var zero T
		return zero
	}
}

// Do is f's result, f run on its own goroutine, unless it takes longer
// than HangGuard: then the test fails, naming what. For a blocking call
// that takes no deadline of its own.
func Do[T any](t testing.TB, what string, f func() T) T {
	t.Helper()
	done := make(chan T, 1)
	go func() { done <- f() }()
	return Within(t, what, done)
}

// Eventually waits until check() is true, looking every 10 ms, within
// HangGuard; else the test fails, naming what and, when describe is not
// nil, the state describe() gives.
func Eventually(t testing.TB, what string, check func() bool, describe func() string) {
	t.Helper()
	deadline := time.Now().Add(HangGuard)
	for !check() {
		if time.Now().After(deadline) {
			state := ""
			if describe != nil {
				state = "\nstate: " + describe()
			}
			t.Fatalf("timed out (%v): %s%s", HangGuard, what, state)
		}
		time.Sleep(poll)
	}
}
