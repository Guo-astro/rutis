package events

import (
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/testleak"
)

// TestMain runs the package's tests and fails when one leaves a goroutine
// running (#630).
func TestMain(m *testing.M) {
	testleak.Verify(m)
}
