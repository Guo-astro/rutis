//go:build unix

// guarantee: bridge/large-and-deep-values
// A real cross-language call for `chunked` (#1363): this Go session as the
// host and the Node runtime's conformance fixture as the far end. A
// parameter and a result over 16 MiB cross whole, and the method runs once.

package peer

import (
	"bytes"
	"context"
	"encoding/json"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

func repoRoot(t *testing.T) string {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("the test's source path is not known")
	}
	return filepath.Clean(filepath.Join(filepath.Dir(file), "..", "..", "..", ".."))
}

func TestNodeCarriesValuesOverTheMessageLimitWhole(t *testing.T) {
	node, err := exec.LookPath("node")
	if err != nil {
		t.Skip("node is not on PATH")
	}
	root := repoRoot(t)
	fixture := filepath.Join(root, "node", "rutis-runtime", "test", "fixtures", "conformance-session.mjs")
	modules := filepath.Join(root, "node", "rutis-runtime", "node_modules")
	if _, err := os.Stat(fixture); err != nil {
		t.Skipf("the Node runtime fixture: %v", err)
	}
	if _, err := os.Stat(modules); err != nil {
		t.Skip("node/rutis-runtime/node_modules is not installed")
	}
	// A short path: macOS's sockaddr_un allows 104 bytes, and t.TempDir()
	// under the runner's long temporary directory is over it.
	directory, err := os.MkdirTemp("/tmp", "rutis-chunk-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(directory) })
	socket := filepath.Join(directory, "node.sock")
	listener, err := net.Listen("unix", socket)
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	if unix, ok := listener.(*net.UnixListener); ok {
		unix.SetDeadline(time.Now().Add(60 * time.Second))
	}
	var output bytes.Buffer
	command := exec.Command(node, fixture, socket)
	command.Dir = filepath.Join(root, "node", "rutis-runtime")
	command.Stdout = &output
	command.Stderr = &output
	if err := command.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_ = command.Process.Kill()
		_, _ = command.Process.Wait()
	})
	connection, err := listener.Accept()
	if err != nil {
		t.Fatalf("the Node runtime: %v\n%s", err, output.String())
	}
	defer connection.Close()
	host, err := New(NewLines(connection), Options{
		Endpoint:     &Endpoint{Local: "main", Expected: "node"},
		Capabilities: TestCapabilities,
		Dispatch:     func(ctx context.Context, target, method string, args []any) (any, error) { return nil, nil },
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := host.Start(); err != nil {
		t.Fatal(err)
	}
	defer host.Close(nil)
	testwait.Within(t, "the handshake", host.Ready())

	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	big := strings.Repeat("x", 17*1024*1024)
	tallyValue, err := host.Call(ctx, "conformance", "tally", []any{big}, true)
	if err != nil {
		t.Fatalf("tally: %v\n%s", err, output.String())
	}
	var tally struct {
		Calls int `json:"calls"`
		Size  int `json:"size"`
	}
	if err := json.Unmarshal(tallyValue.(json.RawMessage), &tally); err != nil {
		t.Fatalf("tally result: %v", err)
	}
	if tally.Calls != 1 || tally.Size != len(big) {
		t.Fatalf("tally = %+v, want one call with %d bytes", tally, len(big))
	}
	value, err := host.Call(ctx, "conformance", "big", []any{len(big)}, true)
	if err != nil {
		t.Fatalf("big: %v\n%s", err, output.String())
	}
	if got := text(value); len(got) != len(big) {
		t.Fatalf("the result came back %d bytes, want %d", len(got), len(big))
	}
	after, err := host.Call(ctx, "conformance", "echo", []any{"after"}, true)
	if err != nil || text(after) != "after" {
		t.Fatalf("after: %v %v", after, err)
	}
	if hint := host.Err(); hint != nil {
		t.Fatalf("the session ended: %v\n%s", hint, output.String())
	}
}
