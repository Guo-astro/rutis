package {{package}}

import (
	"context"
	"testing"

	"github.com/arcships/rutis/go/rutis/rutistest"
)

// The plugin in its real runtime, no host needed (step 6 of the guide).
func TestGreeterGreets(t *testing.T) {
	loaded := rutistest.Load(t, Plugin, Config{Greeting: "Hi"}, nil)
	var greeting string
	if err := loaded.Service("greeter").Call(context.Background(), "hello", &greeting, "Ada"); err != nil {
		t.Fatal(err)
	}
	if greeting != "Hi, Ada!" {
		t.Fatalf("got %q", greeting)
	}
}
