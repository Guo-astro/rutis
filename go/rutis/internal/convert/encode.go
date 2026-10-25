package convert

import (
	"bytes"
	"cmp"
	"context"
	"encoding"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"math/big"
	"reflect"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/values"
)

// ── Go -> wire ───────────────────────────────────────────────────
//
// Every value goes through this one encoder, field by field and item by
// item (design V9): integers beyond ±(2^53-1) are bigints, time.Time is
// an instant, []byte is bytes, maps with keys other than strings are maps,
// map[K]struct{} is a set, *big.Int is a bigint, and DateTime, Date,
// TimeOfDay, Decimal and UUID are their tags. Structs are objects with the
// fields encoding/json would write (names, `-`, omitempty, omitzero,
// string, embedded structs); a type with its own MarshalJSON or
// MarshalText is written as it says.

var (
	timeType          = reflect.TypeFor[time.Time]()
	bigIntType        = reflect.TypeFor[big.Int]()
	bigIntPointerType = reflect.TypeFor[*big.Int]()
	dateTimeType      = reflect.TypeFor[values.DateTime]()
	dateType          = reflect.TypeFor[values.Date]()
	timeOfDayType     = reflect.TypeFor[values.TimeOfDay]()
	decimalType       = reflect.TypeFor[values.Decimal]()
	uuidType          = reflect.TypeFor[values.UUID]()
	rawMessageType    = reflect.TypeFor[json.RawMessage]()
	numberType        = reflect.TypeFor[json.Number]()
	marshalerType     = reflect.TypeFor[json.Marshaler]()
	textMarshalerType = reflect.TypeFor[encoding.TextMarshaler]()
)

// maxHops bounds how many pointers and interfaces a value may go through
// on one path, so a cycle of pointers fails instead of recursing forever.
const maxHops = 1000

// ToPeerArgs converts arguments.
func ToPeerArgs(args []any) ([]any, error) {
	out := make([]any, len(args))
	for i, arg := range args {
		converted, err := ToPeer(arg)
		if err != nil {
			return nil, fmt.Errorf("argument %d: %w", i, err)
		}
		out[i] = converted
	}
	return out, nil
}

// The SDK's plugin definitions (*rutis.Definition), set by package rutis
// (docs/design-plugin-values-2026-10-25.md §5): PluginType is their type,
// PluginToPeer the peer value of one (a peer.Plugin, by reference, not
// data; a *peer.RemotePlugin for one another process handed over), and
// PluginFromPeer the definition of a *peer.RemotePlugin.
var (
	PluginType     reflect.Type
	PluginToPeer   func(definition any) any
	PluginFromPeer func(*peer.RemotePlugin) any
)

// ToPeer converts a Go value to a peer value.
func ToPeer(v any) (any, error) { return toPeer(v, 0, 0) }

// toPeer converts a value whose type is known only now. `levels` counts
// the arrays and objects around it, `hops` the steps on its path.
func toPeer(v any, levels, hops int) (any, error) {
	switch x := v.(type) {
	case nil:
		return nil, nil
	case peer.UndefinedType, *peer.RemoteFunction, *peer.RemoteFuture, *peer.RemotePlugin, peer.Func, peer.Object, peer.Plugin, peer.Map, peer.Set:
		return x, nil
	case json.RawMessage:
		return rawData(x)
	case ref:
		if x.value == nil {
			return nil, nil
		}
		return Object(x.value), nil
	case *Future:
		if x == nil {
			return nil, nil
		}
		return x.remote, nil
	case *Function:
		if x == nil {
			return nil, nil
		}
		return x.remote, nil
	case context.Context:
		return nil, errors.New("a context.Context cannot cross between processes")
	case error:
		return x, nil
	}
	return toPeerValue(reflect.ValueOf(v), levels, hops)
}

func rawData(raw json.RawMessage) (any, error) {
	if len(raw) == 0 {
		return nil, nil
	}
	if !json.Valid(raw) {
		return nil, errors.New("a json.RawMessage that is not JSON cannot cross")
	}
	return raw, nil
}

func toPeerValue(rv reflect.Value, levels, hops int) (any, error) {
	if !rv.IsValid() {
		return nil, nil
	}
	if levels > peer.MaxDepth {
		return nil, peer.ErrTooDeep
	}
	if hops > maxHops {
		return nil, fmt.Errorf("%s nests through more than %d pointers: a cycle cannot cross", rv.Type(), maxHops)
	}
	t := rv.Type()
	switch t.Kind() {
	case reflect.Pointer, reflect.Interface, reflect.Map, reflect.Slice, reflect.Func:
		if rv.IsNil() {
			return nil, nil
		}
	}
	switch t.Kind() {
	case reflect.Interface:
		return toPeer(rv.Elem().Interface(), levels, hops+1)
	case reflect.Pointer:
		switch t {
		case bigIntPointerType:
			return rv.Interface(), nil
		case futureType, functionType:
			return toPeer(rv.Interface(), levels, hops+1)
		}
		// A plugin definition crosses as itself, a `plugin` reference.
		if t == PluginType && PluginToPeer != nil {
			return PluginToPeer(rv.Interface()), nil
		}
		// What it points to: addressable, so methods on the pointer are
		// found there (as encoding/json finds them).
		return toPeerValue(rv.Elem(), levels, hops+1)
	}
	switch t {
	case timeType, dateTimeType, dateType, timeOfDayType, decimalType, uuidType:
		return rv.Interface(), nil
	case bigIntType:
		n := new(big.Int)
		if rv.CanAddr() {
			n.Set(rv.Addr().Interface().(*big.Int))
		} else {
			copied := reflect.New(bigIntType)
			copied.Elem().Set(rv)
			n.Set(copied.Interface().(*big.Int))
		}
		return n, nil
	case rawMessageType:
		return rawData(rv.Bytes())
	case numberType:
		text := rv.String()
		if text == "" {
			text = "0"
		}
		if _, err := strconv.ParseFloat(text, 64); err != nil || !json.Valid([]byte(text)) {
			return nil, fmt.Errorf("invalid json.Number %q", text)
		}
		return withBigInts(json.RawMessage(text))
	}
	if marshaled, ok, err := marshaled(rv); ok {
		return marshaled, err
	}
	switch t.Kind() {
	case reflect.Func:
		return peer.Func(func(ctx context.Context, args []any) (any, error) {
			return Call(ctx, rv, args)
		}), nil
	case reflect.Bool:
		return rv.Bool(), nil
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		n := rv.Int()
		if n > peer.MaxSafe || n < -peer.MaxSafe {
			// Beyond what every language holds exactly: a bigint (V6).
			return big.NewInt(n), nil
		}
		return n, nil
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64, reflect.Uintptr:
		n := rv.Uint()
		if n > peer.MaxSafe {
			return new(big.Int).SetUint64(n), nil
		}
		return n, nil
	case reflect.Float32, reflect.Float64:
		f := rv.Float()
		if math.IsNaN(f) || math.IsInf(f, 0) {
			return nil, fmt.Errorf("the number %v is not finite: only finite numbers cross", f)
		}
		if t.Kind() == reflect.Float32 {
			// Written as encoding/json writes a float32: 0.1, not
			// 0.10000000149011612.
			data, _ := json.Marshal(float32(f))
			return json.RawMessage(data), nil
		}
		return f, nil
	case reflect.String:
		return rv.String(), nil
	case reflect.Slice, reflect.Array:
		if t.Elem().Kind() == reflect.Uint8 {
			out := make([]byte, rv.Len())
			reflect.Copy(reflect.ValueOf(out), rv)
			return out, nil
		}
		items := make([]any, rv.Len())
		for i := range items {
			item, err := toPeerValue(rv.Index(i), levels+1, hops+1)
			if err != nil {
				return nil, err
			}
			items[i] = item
		}
		return items, nil
	case reflect.Map:
		return mapToPeer(rv, levels, hops)
	case reflect.Struct:
		fields := structFields(t)
		out := make(map[string]any, len(fields))
		for _, field := range fields {
			value, err := rv.FieldByIndexErr(field.index)
			if err != nil {
				continue // in an embedded struct behind a nil pointer
			}
			if field.omitEmpty && isEmpty(value) || field.omitZero && isZero(value) {
				continue
			}
			item, err := toPeerValue(value, levels+1, hops+1)
			if err != nil {
				return nil, err
			}
			if field.quoted {
				item = quotedField(item)
			}
			out[field.name] = item
		}
		return out, nil
	}
	return nil, fmt.Errorf("%s cannot cross between processes", t)
}

// marshaled is the value of a type that writes itself (json.Marshaler,
// encoding.TextMarshaler), as encoding/json would take it.
func marshaled(rv reflect.Value) (any, bool, error) {
	t := rv.Type()
	addressable := t.Kind() != reflect.Pointer && rv.CanAddr()
	switch {
	case t.Implements(marshalerType) || addressable && reflect.PointerTo(t).Implements(marshalerType):
		if addressable && !t.Implements(marshalerType) {
			rv = rv.Addr()
		}
		data, err := rv.Interface().(json.Marshaler).MarshalJSON()
		if err != nil {
			return nil, true, fmt.Errorf("%s cannot cross between processes: %w", t, err)
		}
		var compact bytes.Buffer
		if err := json.Compact(&compact, data); err != nil {
			return nil, true, fmt.Errorf("%s cannot cross between processes: its MarshalJSON wrote invalid JSON", t)
		}
		value, err := withBigInts(json.RawMessage(compact.Bytes()))
		return value, true, err
	case t.Implements(textMarshalerType) || addressable && reflect.PointerTo(t).Implements(textMarshalerType):
		if addressable && !t.Implements(textMarshalerType) {
			rv = rv.Addr()
		}
		text, err := rv.Interface().(encoding.TextMarshaler).MarshalText()
		if err != nil {
			return nil, true, fmt.Errorf("%s cannot cross between processes: %w", t, err)
		}
		return string(text), true, nil
	}
	return nil, false, nil
}

// withBigInts is JSON as it crosses: as it is, unless it holds integers
// beyond ±(2^53-1), which cross as bigints (V6) in its items.
func withBigInts(data json.RawMessage) (any, error) {
	if !holdsBigInt(data) {
		return data, nil
	}
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.UseNumber()
	var tree any
	if err := decoder.Decode(&tree); err != nil {
		return nil, err
	}
	return fromJSON(tree), nil
}

func holdsBigInt(data json.RawMessage) bool {
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.UseNumber()
	for {
		token, err := decoder.Token()
		if err != nil {
			return false
		}
		if number, ok := token.(json.Number); ok && bigInteger(number) != nil {
			return true
		}
	}
}

// bigInteger is `number` when it is an integer beyond ±(2^53-1).
func bigInteger(number json.Number) *big.Int {
	if strings.ContainsAny(string(number), ".eE") {
		return nil
	}
	if n, ok := new(big.Int).SetString(string(number), 10); ok && n.CmpAbs(big.NewInt(peer.MaxSafe)) > 0 {
		return n
	}
	return nil
}

func fromJSON(tree any) any {
	switch v := tree.(type) {
	case json.Number:
		if n := bigInteger(v); n != nil {
			return n
		}
		return json.RawMessage(v)
	case []any:
		for i, item := range v {
			v[i] = fromJSON(item)
		}
	case map[string]any:
		for key, item := range v {
			v[key] = fromJSON(item)
		}
	}
	return tree
}

func quotedField(item any) any {
	switch item.(type) {
	case bool, int64, uint64, float64, string, json.RawMessage:
		data, err := json.Marshal(item)
		if err == nil {
			return string(data)
		}
	}
	return item
}

// isSetType reports whether a map of type t is a set: map[K]struct{}.
func isSetType(t reflect.Type) bool {
	return t.Elem().Kind() == reflect.Struct && t.Elem().NumField() == 0
}

// isRecordKey reports whether maps keyed by t are objects: string keys,
// other than the tagged string types.
func isRecordKey(t reflect.Type) bool {
	return t.Kind() == reflect.String && t != decimalType && t != uuidType
}

func mapToPeer(rv reflect.Value, levels, hops int) (any, error) {
	t := rv.Type()
	if isRecordKey(t.Key()) && !isSetType(t) {
		out := make(map[string]any, rv.Len())
		iter := rv.MapRange()
		for iter.Next() {
			item, err := toPeerValue(iter.Value(), levels+1, hops+1)
			if err != nil {
				return nil, err
			}
			out[iter.Key().String()] = item
		}
		return out, nil
	}
	// Go maps have no order: keys are sorted, so the same map is always
	// sent the same way.
	keys := rv.MapKeys()
	slices.SortFunc(keys, compareKeys)
	if isSetType(t) {
		out := make(peer.Set, len(keys))
		for i, key := range keys {
			element, err := toPeerValue(key, levels+1, hops+1)
			if err != nil {
				return nil, err
			}
			out[i] = element
		}
		return out, nil
	}
	out := make(peer.Map, len(keys))
	for i, key := range keys {
		k, err := toPeerValue(key, levels+1, hops+1)
		if err != nil {
			return nil, err
		}
		v, err := toPeerValue(rv.MapIndex(key), levels+1, hops+1)
		if err != nil {
			return nil, err
		}
		out[i] = peer.MapEntry{Key: k, Value: v}
	}
	return out, nil
}

// compareKeys orders map keys: numbers by value, strings and the rest by
// their text, different kinds by kind.
func compareKeys(a, b reflect.Value) int {
	for a.Kind() == reflect.Interface && !a.IsNil() {
		a = a.Elem()
	}
	for b.Kind() == reflect.Interface && !b.IsNil() {
		b = b.Elem()
	}
	rank := func(v reflect.Value) int {
		switch v.Kind() {
		case reflect.Bool:
			return 0
		case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
			reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64, reflect.Uintptr,
			reflect.Float32, reflect.Float64:
			return 1
		case reflect.String:
			return 2
		}
		return 3
	}
	if c := cmp.Compare(rank(a), rank(b)); c != 0 {
		return c
	}
	switch rank(a) {
	case 0:
		return cmp.Compare(boolInt(a.Bool()), boolInt(b.Bool()))
	case 1:
		return cmp.Compare(number(a), number(b))
	case 2:
		if c := cmp.Compare(a.Type().String(), b.Type().String()); c != 0 {
			return c
		}
		return cmp.Compare(a.String(), b.String())
	}
	if c := cmp.Compare(a.Type().String(), b.Type().String()); c != 0 {
		return c
	}
	return cmp.Compare(fmt.Sprint(a.Interface()), fmt.Sprint(b.Interface()))
}

func boolInt(b bool) int {
	if b {
		return 1
	}
	return 0
}

func number(v reflect.Value) float64 {
	switch v.Kind() {
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		return float64(v.Int())
	case reflect.Float32, reflect.Float64:
		return v.Float()
	}
	return float64(v.Uint())
}

func isEmpty(v reflect.Value) bool {
	switch v.Kind() {
	case reflect.Array, reflect.Map, reflect.Slice, reflect.String:
		return v.Len() == 0
	case reflect.Bool, reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64, reflect.Uintptr,
		reflect.Float32, reflect.Float64, reflect.Interface, reflect.Pointer:
		return v.IsZero()
	}
	return false
}

func isZero(v reflect.Value) bool {
	if zeroer, ok := v.Interface().(interface{ IsZero() bool }); ok {
		if v.Kind() == reflect.Pointer && v.IsNil() {
			return true
		}
		return zeroer.IsZero()
	}
	return v.IsZero()
}

// ── Struct fields, as encoding/json finds them ───────────────────

type field struct {
	name      string
	index     []int
	tagged    bool
	omitEmpty bool
	omitZero  bool
	quoted    bool
}

var fieldTables sync.Map

// structFields are the fields of t that encoding/json writes: exported
// fields by their json name, those of embedded structs promoted; of
// fields with the same name, the shallowest wins, then the one with a
// json tag, else none.
func structFields(t reflect.Type) []field {
	if cached, ok := fieldTables.Load(t); ok {
		return cached.([]field)
	}
	type level struct {
		t     reflect.Type
		index []int
	}
	var fields []field
	taken := map[string]bool{}
	visited := map[reflect.Type]bool{}
	next := []level{{t, nil}}
	for len(next) > 0 {
		current := next
		next = nil
		found := map[string][]field{}
		var order []string
		for _, l := range current {
			if visited[l.t] {
				continue
			}
			visited[l.t] = true
			for i := 0; i < l.t.NumField(); i++ {
				sf := l.t.Field(i)
				ft := sf.Type
				if ft.Name() == "" && ft.Kind() == reflect.Pointer {
					ft = ft.Elem()
				}
				if sf.Anonymous {
					if !sf.IsExported() && ft.Kind() != reflect.Struct {
						continue
					}
				} else if !sf.IsExported() {
					continue
				}
				tag := sf.Tag.Get("json")
				if tag == "-" {
					continue
				}
				name, options, _ := strings.Cut(tag, ",")
				index := append(append([]int{}, l.index...), i)
				if name == "" && sf.Anonymous && ft.Kind() == reflect.Struct {
					next = append(next, level{ft, index})
					continue
				}
				f := field{name: name, index: index, tagged: name != ""}
				if name == "" {
					f.name = sf.Name
				}
				for _, option := range strings.Split(options, ",") {
					switch option {
					case "omitempty":
						f.omitEmpty = true
					case "omitzero":
						f.omitZero = true
					case "string":
						switch ft.Kind() {
						case reflect.Bool, reflect.String,
							reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
							reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64, reflect.Uintptr,
							reflect.Float32, reflect.Float64:
							f.quoted = true
						}
					}
				}
				if _, seen := found[f.name]; !seen {
					order = append(order, f.name)
				}
				found[f.name] = append(found[f.name], f)
			}
		}
		for _, name := range order {
			if taken[name] {
				continue
			}
			taken[name] = true
			candidates := found[name]
			if len(candidates) == 1 {
				fields = append(fields, candidates[0])
				continue
			}
			var tagged []field
			for _, f := range candidates {
				if f.tagged {
					tagged = append(tagged, f)
				}
			}
			if len(tagged) == 1 {
				fields = append(fields, tagged[0])
			}
		}
	}
	fieldTables.Store(t, fields)
	return fields
}
