package convert

import (
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// Callee is what a refused call names (#677): the service and method a
// caller called, or the object's type and method; all empty for a
// function.
type Callee struct {
	Service string
	// Object is the type name of an object passed by reference, "" for a
	// service or a function.
	Object string
	Method string
}

func (c Callee) label() string {
	switch {
	case c.Service != "":
		return c.Service + "." + c.Method
	case c.Object != "":
		return c.Object + "." + c.Method
	}
	return "the function"
}

func (c Callee) data(fields map[string]any) map[string]any {
	switch {
	case c.Service != "":
		fields["service"], fields["method"] = c.Service, c.Method
	case c.Object != "":
		fields["object"], fields["method"] = c.Object, c.Method
	}
	return fields
}

// tooMany is `<service>.<method> takes <n> argument(s), got <m>`, code
// bad-argument.
func tooMany(c Callee, expected, received int) error {
	return peer.NewCoded("BindingError", "bad-argument",
		c.data(map[string]any{"expected": expected, "received": received}),
		"%s takes %s, got %d", c.label(), plural(expected, "argument"), received)
}

// badArgument is `<service>.<method>: argument <i>: <why>` (i from 1), the
// decoder's own sentence verbatim, code bad-argument (design §4.5): rutis
// adds only the position.
func badArgument(c Callee, index int, t reflect.Type, arg any, cause error) error {
	return peer.NewCoded("BindingError", "bad-argument",
		c.data(map[string]any{"argument": index + 1, "expected": Expected(t)}),
		"%s: argument %d: %s", c.label(), index+1, decoderSaid(cause, arg, t))
}

// badResult is `<service>.<method>: the result: <why>`, the decoder's own
// sentence verbatim, code bad-result (design §5.3).
func badResult(c Callee, arg any, t reflect.Type, cause error) error {
	return peer.NewCoded("BindingError", "bad-result",
		map[string]any{"service": c.Service, "method": c.Method, "expected": Expected(t)},
		"%s: the result: %s", c.label(), decoderSaid(cause, arg, t))
}

// decoderSaid is the decoder's own sentence: the error's own text, verbatim.
// A failure rutis made itself, which has no decoder behind it (`a list
// cannot be decoded as string`), is what it says too.
func decoderSaid(cause error, arg any, t reflect.Type) string {
	if cause != nil {
		// The decoder's own words: the innermost error, past rutis's own
		// wrapper (`cannot decode x as int: json: …`).
		for {
			inner := errors.Unwrap(cause)
			if inner == nil {
				break
			}
			cause = inner
		}
		return cause.Error()
	}
	return "cannot decode " + shownJSONArg(arg) + " as " + t.String()
}

func shownJSONArg(arg any) string {
	if raw, ok := arg.(json.RawMessage); ok {
		return string(raw)
	}
	return fmt.Sprint(arg)
}

// NoMethod is `<service> has no method <method> (methods: a, b)`, code
// no-method, for a service; for an object `<Type> has no method …`, or
// `the object has no method …` when its type has no name.
func NoMethod(c Callee, methods []string) error {
	return NoMethodOf(c, methods, "")
}

// NoMethodOf is [NoMethod] for a service another row provides: it says
// whose the name is (`weather has no method today (methods: ask); it is
// provided by row store`) and puts the row in `data.provider` (#937), so a
// developer looking for their own service sees the name is taken.
// `provider`: the providing row's name; "" when this side does not know it.
func NoMethodOf(c Callee, methods []string, provider string) error {
	sorted := append([]string(nil), methods...)
	sort.Strings(sorted)
	listed := "no methods"
	if len(sorted) > 0 {
		listed = "methods: " + strings.Join(sorted, ", ")
	}
	data := map[string]any{"method": c.Method, "methods": sorted}
	owner := c.Service
	if c.Service != "" {
		data["service"] = c.Service
	} else {
		owner = c.Object
		if owner == "" {
			owner = "the object"
			data["object"] = nil
		} else {
			data["object"] = c.Object
		}
	}
	message := fmt.Sprintf("%s has no method %s (%s)", owner, c.Method, listed)
	if provider != "" {
		message += "; it is provided by row " + provider
		data["provider"] = provider
	}
	return peer.NewCoded("BindingError", "no-method", data, "%s", message)
}

func plural(n int, word string) string {
	if n == 1 {
		return "1 " + word
	}
	return strconv.Itoa(n) + " " + word + "s"
}

func article(word string) string {
	if word != "" && strings.ContainsRune("aeiouAEIOU", rune(word[0])) {
		return "an " + word
	}
	return "a " + word
}

// Expected is what a parameter of type t takes, in words every language
// shares: `a string`, `an integer`, `a list`; a named struct by its name
// (`a Forecast`).
func Expected(t reflect.Type) string {
	for t.Kind() == reflect.Pointer {
		if t == futureType {
			return "an asynchronous result"
		}
		if t == functionType {
			return "a function"
		}
		t = t.Elem()
	}
	switch t {
	case reflect.TypeFor[time.Time]():
		return "an instant"
	case bigIntType:
		return "a bigint"
	case errType:
		return "an error"
	}
	switch t.Kind() {
	case reflect.Bool:
		return "a boolean"
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64, reflect.Uintptr:
		return "an integer"
	case reflect.Float32, reflect.Float64:
		return "a number"
	case reflect.String:
		return "a string"
	case reflect.Slice, reflect.Array:
		if t.Elem().Kind() == reflect.Uint8 {
			return "bytes"
		}
		return "a list"
	case reflect.Map:
		if t.Elem().Kind() == reflect.Struct && t.Elem().NumField() == 0 {
			return "a set"
		}
		if t.Key().Kind() == reflect.String {
			return "an object"
		}
		return "a map"
	case reflect.Func:
		return "a function"
	case reflect.Struct:
		if t.Name() != "" {
			return article(t.Name())
		}
		return "an object"
	}
	return "a value"
}

// actual is the argument as a refusal shows it: a short value (`"x"`,
// `1.5`, `true`), or what it is (`a list`, `an object`, `a function`).
func actual(arg any, t reflect.Type, cause error) string {
	raw, isJSON := arg.(json.RawMessage)
	if !isJSON {
		switch v := arg.(type) {
		case *peer.RemoteObject:
			// A struct that cannot take it says how one would (#687 §八).
			if errors.Is(cause, ErrNotForObjects) {
				return "an object (" + ErrNotForObjects.Error() + ")"
			}
			return "an object"
		case *big.Int:
			return abbreviateText(v.String()) + " (out of range for " + t.String() + ")"
		case []any:
			return "a list of " + plural(len(v), "item")
		case []byte:
			return plural(len(v), "byte")
		}
		shown := shownValue(arg)
		if cause != nil {
			reason := cause.Error()
			if strings.Contains(reason, "would lose") {
				// Keys or elements that become equal in Go.
				return shown + " (" + reason + ")"
			}
			if _, hint, ok := strings.Cut(reason, ": use "); ok {
				return shown + " (use " + hint + ")"
			}
		}
		return shown
	}
	var typed *json.UnmarshalTypeError
	if errors.As(cause, &typed) {
		if typed.Field != "" {
			return "an object whose " + typed.Field + " is " + jsonKind(typed.Value)
		}
		if number, ok := strings.CutPrefix(typed.Value, "number "); ok {
			if isInteger(number) {
				return abbreviateText(number) + " (out of range for " + t.String() + ")"
			}
		}
	}
	return shownJSON(raw)
}

func isInteger(text string) bool {
	_, ok := new(big.Int).SetString(text, 10)
	return ok
}

// jsonKind is what encoding/json's UnmarshalTypeError.Value names, in
// words: `a string`, `a number` (or the number itself).
func jsonKind(value string) string {
	switch value {
	case "string":
		return "a string"
	case "number":
		return "a number"
	case "bool":
		return "a boolean"
	case "array":
		return "a list"
	case "object":
		return "an object"
	}
	if number, ok := strings.CutPrefix(value, "number "); ok {
		return abbreviateText(number)
	}
	return value
}

func shownJSON(raw json.RawMessage) string {
	text := strings.TrimSpace(string(raw))
	if text == "" {
		return "nothing"
	}
	switch text[0] {
	case '[':
		return "a list"
	case '{':
		return "an object"
	}
	return abbreviateText(text)
}

func shownValue(v any) string {
	switch v.(type) {
	case *peer.RemoteFunction, peer.Func:
		return "a function"
	case *peer.RemoteFuture:
		return "an asynchronous result"
	case *peer.RemotePlugin, peer.Plugin:
		return "a plugin definition"
	case peer.Object:
		return "an object"
	case []any:
		return "a list"
	case map[string]any:
		return "an object"
	case string, float64, bool:
		data, _ := json.Marshal(v)
		return abbreviateText(string(data))
	}
	return describe(v)
}
