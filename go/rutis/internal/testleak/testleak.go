// Package testleak checks that a package's tests leave no goroutine running
// (#630). Go has no controlled scheduler, so a goroutine a test forgot to
// end is found by its stack: goleak waits for the goroutines it finds to
// finish, and names those that do not.
//
// Every package of the SDK with tests calls Verify from its TestMain. A
// package whose tests keep a shared rutis-host passes SharedHost as well:
// that session belongs to the test process, not to a test. internal/runtime
// has none yet: its tests leave the WebSocket listeners they start running,
// as listenWebSocket has no way to stop one (#1245).
package testleak

import (
	"testing"

	"go.uber.org/goleak"
)

// Verify runs m and exits non-zero when a goroutine is still running after
// the tests. The runtime's own goroutines, the testing package's and this
// check's are not the tests'.
func Verify(m *testing.M, opts ...goleak.Option) {
	goleak.VerifyTestMain(m, opts...)
}

// SharedHost ignores the goroutines a shared rutistest host keeps from the
// first Load until the test process ends: the session with rutis-host, its
// reader and its write-back loop, and the wait for the host process. Without
// this every package that uses the test tool would report them, and a leak
// of any other goroutine is still found. Each is matched by a function
// anywhere in its stack: the top of the stack is where it waits, not the
// function that keeps it.
func SharedHost() []goleak.Option {
	return []goleak.Option{
		goleak.IgnoreAnyFunction("github.com/arcships/rutis/go/rutis/rutistest.serveTest"),
		goleak.IgnoreAnyFunction("github.com/arcships/rutis/go/rutis/rutistest.waitFor"),
		goleak.IgnoreAnyFunction("github.com/arcships/rutis/go/rutis/internal/peer.(*Peer).read"),
		goleak.IgnoreAnyFunction("github.com/arcships/rutis/go/rutis/internal/peer.(*Peer).writeBackLoop"),
	}
}
