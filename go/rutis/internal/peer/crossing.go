package peer

import (
	"encoding/json"
	"errors"
	"fmt"
	"reflect"
	"strings"
)

// What a call cannot send (#675): a value the far end cannot take, or a
// message over the limit, fails only that call, on the side that would
// send it, and the error says which call, which argument (or the result)
// and why. The session goes on.

// invocation is a call for messages: `service.method` (the target without
// a `host:` prefix or a scope label), or the method alone for a control
// call (no target).
func invocation(target, method string) string {
	service := strings.TrimPrefix(target, "host:")
	if i := strings.IndexByte(service, 0); i >= 0 {
		service = service[:i]
	}
	if service == "" {
		return method
	}
	return service + "." + method
}

// describeRequest is an outgoing call of `op` with `fields`, for messages.
func describeRequest(op string, fields map[string]any) string {
	method, _ := fields["method"].(string)
	switch op {
	case "invoke":
		target, _ := fields["target"].(string)
		return invocation(target, method)
	case "call":
		if method != "" {
			return "the method " + method
		}
		return "a function"
	}
	return op
}

// describeJob is an incoming call, for messages about its result; "" when
// it is not known.
func describeJob(j *job) string {
	switch {
	case j.op == "invoke" && j.method != nil:
		return invocation(j.target, *j.method)
	case j.op == "get":
		return "the property " + j.property
	case j.op == "call" && j.method != nil:
		return "the method " + *j.method
	case j.op == "call":
		return "a function"
	}
	return ""
}

// ofArgument is `err` as one of argument `index` (from 0) of `call`:
// `<call>: argument <i> cannot be sent: <why>`, code bad-value (#677).
func ofArgument(err error, index int, call string) error {
	return &ValueError{Message: fmt.Sprintf("%s: argument %d cannot be sent: %s", call, index+1, err.Error()), cause: err,
		argument: index + 1, part: "argument", limit: limitOf(err)}
}

// ofResult is `err` as one of the result of `call` ("" when not known):
// `<call>: the result cannot be sent: <why>`.
func ofResult(err error, call string) error {
	if call == "" {
		return &ValueError{Message: "the result cannot be sent: " + err.Error(), cause: err, part: "result", limit: limitOf(err)}
	}
	return &ValueError{Message: fmt.Sprintf("%s: the result cannot be sent: %s", call, err.Error()), cause: err,
		part: "result", limit: limitOf(err)}
}

// ofArguments is `err` as one of the arguments of `call` together.
func ofArguments(err error, call string) error {
	return &ValueError{Message: fmt.Sprintf("%s: the arguments cannot be sent: %s", call, err.Error()), cause: err,
		part: "arguments", limit: limitOf(err)}
}

// tooLarge is a message over the limit.
type tooLarge struct{ size int }

func (e *tooLarge) Error() string { return overLimit(e.size) }

// limitOf is the limit `err` is over, 0 when it is not a message over it.
func limitOf(err error) int {
	var large *tooLarge
	if errors.As(err, &large) {
		return MaxMessage
	}
	return 0
}

// overLimit says why a message of `size` bytes is not sent.
func overLimit(size int) string {
	const mib = 1 << 20
	text := fmt.Sprintf("the message would be %d bytes, over the limit of %d bytes", size, MaxMessage)
	if MaxMessage%mib == 0 {
		text += fmt.Sprintf(" (%d MiB)", MaxMessage/mib)
	}
	return text
}

// largest is the index of the largest of a call's encoded arguments, -1
// when they are not a list.
func largest(encoded map[string]any) int {
	var sizes []int
	switch items := encoded["value"].(type) {
	case []any:
		for _, item := range items {
			data, _ := marshal(item)
			sizes = append(sizes, len(data))
		}
	case json.RawMessage:
		var list []json.RawMessage
		if json.Unmarshal(items, &list) != nil {
			return -1
		}
		for _, item := range list {
			sizes = append(sizes, len(item))
		}
	}
	index := -1
	for i, size := range sizes {
		if index < 0 || size > sizes[index] {
			index = i
		}
	}
	return index
}

// typeName is the name of a value's type, for messages: what an Object
// says it is (TypeName), else its Go type's name without the package.
func typeName(value any) string {
	if named, ok := value.(interface{ TypeName() string }); ok {
		return named.TypeName()
	}
	t := reflect.TypeOf(value)
	for t != nil && t.Kind() == reflect.Pointer {
		t = t.Elem()
	}
	if t == nil {
		return ""
	}
	return t.Name()
}

// objectRefused says why an object is not sent to a far end that did not
// declare `objects`; p.mu is held.
func (p *Peer) objectRefused(value any) error {
	far := "the far end"
	if p.greeting != nil {
		far = p.greeting.Endpoint
	}
	what := "an object with methods"
	if name := typeName(value); name != "" {
		what += " (" + name + ")"
	}
	return valueErrorf("%s cannot cross to %s; pass plain data or a function instead", what, far)
}

// encodeArgs encodes a call's arguments; p.mu is held. An error names the
// argument of `call` it is of.
func (p *Peer) encodeArgs(args []any, grants *[]int64, origin []string, call string) (map[string]any, error) {
	encoded, err := p.encode(args, grants, origin, -1)
	if err == nil {
		return encoded, nil
	}
	// Each argument alone says which one it is.
	for i, arg := range args {
		var tried []int64
		_, failed := p.encode(arg, &tried, origin, 0)
		p.rollback(tried)
		if failed != nil {
			return nil, ofArgument(failed, i, call)
		}
	}
	return nil, ofArguments(err, call)
}
