// Package convert moves Go values across the session: Go values to wire
// values (peer values) and back into typed Go values, and calls Go
// functions and methods with wire arguments.
//
// Data (numbers, strings, slices, maps, structs) is copied, checked value
// by value; time.Time, []byte, *big.Int, maps with keys other than
// strings, map[K]struct{} and the rutis value types cross as tagged
// values (#308); functions, Ref(v) and the far end's references cross by
// reference.
package convert

import (
	"context"
	"fmt"
	"reflect"
	"sync"
	"unicode"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

var (
	ctxType      = reflect.TypeFor[context.Context]()
	errType      = reflect.TypeFor[error]()
	anyType      = reflect.TypeFor[any]()
	futureType   = reflect.TypeFor[*Future]()
	functionType = reflect.TypeFor[*Function]()
)

// ── Names ────────────────────────────────────────────────────────

// WireName is the name a Go method or field has on the wire: the first
// letter lowercased; a leading run of capitals lowercased as a whole,
// except that when a lowercase letter follows the run, its last capital
// starts the next word (URLFor -> urlFor, HTTPServer -> httpServer).
func WireName(name string) string {
	runes := []rune(name)
	n := 0
	for n < len(runes) && unicode.IsUpper(runes[n]) {
		n++
	}
	if n == 0 {
		return name
	}
	lower := n
	if n > 1 && n < len(runes) && unicode.IsLower(runes[n]) {
		lower = n - 1
	}
	for i := 0; i < lower; i++ {
		runes[i] = unicode.ToLower(runes[i])
	}
	return string(runes)
}

// ── References ───────────────────────────────────────────────────

type ref struct{ value any }

// Ref marks v to cross by reference: the far end gets a proxy calling v's
// exported methods.
func Ref(v any) any { return ref{v} }

// Function is a function of the far end, received where a value of any
// type was expected.
type Function struct{ remote *peer.RemoteFunction }

// Call calls the function and decodes its result into `result` (a
// pointer, or nil).
func (f *Function) Call(ctx context.Context, result any, args ...any) error {
	converted, err := ToPeerArgs(args)
	if err != nil {
		return err
	}
	value, err := f.remote.Call(ctx, converted, true)
	if err != nil {
		return err
	}
	return Decode(value, result)
}

// Future is an asynchronous result of the far end.
type Future struct{ remote *peer.RemoteFuture }

// Await waits for the result and decodes it into `result` (a pointer, or
// nil); ctx cancels the wait.
func (f *Future) Await(ctx context.Context, result any) error {
	value, err := f.remote.Await(ctx)
	if err != nil {
		return err
	}
	if value, err = peer.Settle(ctx, value); err != nil {
		return err
	}
	return Decode(value, result)
}

// ── Calls ────────────────────────────────────────────────────────

// Call calls the Go function `fn` with wire arguments: a leading
// context.Context parameter gets ctx; the others are decoded by position
// (missing ones are zero values, extra ones an error); a trailing error
// result is the call's error. A panic becomes a peer.PanicError.
func Call(ctx context.Context, fn reflect.Value, args []any) (any, error) {
	return peer.Protect(func() (any, error) {
		ft := fn.Type()
		for len(args) > 0 {
			if _, undefined := args[len(args)-1].(peer.UndefinedType); !undefined {
				break
			}
			args = args[:len(args)-1]
		}
		in := make([]reflect.Value, 0, ft.NumIn())
		first := 0
		if ft.NumIn() > 0 && ft.In(0) == ctxType {
			in = append(in, reflect.ValueOf(&ctx).Elem())
			first = 1
		}
		fixed := ft.NumIn() - first
		if ft.IsVariadic() {
			fixed--
		}
		if !ft.IsVariadic() && len(args) > fixed {
			return nil, fmt.Errorf("%d arguments given, %d expected", len(args), fixed)
		}
		for i := 0; i < fixed; i++ {
			var arg any = peer.Undefined
			if i < len(args) {
				arg = args[i]
			}
			value, err := FromPeer(arg, ft.In(first+i))
			if err != nil {
				return nil, fmt.Errorf("argument %d: %w", i, err)
			}
			in = append(in, value)
		}
		if ft.IsVariadic() {
			elem := ft.In(ft.NumIn() - 1).Elem()
			for i := fixed; i < len(args); i++ {
				value, err := FromPeer(args[i], elem)
				if err != nil {
					return nil, fmt.Errorf("argument %d: %w", i, err)
				}
				in = append(in, value)
			}
		}
		out := fn.Call(in)
		return Results(out)
	})
}

// Results converts a Go call's results: a trailing error is the error,
// and at most one other result is the value.
func Results(out []reflect.Value) (any, error) {
	if n := len(out); n > 0 && out[n-1].Type() == errType {
		if !out[n-1].IsNil() {
			return nil, out[n-1].Interface().(error)
		}
		out = out[:n-1]
	}
	switch len(out) {
	case 0:
		return peer.Undefined, nil
	case 1:
		return ToPeer(out[0].Interface())
	}
	return nil, fmt.Errorf("a function returns at most one value and an error, not %d values", len(out))
}

// CheckSignature reports whether t can be called across processes: an
// optional leading context.Context, and at most one result before an
// optional trailing error.
func CheckSignature(t reflect.Type, needError bool) error {
	n := t.NumOut()
	hasError := n > 0 && t.Out(n-1) == errType
	if hasError {
		n--
	}
	if n > 1 {
		return fmt.Errorf("%s returns %d values: at most one value and an error cross between processes", t, n)
	}
	if needError && !hasError {
		return fmt.Errorf("%s must return an error last", t)
	}
	return nil
}

// MakeFunc makes a Go function of type t from `call`: a leading
// context.Context argument is its ctx (nil panics), the other arguments
// are converted, and the result is decoded as t's result. Without an
// error result, a failure panics.
func MakeFunc(t reflect.Type, call func(ctx context.Context, args []any) (any, error)) reflect.Value {
	return reflect.MakeFunc(t, func(in []reflect.Value) []reflect.Value {
		ctx := context.Background()
		first := 0
		if t.NumIn() > 0 && t.In(0) == ctxType {
			first = 1
			if in[0].IsNil() {
				panic("rutis: a nil context: pass the context.Context your caller gave you")
			}
			ctx = in[0].Interface().(context.Context)
		}
		var args []any
		for i := first; i < len(in); i++ {
			if t.IsVariadic() && i == len(in)-1 {
				for j := 0; j < in[i].Len(); j++ {
					args = append(args, in[i].Index(j).Interface())
				}
				continue
			}
			args = append(args, in[i].Interface())
		}
		var value any
		converted, err := ToPeerArgs(args)
		if err == nil {
			value, err = call(ctx, converted)
		}
		if err == nil {
			value, err = peer.Settle(ctx, value)
		}
		return results(t, value, err)
	})
}

func results(t reflect.Type, value any, err error) []reflect.Value {
	n := t.NumOut()
	hasError := n > 0 && t.Out(n-1) == errType
	out := make([]reflect.Value, n)
	for i := range out {
		out[i] = reflect.Zero(t.Out(i))
	}
	if err == nil && n > 0 && (!hasError || n == 2) {
		decoded, decodeErr := FromPeer(value, t.Out(0))
		if decodeErr != nil {
			err = decodeErr
		} else {
			out[0] = decoded
		}
	}
	if err != nil {
		if !hasError {
			panic(err)
		}
		out[n-1] = reflect.ValueOf(&err).Elem()
	}
	return out
}

// ── Objects ──────────────────────────────────────────────────────

type object struct{ rv reflect.Value }

// Object is v as an object the far end calls: its exported methods by
// wire name, its exported fields as properties.
func Object(v any) peer.Object { return object{reflect.ValueOf(v)} }

func (o object) Call(ctx context.Context, method string, args []any) (any, error) {
	index, ok := Methods(o.rv.Type())[method]
	if !ok {
		return nil, fmt.Errorf("unknown method %s", method)
	}
	return Call(ctx, o.rv.Method(index), args)
}

func (o object) Get(_ context.Context, property string) (any, error) {
	return Property(o.rv, property)
}

// TypeName is the name of v's type, for messages (#675).
func (o object) TypeName() string {
	if !o.rv.IsValid() {
		return ""
	}
	t := o.rv.Type()
	for t.Kind() == reflect.Pointer {
		t = t.Elem()
	}
	return t.Name()
}

// Property reads the exported field of v named `name` on the wire (its
// json name, or its wire name).
func Property(v reflect.Value, name string) (any, error) {
	for v.Kind() == reflect.Pointer || v.Kind() == reflect.Interface {
		if v.IsNil() {
			return nil, fmt.Errorf("no property %s", name)
		}
		v = v.Elem()
	}
	if v.Kind() == reflect.Struct {
		if field, ok := fieldByJSONName(v.Type(), name); ok {
			return ToPeer(v.FieldByIndex(field.Index).Interface())
		}
		for _, field := range reflect.VisibleFields(v.Type()) {
			if field.IsExported() && WireName(field.Name) == name {
				return ToPeer(v.FieldByIndex(field.Index).Interface())
			}
		}
	}
	return nil, fmt.Errorf("no property %s", name)
}

var methodTables sync.Map

// Methods maps the wire names of t's exported methods to their indexes.
func Methods(t reflect.Type) map[string]int {
	if cached, ok := methodTables.Load(t); ok {
		return cached.(map[string]int)
	}
	table := map[string]int{}
	for i := 0; i < t.NumMethod(); i++ {
		table[WireName(t.Method(i).Name)] = i
	}
	methodTables.Store(t, table)
	return table
}
