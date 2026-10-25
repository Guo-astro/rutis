package convert

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"reflect"
	"strings"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/values"
)

// ── wire -> Go ───────────────────────────────────────────────────
//
// A value decodes into its declared Go type: data as encoding/json
// decodes it; an instant into time.Time (UTC), bytes into []byte, a
// bigint into *big.Int, big.Int or an integer it fits, a datetime, date,
// time, decimal or uuid into its type or a string, a map into map[K]V,
// a set into a slice or map[K]struct{}. Into `any`: the same types, a map
// as map[any]any and a set as a slice. A map whose keys become equal in
// Go (two keys that differ only in kind, decoded into one key type) fails:
// it would lose entries.

// Decode decodes a peer value into `out`, a pointer (nil: discard).
func Decode(v any, out any) error {
	if out == nil {
		return nil
	}
	target := reflect.ValueOf(out)
	if target.Kind() != reflect.Pointer || target.IsNil() {
		return errors.New("a result must be decoded into a non-nil pointer")
	}
	value, err := FromPeer(v, target.Type().Elem())
	if err != nil {
		return err
	}
	target.Elem().Set(value)
	return nil
}

// FromPeer decodes a peer value as a value of type t.
func FromPeer(v any, t reflect.Type) (reflect.Value, error) {
	switch x := v.(type) {
	case nil, peer.UndefinedType, *peer.Signal:
		return reflect.Zero(t), nil
	case json.RawMessage:
		if t == anyType {
			plain, err := Plain(x)
			if err != nil {
				return reflect.Value{}, err
			}
			return valueOf(plain, t), nil
		}
		if t == errType {
			var failure struct{ Name, Message string }
			if err := json.Unmarshal(x, &failure); err != nil {
				return reflect.Value{}, err
			}
			return reflect.ValueOf(error(&peer.RemoteError{Name: failure.Name, Message: failure.Message})), nil
		}
		if string(x) == "null" {
			return reflect.Zero(t), nil
		}
		out := reflect.New(t)
		if err := json.Unmarshal(x, out.Interface()); err != nil {
			return reflect.Value{}, fmt.Errorf("cannot decode %s as %s: %w", abbreviate(x), t, err)
		}
		return out.Elem(), nil
	case *peer.RemoteFunction:
		switch {
		case t == functionType:
			return reflect.ValueOf(&Function{x}), nil
		case t.Kind() == reflect.Func:
			return MakeFunc(t, func(ctx context.Context, args []any) (any, error) {
				return x.Call(ctx, args, true)
			}), nil
		case t == anyType:
			return reflect.ValueOf(any(&Function{x})), nil
		}
		return reflect.Value{}, fmt.Errorf("a function cannot be decoded as %s", t)
	case peer.Func:
		if t.Kind() == reflect.Func {
			return MakeFunc(t, x), nil
		}
		return reflect.Value{}, fmt.Errorf("a function cannot be decoded as %s", t)
	case *peer.RemoteFuture:
		switch t {
		case futureType:
			return reflect.ValueOf(&Future{x}), nil
		case anyType:
			return reflect.ValueOf(any(&Future{x})), nil
		}
		return reflect.Value{}, fmt.Errorf("an asynchronous result cannot be decoded as %s: use *rutis.Future", t)
	case peer.Object:
		if reflect.TypeOf(x).AssignableTo(t) {
			return reflect.ValueOf(x), nil
		}
		if o, ok := x.(object); ok && o.rv.Type().AssignableTo(t) {
			return o.rv, nil
		}
		return reflect.Value{}, fmt.Errorf("an object cannot be decoded as %s", t)
	case peer.Plugin:
		// A plugin definition is its own value within this process: a
		// service of this process gets its arguments as peer values when
		// the client's field type is not the method's, or through
		// *rutis.Service (localService → Call; see
		// TestALocalServiceGetsTheDefinitionItself).
		if x.Value != nil && reflect.TypeOf(x.Value).AssignableTo(t) {
			return valueOf(x.Value, t), nil
		}
		return reflect.Value{}, fmt.Errorf("a plugin definition cannot be decoded as %s: use *rutis.Definition", t)
	case *peer.RemotePlugin:
		if PluginFromPeer != nil && (t == PluginType || t == anyType) {
			return valueOf(PluginFromPeer(x), t), nil
		}
		return reflect.Value{}, fmt.Errorf("a plugin definition cannot be decoded as %s: use *rutis.Definition", t)
	}
	switch v.(type) {
	case []any, map[string]any, peer.Map, peer.Set:
	default:
		// A tagged value into its own type, or a Go value given as it is
		// (a call within this process).
		if value := reflect.ValueOf(v); value.Type().AssignableTo(t) {
			return valueOf(v, t), nil
		}
	}
	// Lists, records and tagged values: into what a pointer points to, or
	// as plain values into an interface.
	switch t.Kind() {
	case reflect.Pointer:
		if t != bigIntPointerType {
			inner, err := FromPeer(v, t.Elem())
			if err != nil {
				return reflect.Value{}, err
			}
			out := reflect.New(t.Elem())
			out.Elem().Set(inner)
			return out, nil
		}
	case reflect.Interface:
		plain, err := Plain(v)
		if err != nil {
			return reflect.Value{}, err
		}
		if plain != nil && !reflect.TypeOf(plain).AssignableTo(t) {
			return reflect.Value{}, fmt.Errorf("%T cannot be decoded as %s", plain, t)
		}
		return valueOf(plain, t), nil
	}
	switch x := v.(type) {
	case []any:
		switch t.Kind() {
		case reflect.Slice, reflect.Array:
			return listFromPeer(x, t, "a list")
		}
		return reflect.Value{}, fmt.Errorf("a list cannot be decoded as %s", t)
	case map[string]any:
		switch t.Kind() {
		case reflect.Map:
			if t.Key().Kind() != reflect.String {
				return reflect.Value{}, fmt.Errorf("a record cannot be decoded as %s", t)
			}
			out := reflect.MakeMapWithSize(t, len(x))
			for key, item := range x {
				value, err := FromPeer(item, t.Elem())
				if err != nil {
					return reflect.Value{}, err
				}
				out.SetMapIndex(reflect.ValueOf(key).Convert(t.Key()), value)
			}
			return out, nil
		case reflect.Struct:
			out := reflect.New(t).Elem()
			for key, item := range x {
				field, ok := fieldByJSONName(t, key)
				if !ok {
					continue
				}
				value, err := FromPeer(item, field.Type)
				if err != nil {
					return reflect.Value{}, err
				}
				out.FieldByIndex(field.Index).Set(value)
			}
			return out, nil
		}
		return reflect.Value{}, fmt.Errorf("a record cannot be decoded as %s", t)
	case peer.Map:
		if t.Kind() != reflect.Map {
			return reflect.Value{}, fmt.Errorf("a map cannot be decoded as %s", t)
		}
		out := reflect.MakeMapWithSize(t, len(x))
		for _, entry := range x {
			key, err := FromPeer(entry.Key, t.Key())
			if err != nil {
				return reflect.Value{}, fmt.Errorf("map key: %w", err)
			}
			value, err := FromPeer(entry.Value, t.Elem())
			if err != nil {
				return reflect.Value{}, err
			}
			out.SetMapIndex(key, value)
		}
		if out.Len() != len(x) {
			return reflect.Value{}, fmt.Errorf("a map whose keys become equal as %s cannot be decoded: it would lose entries", t.Key())
		}
		return out, nil
	case peer.Set:
		switch {
		case t.Kind() == reflect.Slice || t.Kind() == reflect.Array:
			return listFromPeer(x, t, "a set")
		case t.Kind() == reflect.Map && isSetType(t):
			out := reflect.MakeMapWithSize(t, len(x))
			for _, element := range x {
				key, err := FromPeer(element, t.Key())
				if err != nil {
					return reflect.Value{}, fmt.Errorf("set element: %w", err)
				}
				out.SetMapIndex(key, reflect.Zero(t.Elem()))
			}
			if out.Len() != len(x) {
				return reflect.Value{}, fmt.Errorf("a set whose elements become equal as %s cannot be decoded: it would lose elements", t.Key())
			}
			return out, nil
		}
		return reflect.Value{}, fmt.Errorf("a set cannot be decoded as %s: use a slice or map[K]struct{}", t)
	case time.Time:
		if t == timeType {
			return reflect.ValueOf(x), nil
		}
	case []byte:
		switch {
		case t == rawMessageType:
		case t.Kind() == reflect.Slice && t.Elem().Kind() == reflect.Uint8:
			return reflect.ValueOf(append([]byte{}, x...)).Convert(t), nil
		case t.Kind() == reflect.Array && t.Elem().Kind() == reflect.Uint8:
			if len(x) != t.Len() {
				return reflect.Value{}, fmt.Errorf("%d bytes cannot be decoded as %s", len(x), t)
			}
			out := reflect.New(t).Elem()
			reflect.Copy(out, reflect.ValueOf(x))
			return out, nil
		}
	case *big.Int:
		switch t {
		case bigIntPointerType:
			return reflect.ValueOf(new(big.Int).Set(x)), nil
		case bigIntType:
			return reflect.ValueOf(new(big.Int).Set(x)).Elem(), nil
		}
		out := reflect.New(t).Elem()
		switch t.Kind() {
		case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
			if x.IsInt64() && !out.OverflowInt(x.Int64()) {
				out.SetInt(x.Int64())
				return out, nil
			}
			return reflect.Value{}, fmt.Errorf("the bigint %s does not fit %s", abbreviateText(x.String()), t)
		case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64, reflect.Uintptr:
			if x.IsUint64() && !out.OverflowUint(x.Uint64()) {
				out.SetUint(x.Uint64())
				return out, nil
			}
			return reflect.Value{}, fmt.Errorf("the bigint %s does not fit %s", abbreviateText(x.String()), t)
		}
	case values.DateTime, values.Date, values.TimeOfDay, values.Decimal, values.UUID:
		own := reflect.TypeOf(x)
		if t == own {
			return reflect.ValueOf(x), nil
		}
		// Their text, into a string that is not another tagged type.
		if t.Kind() == reflect.String && t != decimalType && t != uuidType {
			return reflect.ValueOf(fmt.Sprint(x)).Convert(t), nil
		}
	}
	return reflect.Value{}, fmt.Errorf("%s cannot be decoded as %s", describe(v), t)
}

// listFromPeer decodes the items of a list or set into a slice or array.
func listFromPeer(items []any, t reflect.Type, what string) (reflect.Value, error) {
	out := reflect.New(t).Elem()
	if t.Kind() == reflect.Slice {
		out = reflect.MakeSlice(t, len(items), len(items))
	} else if len(items) > t.Len() {
		return reflect.Value{}, fmt.Errorf("%s of %d items does not fit %s", what, len(items), t)
	}
	for i, item := range items {
		value, err := FromPeer(item, t.Elem())
		if err != nil {
			return reflect.Value{}, err
		}
		out.Index(i).Set(value)
	}
	return out, nil
}

// describe is what a peer value is, for errors.
func describe(v any) string {
	switch v.(type) {
	case time.Time:
		return "an instant"
	case values.DateTime:
		return "a datetime"
	case values.Date:
		return "a date"
	case values.TimeOfDay:
		return "a time"
	case []byte:
		return "bytes"
	case *big.Int:
		return "a bigint"
	case values.Decimal:
		return "a decimal"
	case values.UUID:
		return "a uuid"
	case peer.Map:
		return "a map"
	case peer.Set:
		return "a set"
	}
	return fmt.Sprintf("%T", v)
}

func valueOf(v any, t reflect.Type) reflect.Value {
	if v == nil {
		return reflect.Zero(t)
	}
	value := reflect.ValueOf(v)
	if t.Kind() == reflect.Interface && value.Type() != t {
		out := reflect.New(t).Elem()
		out.Set(value)
		return out
	}
	return value
}

func abbreviate(raw json.RawMessage) string { return abbreviateText(string(raw)) }

func abbreviateText(text string) string {
	if len(text) > 60 {
		return text[:57] + "..."
	}
	return text
}

// Plain is a peer value as plain Go values: JSON data decoded as
// encoding/json does into `any`, references as *Function and *Future,
// tagged values as their Go types (time.Time, []byte, *big.Int, DateTime,
// Date, TimeOfDay, Decimal, UUID), a map as map[any]any and a set as a
// slice.
func Plain(v any) (any, error) {
	switch x := v.(type) {
	case nil, peer.UndefinedType, *peer.Signal:
		return nil, nil
	case json.RawMessage:
		var out any
		if err := json.Unmarshal(x, &out); err != nil {
			return nil, err
		}
		return out, nil
	case []any:
		out := make([]any, len(x))
		for i, item := range x {
			plain, err := Plain(item)
			if err != nil {
				return nil, err
			}
			out[i] = plain
		}
		return out, nil
	case peer.Set:
		return Plain([]any(x))
	case map[string]any:
		out := make(map[string]any, len(x))
		for key, item := range x {
			plain, err := Plain(item)
			if err != nil {
				return nil, err
			}
			out[key] = plain
		}
		return out, nil
	case peer.Map:
		out := make(map[any]any, len(x))
		for _, entry := range x {
			key, err := Plain(entry.Key)
			if err != nil {
				return nil, err
			}
			value, err := Plain(entry.Value)
			if err != nil {
				return nil, err
			}
			out[key] = value
		}
		if len(out) != len(x) {
			return nil, errors.New("a map whose keys become equal in Go cannot be decoded: it would lose entries")
		}
		return out, nil
	case *peer.RemoteFunction:
		return &Function{x}, nil
	case *peer.RemoteFuture:
		return &Future{x}, nil
	case object:
		return x.rv.Interface(), nil
	}
	return v, nil
}

func fieldByJSONName(t reflect.Type, name string) (reflect.StructField, bool) {
	var fold *reflect.StructField
	for _, field := range reflect.VisibleFields(t) {
		if !field.IsExported() || field.Anonymous {
			continue
		}
		key := field.Name
		if tag, ok := field.Tag.Lookup("json"); ok {
			tagName, _, _ := strings.Cut(tag, ",")
			if tagName == "-" {
				continue
			}
			if tagName != "" {
				key = tagName
			}
		}
		if key == name {
			return field, true
		}
		if fold == nil && strings.EqualFold(key, name) {
			f := field
			fold = &f
		}
	}
	if fold != nil {
		return *fold, true
	}
	return reflect.StructField{}, false
}
