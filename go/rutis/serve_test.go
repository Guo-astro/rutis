package rutis_test

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"slices"
	"testing"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/advanced"
	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/testwait"
)

// A local channel given --id speaks the endpoint format and greets as that
// id (#811 R4).
func TestALocalChannelWithAnIDGreetsAsThatEndpoint(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	t.Setenv("RUTIS_CHANNEL_TOKEN", "secret")
	plugin := rutis.Define(rutis.Plugin[rutis.NoConfig]{Name: "any", Apply: func(*rutis.Ctx, rutis.NoConfig) error { return nil }})
	done := make(chan error, 1)
	go func() {
		done <- advanced.ServeArgs([]string{"tcp:" + listener.Addr().String(), "--id", "gorun", "--peer", "host", t.TempDir()}, plugin)
	}()
	conn, err := listener.Accept()
	if err != nil {
		t.Fatal(err)
	}
	// The token line, read whole before the session starts on conn.
	token := make([]byte, len("secret\n"))
	if _, err := io.ReadFull(conn, token); err != nil || string(token) != "secret\n" {
		t.Fatalf("token %q, %v", token, err)
	}
	host, err := peer.New(peer.NewLines(conn), peer.Options{
		Endpoint:     &peer.Endpoint{Local: "host", Expected: "gorun"},
		Capabilities: append([]string{"sync-wait", peer.EventsCapability}, peer.ValueCapabilities...),
		Plugins:      true,
		Dispatch: func(context.Context, string, string, []any) (any, error) {
			return nil, fmt.Errorf("not served")
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := host.Start(); err != nil {
		t.Fatal(err)
	}
	testwait.Within(t, "the runtime greets", host.Ready())
	if host.Greeting() == nil {
		t.Fatalf("no greeting: %v", host.Err())
	}
	if got := host.Greeting().Endpoint; got != "gorun" {
		t.Fatalf("greeted as %q", got)
	}
	if !host.Supports("runtime") {
		t.Fatal("the runtime does not declare runtime")
	}
	mounted, err := host.Call(context.Background(), "", "mount", []any{map[string]any{"announce": true, "runtime": "gorun"}}, false)
	if err != nil {
		t.Fatal(err)
	}
	var reply struct{ Features []string }
	if err := json.Unmarshal(mounted.(json.RawMessage), &reply); err != nil || !slices.Contains(reply.Features, "rows.v2") {
		t.Fatalf("mount answered %s", mounted)
	}
	// The host goes: the runtime's session ends and it stops.
	host.Close(nil)
	conn.Close()
	if err := testwait.Within(t, "the runtime stops", done); err != nil {
		t.Fatal(err)
	}
}

// The id a local channel greets with is checked before it is opened.
func TestALocalChannelRefusesAnInvalidID(t *testing.T) {
	plugin := rutis.Define(rutis.Plugin[rutis.NoConfig]{Name: "any", Apply: func(*rutis.Ctx, rutis.NoConfig) error { return nil }})
	err := advanced.ServeArgs([]string{"unix:/nowhere", "--id", "Go Run", t.TempDir()}, plugin)
	want := "invalid endpoint id Go Run: use lower-case letters, digits and -"
	if err == nil || err.Error() != want {
		t.Fatalf("got %v", err)
	}
}

// A local channel speaks the endpoint format only: without --id it is
// refused before it is opened.
func TestALocalChannelNeedsAnID(t *testing.T) {
	plugin := rutis.Define(rutis.Plugin[rutis.NoConfig]{Name: "any", Apply: func(*rutis.Ctx, rutis.NoConfig) error { return nil }})
	err := advanced.ServeArgs([]string{"unix:/nowhere", t.TempDir()}, plugin)
	want := "a local channel needs --id <endpoint>: unix:/nowhere"
	if err == nil || err.Error() != want {
		t.Fatalf("got %v", err)
	}
}
