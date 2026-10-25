package rutis

import (
	"context"
	"reflect"
	"strings"
	"testing"

	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// A service of this process called through a client whose field type is
// not the method's (localService → convert.Call), or through *Service,
// gets its arguments as peer values: a plugin definition is a peer.Plugin
// there, decoded back into the *Definition itself.
func TestALocalServiceGetsTheDefinitionItself(t *testing.T) {
	def := Define(Plugin[NoConfig]{Apply: func(*Ctx, NoConfig) error { return nil }})
	args, err := convert.ToPeerArgs([]any{def})
	if err != nil {
		t.Fatal(err)
	}
	if _, ok := args[0].(peer.Plugin); !ok {
		t.Fatalf("%T", args[0])
	}
	var got *Definition
	method := reflect.ValueOf(func(_ context.Context, d *Definition) error { got = d; return nil })
	if _, err := convert.Call(context.Background(), method, args); err != nil {
		t.Fatal(err)
	}
	if got != def {
		t.Fatalf("%p, want %p", got, def)
	}
	// Into anything else, it fails saying what it is.
	wrong := reflect.ValueOf(func(_ context.Context, s string) error { return nil })
	if _, err := convert.Call(context.Background(), wrong, args); err == nil || !strings.Contains(err.Error(), "plugin definition") {
		t.Fatalf("%v", err)
	}
}

// Messages about a plugin without a name do not print an empty one.
func TestANamelessPluginIsCalledSoInPanics(t *testing.T) {
	defer func() {
		message, _ := recover().(string)
		if message != "rutis: a plugin without a name has no Apply" {
			t.Fatalf("%q", message)
		}
	}()
	Define(Plugin[NoConfig]{})
}
