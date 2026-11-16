package rutistest_test

import (
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/testleak"
)

// TestMain runs the package's tests and fails when one leaves a goroutine
// running (#630). The shared host is the test process's, not a test's.
func TestMain(m *testing.M) {
	testleak.Verify(m, testleak.SharedHost()...)
}
