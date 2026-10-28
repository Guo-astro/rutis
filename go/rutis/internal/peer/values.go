package peer

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"math/big"
	"strconv"
	"strings"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/values"
)

// The tagged values beyond JSON data (#308), as this package holds them:
//
//	instant   time.Time (UTC when received)
//	datetime  values.DateTime
//	date      values.Date
//	time      values.TimeOfDay
//	bytes     []byte
//	map       Map
//	set       Set
//	bigint    *big.Int
//	decimal   values.Decimal
//	uuid      values.UUID
//
// Each is sent only to a far end that declared `value-<tag>`; to any
// other, the call carrying it fails and the session goes on.

// ValueCapabilities are the capabilities of the tagged values: a side
// declaring one takes values of that tag. This side takes them all.
var ValueCapabilities = func() []string {
	out := make([]string, len(values.Tags))
	for i, tag := range values.Tags {
		out[i] = "value-" + tag
	}
	return out
}()

// Map is a map's entries in the sender's order (tag `map`).
type Map []MapEntry

// MapEntry is an entry of a Map.
type MapEntry struct{ Key, Value any }

// Set is a set's elements in the sender's order (tag `set`).
type Set []any

// ValueError is a value this side cannot send or take: only the call
// carrying it fails, and the session goes on.
type ValueError struct {
	Message string
	// What it says more of (ErrTooDeep, say), for errors.Is.
	cause error
	// Which part of a call it is of (#677): "argument" (argument, from
	// 1), "arguments", "result"; "" when not known. limit is the message
	// limit it is over, 0 when it is not over it.
	argument int
	part     string
	limit    int
}

func (e *ValueError) Error() string { return e.Message }

// Code is bad-value: a value that cannot cross (#677).
func (e *ValueError) Code() string { return "bad-value" }

// Data is {argument?, part, limit?}, nil when the part is not known.
func (e *ValueError) Data() any {
	if e.part == "" {
		return nil
	}
	data := map[string]any{"part": e.part}
	if e.argument > 0 {
		data["argument"] = e.argument
	}
	if e.limit > 0 {
		data["limit"] = e.limit
	}
	return data
}

// Unwrap is the error it says more of, if any.
func (e *ValueError) Unwrap() error { return e.cause }

// Name is the name it crosses with.
func (e *ValueError) Name() string { return "Error" }

func valueErrorf(format string, args ...any) error {
	return &ValueError{Message: fmt.Sprintf(format, args...)}
}

// tagOf is the tag of a tagged value, "" for any other.
func tagOf(value any) string {
	switch value.(type) {
	case time.Time:
		return "instant"
	case values.DateTime:
		return "datetime"
	case values.Date:
		return "date"
	case values.TimeOfDay:
		return "time"
	case []byte:
		return "bytes"
	case Map:
		return "map"
	case Set:
		return "set"
	case *big.Int:
		return "bigint"
	case values.Decimal:
		return "decimal"
	case values.UUID:
		return "uuid"
	}
	return ""
}

// describe is what a value is, for errors: "null", "an instant", ...
func describe(value any) string {
	switch v := value.(type) {
	case nil:
		return "null"
	case UndefinedType:
		return "undefined"
	case json.RawMessage:
		text := bytes.TrimSpace(v)
		if len(text) == 0 {
			return "null"
		}
		switch text[0] {
		case 'n':
			return "null"
		case 't', 'f':
			return "a boolean"
		case '"':
			return "a string"
		case '[':
			return "an array"
		case '{':
			return "an object"
		}
		return "a number"
	case string:
		return "a string"
	case bool:
		return "a boolean"
	case int, int8, int16, int32, int64, uint, uint8, uint16, uint32, uint64, float32, float64:
		return "a number"
	case []any:
		return "an array"
	case map[string]any, error:
		return "an object"
	case Func, Object, *RemoteFunction, *RemoteFuture, *RemoteObject:
		return "a reference"
	case *Signal:
		return "a signal"
	case []byte:
		return "bytes"
	}
	switch tag := tagOf(value); tag {
	case "":
		return fmt.Sprintf("a %T", value)
	case "instant":
		return "an instant"
	default:
		return "a " + tag
	}
}

// tagged encodes a tagged value other than a map or set as its text.
func tagged(value any) (map[string]any, error) {
	var text string
	var err error
	switch v := value.(type) {
	case time.Time:
		text, err = values.FormatInstant(v)
	case values.DateTime, values.Date, values.TimeOfDay:
		if err = values.Check(v); err == nil {
			text = fmt.Sprint(v)
		}
	case []byte:
		text = values.FormatBytes(v)
	case *big.Int:
		text, err = values.FormatBigInt(v)
	case values.Decimal:
		text, err = string(v), values.CheckDecimal(string(v))
	case values.UUID:
		text, err = string(v), values.CheckUUID(string(v))
	}
	if err != nil {
		return nil, &ValueError{Message: err.Error()}
	}
	return map[string]any{"type": tagOf(value), "value": text}, nil
}

// ── Map keys and set elements (design §5) ────────────────────────────

// keyOf is a map key or set element as the languages compare it: numbers
// by value (`1` and `1.0` are one key, as are `0` and `-0`), the rest by
// kind and text. `what` is "map key" or "set element".
func keyOf(value any, what string) (string, error) {
	switch v := value.(type) {
	case string:
		return "s" + v, nil
	case bool:
		return "b" + strconv.FormatBool(v), nil
	case int:
		return integerKey(big.NewInt(int64(v)), what)
	case int64:
		return integerKey(big.NewInt(v), what)
	case uint64:
		return integerKey(new(big.Int).SetUint64(v), what)
	case float64:
		return floatKey(v), nil
	case json.RawMessage:
		text := bytes.TrimSpace(v)
		if len(text) > 0 {
			switch text[0] {
			case '"':
				var s string
				if json.Unmarshal(text, &s) == nil {
					return "s" + s, nil
				}
			case 't', 'f':
				return "b" + string(text), nil
			case 'n', '[', '{':
			default:
				return numberKey(string(text), what)
			}
		}
	case *big.Int:
		if v != nil {
			return "B" + v.String(), nil
		}
	case values.DateTime, values.Date, values.TimeOfDay:
		return fmt.Sprintf("%T%s", v, v), nil
	case values.Decimal:
		return "D" + string(v), nil
	case values.UUID:
		return "U" + string(v), nil
	}
	return "", valueErrorf("a %s cannot be %s: only strings, finite numbers, booleans, bigint, datetime, date, time, decimal and uuid values can (an instant, bytes, null, an array, an object, a map, a set or a reference cannot)", what, describe(value))
}

func integerKey(n *big.Int, what string) (string, error) {
	if n.CmpAbs(big.NewInt(MaxSafe)) > 0 {
		return "", valueErrorf("a %s cannot be the integer %s, beyond ±(2^53-1): send it as a bigint", what, n)
	}
	return "n" + n.String(), nil
}

func floatKey(f float64) string {
	if f == math.Trunc(f) && math.Abs(f) < 1e38 {
		n, _ := big.NewFloat(f).Int(nil)
		return "n" + n.String()
	}
	return "f" + strconv.FormatUint(math.Float64bits(f), 16)
}

// numberKey is the key of a JSON number: an integer literal by its value
// (beyond ±(2^53-1) refused), any other by its float value.
func numberKey(text, what string) (string, error) {
	if !strings.ContainsAny(text, ".eE") {
		if n, ok := new(big.Int).SetString(text, 10); ok {
			return integerKey(n, what)
		}
	}
	f, err := strconv.ParseFloat(text, 64)
	if err != nil {
		return "", valueErrorf("a %s cannot be %s", what, text)
	}
	return floatKey(f), nil
}

// shownKey is a key as an error shows it.
func shownKey(value any) string {
	text := fmt.Sprint(value)
	switch v := value.(type) {
	case json.RawMessage:
		text = string(v)
	case string:
		text = strconv.Quote(v)
	case time.Time, []byte, Map, Set:
		text = describe(value)
	}
	if runes := []rune(text); len(runes) > 40 {
		return string(runes[:40]) + "…"
	}
	return text
}

// CheckKeys checks the keys of a map (`what` "map key") or the elements
// of a set ("set element"): each of a kind every language takes as a key,
// and no two equal, since a language that merges them would lose an
// entry.
func CheckKeys(keys []any, what string) error {
	seen := make(map[string]bool, len(keys))
	for _, key := range keys {
		identity, err := keyOf(key, what)
		if err != nil {
			return err
		}
		if seen[identity] {
			return valueErrorf("duplicate %s %s: a map or set whose keys collide would lose entries", what, shownKey(key))
		}
		seen[identity] = true
	}
	return nil
}

func (m Map) keys() []any {
	keys := make([]any, len(m))
	for i, entry := range m {
		keys[i] = entry.Key
	}
	return keys
}

// ── Decoding ─────────────────────────────────────────────────────────

// decoding is the state of decoding one frame's value: the call it is the
// arguments of ("" elsewhere), the references it imported anew, and the
// first value error. A value error does not stop the decoding: every
// reference in the value is still imported, so that a refused call
// releases each one (as a call taken would once its proxies go).
type decoding struct {
	call    string
	fresh   []*imported
	refused error
	// arguments: the value is a call's argument list, whose items are
	// whole arguments and keep undefined (#687 §七); only the outermost
	// list.
	arguments bool
}

func (d *decoding) refuse(err error) {
	if d.refused == nil {
		d.refused = err
	}
}

// declares reports whether this side declared `capability`; p.mu is
// held or not needed.
func (p *Peer) declares(capability string) bool {
	for _, c := range append(append([]string{}, p.opts.Capabilities...), p.opts.Endpoint.Declare...) {
		if c == capability {
			return true
		}
	}
	return false
}

// decodeTagged decodes a wire value of a tag beyond JSON data; p.mu is
// held. A value whose JSON shape is wrong is a malformed frame (an error
// returned); one whose text is not in its format, or of a tag this side
// did not declare, fails only its call (d.refuse).
func (p *Peer) decodeTagged(tag string, raw json.RawMessage, d *decoding) (any, error) {
	refuse := func(err error) (any, error) {
		d.refuse(&ValueError{Message: err.Error()})
		return nil, nil
	}
	undeclared := func() error {
		return valueErrorf("received a %s value, which this side does not take (it did not declare the capability value-%s)", tag, tag)
	}
	switch tag {
	case "map", "set":
		var items []json.RawMessage
		if err := json.Unmarshal(raw, &items); err != nil || items == nil {
			return nil, fmt.Errorf("invalid %s", tag)
		}
		before := d.refused
		var out []any
		if tag == "map" {
			for _, item := range items {
				var pair []json.RawMessage
				if err := json.Unmarshal(item, &pair); err != nil || len(pair) != 2 {
					return nil, errors.New("invalid map")
				}
				for _, part := range pair {
					decoded, err := p.decodeWith(part, d)
					if err != nil {
						return nil, err
					}
					out = append(out, decoded)
				}
			}
		} else {
			for _, item := range items {
				decoded, err := p.decodeWith(item, d)
				if err != nil {
					return nil, err
				}
				out = append(out, decoded)
			}
		}
		// Their entries are read even then, for the references in them.
		if !p.declares("value-" + tag) {
			return refuse(undeclared())
		}
		if d.refused != before {
			return nil, nil
		}
		if tag == "set" {
			if err := CheckKeys(out, "set element"); err != nil {
				return refuse(err)
			}
			return Set(out), nil
		}
		m := make(Map, len(out)/2)
		for i := range m {
			value := out[2*i+1]
			// Undefined is null in data (design V10).
			if _, undefined := value.(UndefinedType); undefined {
				value = json.RawMessage("null")
			}
			m[i] = MapEntry{Key: out[2*i], Value: value}
		}
		if err := CheckKeys(m.keys(), "map key"); err != nil {
			return refuse(err)
		}
		return m, nil
	}
	var text string
	if err := json.Unmarshal(raw, &text); err != nil {
		return nil, fmt.Errorf("invalid %s", tag)
	}
	if !p.declares("value-" + tag) {
		return refuse(undeclared())
	}
	value, err := parseTagged(tag, text)
	if err != nil {
		return refuse(err)
	}
	return value, nil
}

// parseTagged reads the text of a tagged value other than a map or a set.
func parseTagged(tag, text string) (any, error) {
	var value any
	var err error
	switch tag {
	case "instant":
		value, err = values.ParseInstant(text)
	case "datetime":
		value, err = values.ParseDateTime(text)
	case "date":
		value, err = values.ParseDate(text)
	case "time":
		value, err = values.ParseTimeOfDay(text)
	case "bytes":
		value, err = values.ParseBytes(text)
	case "bigint":
		value, err = values.ParseBigInt(text)
	case "decimal":
		err = values.CheckDecimal(text)
		value = values.Decimal(text)
	case "uuid":
		err = values.CheckUUID(text)
		value = values.UUID(text)
	default:
		err = fmt.Errorf("invalid %s", tag)
	}
	return value, err
}

// isTag reports whether `tag` is one of the tags beyond JSON data.
func isTag(tag string) bool {
	for _, t := range values.Tags {
		if t == tag {
			return true
		}
	}
	return false
}
