// Package config checks a row's config against its plugin's JSON Schema:
// defaults filled, then validated (config schema design,
// docs/design-config-schema-2026-10-27.en.md §3–§6). The rules every
// language's SDK has; the shared case list specs/config-schema/cases.json
// keeps them the same word for word, and its README states the details
// settled while implementing.
//
// Values are JSON as Decode reads it: nil, bool, string, json.Number,
// []any and map[string]any. The runtime (and so rutistest, which runs it)
// checks a row's config with Check before Apply.
//
// Keywords checked: type, properties, required, additionalProperties,
// items, enum, const, minimum, maximum, default, anyOf, oneOf, allOf, $ref
// (to #/$defs/… and #/definitions/…). Others are neither checked nor
// warned about.
package config

import (
	"bytes"
	"encoding/json"
	"fmt"
	"math"
	"reflect"
	"slices"
	"sort"
	"strconv"
	"strings"
)

// Shown is how many problems are written; the rest as one last line.
const Shown = 20

// exact is the largest magnitude integers are written without a decimal
// point at.
const exact = 1 << 53

// Issue is one problem: where it is (keys as strings, indices as ints)
// and its sentence, path included (`home.lat should be a number, got
// "north"`).
type Issue struct {
	Path    []any
	Message string
}

// At is the problem at `path`, its sentence `<path> <reason>`.
func At(path []any, reason string) Issue {
	return Issue{Path: slices.Clone(path), Message: PathText(path) + " " + reason}
}

// Checked is what checking a config gives: the config with defaults
// filled (what Apply gets; nil for no value), every problem sorted by
// path, and the extra keys where the schema allows them.
type Checked struct {
	Config   any
	Errors   []Issue
	Warnings []Issue
}

// Decode reads JSON as this package's values (numbers as json.Number, so
// that integers stay apart from fractions).
func Decode(data []byte) (any, error) {
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.UseNumber()
	var value any
	if err := decoder.Decode(&value); err != nil {
		return nil, err
	}
	return value, nil
}

// Normalize is `value` (a schema written in Go, say) as Decode would read
// its JSON.
func Normalize(value any) (any, error) {
	data, err := json.Marshal(value)
	if err != nil {
		return nil, err
	}
	return Decode(data)
}

// Check fills `config` (nil: no config) with the defaults of `schema`,
// then validates it. Both are this package's values; `config` is copied,
// so changing the result changes nothing of it. `required`: missing
// `required` keys are errors.
func Check(schema, config any, required bool) Checked {
	doc := document{root: schema}
	none := config == nil
	var value any
	if !none {
		value = deepCopy(config)
	} else if fallback, ok := doc.defaultOf([]any{schema}); ok {
		value = deepCopy(fallback)
	} else if doc.isObjectRoot(schema) {
		value = map[string]any{}
	}
	doc.fill([]any{schema}, value)
	s := &sink{warn: true}
	value = doc.validate([]any{schema}, value, nil, required, s, branches{})
	// No value where the root does not allow it.
	if value == nil && none && len(s.errors) > 0 {
		s.errors = []Issue{At(nil, "is required")}
	}
	return Checked{Config: value, Errors: sorted(s.errors), Warnings: sorted(s.warnings)}
}

// Lines are the lines a config error is written as: `config: <problem>`,
// at most 20, then `config: and N more problems`.
func Lines(issues []Issue) []string {
	lines := []string{}
	for i, problem := range issues {
		if i == Shown {
			break
		}
		lines = append(lines, "config: "+problem.Message)
	}
	if len(issues) > Shown {
		lines = append(lines, fmt.Sprintf("config: and %d more problems", len(issues)-Shown))
	}
	return lines
}

// Data is the `data` of the `bad-config` error: `{issues: [{path,
// message}]}` (design §5.2).
func Data(issues []Issue) map[string]any {
	list := make([]any, len(issues))
	for i, problem := range issues {
		path := make([]any, len(problem.Path))
		copy(path, problem.Path)
		list[i] = map[string]any{"path": path, "message": problem.Message}
	}
	return map[string]any{"issues": list}
}

// ── The checking ────────────────────────────────────────────────

// less orders paths segment by segment, a key before an index, keys by
// code point (UTF-8 byte order is code point order), indices by value; a
// prefix first.
func less(a, b []any) bool {
	for i := 0; i < len(a) && i < len(b); i++ {
		xs, xKey := a[i].(string)
		ys, yKey := b[i].(string)
		switch {
		case xKey && yKey:
			if xs != ys {
				return xs < ys
			}
		case xKey != yKey:
			return xKey
		default:
			if x, y := a[i].(int), b[i].(int); x != y {
				return x < y
			}
		}
	}
	return len(a) < len(b)
}

// sorted is the problems sorted by path (stable: at one path, in the
// order found), each once.
func sorted(issues []Issue) []Issue {
	out := slices.Clone(issues)
	sort.SliceStable(out, func(i, j int) bool { return less(out[i].Path, out[j].Path) })
	seen := map[string]bool{}
	kept := []Issue{}
	for _, problem := range out {
		mark := fmt.Sprintf("%#v\x00%s", problem.Path, problem.Message)
		if !seen[mark] {
			seen[mark] = true
			kept = append(kept, problem)
		}
	}
	return kept
}

type sink struct {
	errors, warnings []Issue
	// Extra keys are warned about (not inside anyOf / oneOf branches).
	warn bool
}

// The state of an anyOf / oneOf branch tried at one value.
type tried struct {
	passed bool
	value  any
}

// branches are the anyOf / oneOf branches tried at one value, by the
// branch's identity (where its decoded map is).
type branches map[uintptr]*tried

type document struct{ root any }

// resolve is `#/$defs/…` or `#/definitions/…` in the document.
func (d document) resolve(reference any) (any, bool) {
	text, ok := reference.(string)
	if !ok || !strings.HasPrefix(text, "#") {
		return nil, false
	}
	pointer := text[1:]
	if !strings.HasPrefix(pointer, "/$defs/") && !strings.HasPrefix(pointer, "/definitions/") {
		return nil, false
	}
	node := d.root
	for _, token := range strings.Split(pointer[1:], "/") {
		token = strings.ReplaceAll(strings.ReplaceAll(token, "~1", "/"), "~0", "~")
		switch n := node.(type) {
		case map[string]any:
			child, ok := n[token]
			if !ok {
				return nil, false
			}
			node = child
		case []any:
			index, ok := arrayIndex(token)
			if !ok || index >= len(n) {
				return nil, false
			}
			node = n[index]
		default:
			return nil, false
		}
	}
	return node, true
}

// arrayIndex is a JSON Pointer's array index: digits without a leading
// zero.
func arrayIndex(token string) (int, bool) {
	if token == "" || (len(token) > 1 && token[0] == '0') {
		return 0, false
	}
	for _, c := range token {
		if c < '0' || c > '9' {
			return 0, false
		}
	}
	index, err := strconv.Atoi(token)
	return index, err == nil
}

// parts is `schemas` and what they take in through $ref and allOf, each
// once: a schema, then its $ref's, then its allOf members' in order.
func (d document) parts(schemas []any) []map[string]any {
	out := []map[string]any{}
	seen := map[uintptr]bool{}
	for _, schema := range schemas {
		d.collect(schema, &out, seen)
	}
	return out
}

func (d document) collect(schema any, out *[]map[string]any, seen map[uintptr]bool) {
	node, ok := schema.(map[string]any)
	if !ok {
		return
	}
	id := identity(node)
	if seen[id] {
		return
	}
	seen[id] = true
	*out = append(*out, node)
	if target, ok := d.resolve(node["$ref"]); ok {
		d.collect(target, out, seen)
	}
	if members, ok := node["allOf"].([]any); ok {
		for _, member := range members {
			d.collect(member, out, seen)
		}
	}
}

// identity tells schema objects apart: where the decoded map is (0 for
// anything else, such as a true or false branch, which never recurses).
func identity(value any) uintptr {
	if m, ok := value.(map[string]any); ok {
		return reflect.ValueOf(m).Pointer()
	}
	return 0
}

// defaultOf is the first `default` of `schemas` (through $ref and allOf).
func (d document) defaultOf(schemas []any) (any, bool) {
	for _, part := range d.parts(schemas) {
		if value, ok := part["default"]; ok {
			return value, true
		}
	}
	return nil, false
}

// isObjectRoot: the root's type is `object` or includes it.
func (d document) isObjectRoot(schema any) bool {
	for _, part := range d.parts([]any{schema}) {
		if ty, ok := part["type"]; ok {
			return slices.Contains(types(ty), "object")
		}
	}
	return false
}

// property is the schemas a child at `key` is checked against: each
// part's properties[key].
func property(parts []map[string]any, key string) []any {
	out := []any{}
	for _, part := range parts {
		if properties, ok := part["properties"].(map[string]any); ok {
			if schema, ok := properties[key]; ok {
				out = append(out, schema)
			}
		}
	}
	return out
}

// additional is the first additionalProperties of `parts`.
func additional(parts []map[string]any) (any, bool) {
	for _, part := range parts {
		if value, ok := part["additionalProperties"]; ok {
			return value, true
		}
	}
	return nil, false
}

// known is every properties key of `parts`, sorted by code point.
func known(parts []map[string]any) []string {
	keys := map[string]bool{}
	for _, part := range parts {
		if properties, ok := part["properties"].(map[string]any); ok {
			for key := range properties {
				keys[key] = true
			}
		}
	}
	return keysOf(keys)
}

// keysOf is a map's keys, sorted.
func keysOf[V any](m map[string]V) []string {
	out := make([]string, 0, len(m))
	for key := range m {
		out = append(out, key)
	}
	sort.Strings(out)
	return out
}

func itemsOf(parts []map[string]any) []any {
	out := []any{}
	for _, part := range parts {
		if items, ok := part["items"].(map[string]any); ok {
			out = append(out, items)
		}
	}
	return out
}

// fill fills defaults level by level: in an object that is there, each
// missing property with a `default`; then into the child objects and
// array items. Not into anyOf / oneOf.
func (d document) fill(schemas []any, value any) {
	parts := d.parts(schemas)
	switch v := value.(type) {
	case map[string]any:
		for _, part := range parts {
			properties, ok := part["properties"].(map[string]any)
			if !ok {
				continue
			}
			for _, key := range keysOf(properties) {
				if _, there := v[key]; there {
					continue
				}
				if fallback, ok := d.defaultOf([]any{properties[key]}); ok {
					v[key] = deepCopy(fallback)
				}
			}
		}
		names := known(parts)
		extra, _ := additional(parts)
		for _, key := range keysOf(v) {
			var below []any
			if _, found := slices.BinarySearch(names, key); found {
				below = property(parts, key)
			} else if schema, ok := extra.(map[string]any); ok {
				below = []any{schema}
			}
			if len(below) > 0 {
				d.fill(below, v[key])
			}
		}
	case []any:
		if below := itemsOf(parts); len(below) > 0 {
			for _, item := range v {
				d.fill(below, item)
			}
		}
	}
}

// validate validates `value` at `path` against every one of `schemas`:
// the value as it passed (integers normalised, a branch's defaults).
func (d document) validate(schemas []any, value any, path []any, required bool, s *sink, seen branches) any {
	parts := d.parts(schemas)
	// The type first: when it is wrong, nothing else at or below it.
	for _, part := range parts {
		ty, ok := part["type"]
		if !ok {
			continue
		}
		names := types(ty)
		if len(names) == 0 {
			continue
		}
		if !slices.ContainsFunc(names, func(name string) bool { return isType(name, value) }) {
			expected := make([]string, len(names))
			for i, name := range names {
				expected[i] = typeText(name)
			}
			s.errors = append(s.errors, At(path, "should be "+joined(expected)+", got "+ValueText(value)))
			return value
		}
		if slices.Contains(names, "integer") {
			value = integral(value)
		}
	}
	for _, part := range parts {
		if allowed, ok := part["enum"].([]any); ok && !slices.ContainsFunc(allowed, func(c any) bool { return same(c, value) }) {
			texts := make([]string, len(allowed))
			for i, c := range allowed {
				texts[i] = ValueText(c)
			}
			s.errors = append(s.errors, At(path, "should be "+joined(texts)+", got "+ValueText(value)))
		}
		if constant, ok := part["const"]; ok && !same(constant, value) {
			s.errors = append(s.errors, At(path, "should be "+ValueText(constant)+", got "+ValueText(value)))
		}
		if n, ok := number(value); ok {
			if minimum, ok := number(part["minimum"]); ok && n < minimum {
				s.errors = append(s.errors, At(path, "should be at least "+ValueText(part["minimum"])+", got "+ValueText(value)))
			}
			if maximum, ok := number(part["maximum"]); ok && n > maximum {
				s.errors = append(s.errors, At(path, "should be at most "+ValueText(part["maximum"])+", got "+ValueText(value)))
			}
		}
		for _, keyword := range []string{"anyOf", "oneOf"} {
			list, ok := part[keyword].([]any)
			if !ok {
				continue
			}
			var passed any
			matched := false
			for _, branch := range list {
				// At one value each branch is tried once: one being tried
				// (reached again through $ref) does not match, one tried
				// before gives its result again.
				id := identity(branch)
				if state, ok := seen[id]; id != 0 && ok {
					if state != nil && state.passed {
						passed, matched = deepCopy(state.value), true
						break
					}
					continue
				}
				if id != 0 {
					seen[id] = nil // being tried
				}
				trial := &sink{}
				candidate := d.validate([]any{branch}, deepCopy(value), path, required, trial, seen)
				good := len(trial.errors) == 0
				if id != 0 {
					seen[id] = &tried{passed: good, value: deepCopy(candidate)}
				}
				if good {
					passed, matched = candidate, true
					break
				}
			}
			if matched {
				value = passed
			} else {
				s.errors = append(s.errors, At(path, "should be "+joined(d.branchTexts(list))+", got "+ValueText(value)))
			}
		}
	}
	switch v := value.(type) {
	case map[string]any:
		if required {
			for _, part := range parts {
				keys, _ := part["required"].([]any)
				for _, key := range keys {
					if name, ok := key.(string); ok {
						if _, there := v[name]; !there {
							s.errors = append(s.errors, At(append(slices.Clone(path), name), "is required"))
						}
					}
				}
			}
		}
		names := known(parts)
		extra, written := additional(parts)
		described := slices.ContainsFunc(parts, func(part map[string]any) bool { _, ok := part["properties"]; return ok })
		for _, key := range keysOf(v) {
			here := append(slices.Clone(path), key)
			if _, found := slices.BinarySearch(names, key); found {
				v[key] = d.validate(property(parts, key), v[key], here, required, s, branches{})
				continue
			}
			switch schema := extra.(type) {
			case bool:
				if !schema {
					s.errors = append(s.errors, extraKey(here, key, names))
				}
			case map[string]any:
				v[key] = d.validate([]any{schema}, v[key], here, required, s, branches{})
			default:
				if !written && described && s.warn {
					s.warnings = append(s.warnings, extraKey(here, key, names))
				}
			}
		}
		return v
	case []any:
		if below := itemsOf(parts); len(below) > 0 {
			for i, item := range v {
				v[i] = d.validate(below, item, append(slices.Clone(path), i), required, s, branches{})
			}
		}
		return v
	}
	return value
}

// branchTexts is how the branches of a failed anyOf / oneOf are written
// (design §6.5), in order, each once.
func (d document) branchTexts(list []any) []string {
	texts := []string{}
	for _, branch := range list {
		var found []string
		known := false
		for _, part := range d.parts([]any{branch}) {
			if constant, ok := part["const"]; ok {
				found, known = []string{ValueText(constant)}, true
				break
			}
			if allowed, ok := part["enum"].([]any); ok {
				for _, c := range allowed {
					found = append(found, ValueText(c))
				}
				known = true
				break
			}
			if ty, ok := part["type"]; ok {
				if names := types(ty); len(names) > 0 {
					for _, name := range names {
						found = append(found, typeText(name))
					}
					known = true
					break
				}
			}
		}
		if !known {
			found = []string{"another accepted value"}
		}
		for _, text := range found {
			if !slices.Contains(texts, text) {
				texts = append(texts, text)
			}
		}
	}
	return texts
}

// extraKey is `<path> is not a config key`, with a close known key
// suggested.
func extraKey(path []any, key string, names []string) Issue {
	if like, ok := Closest(key, names); ok {
		return At(path, "is not a config key (did you mean "+like+"?)")
	}
	return At(path, "is not a config key")
}

var typeNames = []string{"string", "number", "integer", "boolean", "object", "array", "null"}

// types is the type names of `type` this package knows.
func types(ty any) []string {
	var names []string
	switch t := ty.(type) {
	case string:
		names = []string{t}
	case []any:
		for _, name := range t {
			if text, ok := name.(string); ok {
				names = append(names, text)
			}
		}
	}
	out := []string{}
	for _, name := range names {
		if slices.Contains(typeNames, name) {
			out = append(out, name)
		}
	}
	return out
}

// number is a JSON number's value.
func number(value any) (float64, bool) {
	n, ok := value.(json.Number)
	if !ok {
		return 0, false
	}
	f, err := strconv.ParseFloat(string(n), 64)
	if err != nil {
		if e, ok := err.(*strconv.NumError); !ok || e.Err != strconv.ErrRange {
			return 0, false
		}
	}
	return f, true
}

// integerText: a number written without a fraction or an exponent.
func integerText(n json.Number) bool {
	return !strings.ContainsAny(string(n), ".eE")
}

func isType(name string, value any) bool {
	switch name {
	case "string":
		_, ok := value.(string)
		return ok
	case "number":
		_, ok := value.(json.Number)
		return ok
	case "integer":
		n, ok := value.(json.Number)
		if !ok {
			return false
		}
		if integerText(n) {
			return true
		}
		f, ok := number(n)
		return ok && !math.IsInf(f, 0) && f == math.Trunc(f)
	case "boolean":
		_, ok := value.(bool)
		return ok
	case "object":
		_, ok := value.(map[string]any)
		return ok
	case "array":
		_, ok := value.([]any)
		return ok
	}
	return value == nil
}

// integral is an integral fraction written as an integer.
func integral(value any) any {
	n, ok := value.(json.Number)
	if !ok || integerText(n) {
		return value
	}
	if f, ok := number(n); ok && f == math.Trunc(f) && math.Abs(f) <= exact {
		return json.Number(strconv.FormatInt(int64(f), 10))
	}
	return value
}

func typeText(name string) string {
	switch name {
	case "string":
		return "a string"
	case "number":
		return "a number"
	case "integer":
		return "an integer"
	case "boolean":
		return "a boolean"
	case "object":
		return "an object"
	case "array":
		return "an array"
	}
	return "null"
}

// joined is `a`, `a or b`, `one of a, b, c`.
func joined(texts []string) string {
	switch len(texts) {
	case 1:
		return texts[0]
	case 2:
		return texts[0] + " or " + texts[1]
	}
	return "one of " + strings.Join(texts, ", ")
}

// same is JSON equality: numbers by value, objects regardless of key
// order.
func same(a, b any) bool {
	switch x := a.(type) {
	case json.Number:
		y, ok := b.(json.Number)
		if !ok {
			return false
		}
		if integerText(x) && integerText(y) {
			if i, err := strconv.ParseInt(string(x), 10, 64); err == nil {
				if j, err := strconv.ParseInt(string(y), 10, 64); err == nil {
					return i == j
				}
			}
			if i, err := strconv.ParseUint(string(x), 10, 64); err == nil {
				if j, err := strconv.ParseUint(string(y), 10, 64); err == nil {
					return i == j
				}
			}
		}
		f, okx := number(x)
		g, oky := number(y)
		return okx && oky && f == g
	case []any:
		y, ok := b.([]any)
		if !ok || len(x) != len(y) {
			return false
		}
		for i := range x {
			if !same(x[i], y[i]) {
				return false
			}
		}
		return true
	case map[string]any:
		y, ok := b.(map[string]any)
		if !ok || len(x) != len(y) {
			return false
		}
		for key, value := range x {
			other, ok := y[key]
			if !ok || !same(value, other) {
				return false
			}
		}
		return true
	case nil:
		return b == nil
	case string:
		y, ok := b.(string)
		return ok && x == y
	case bool:
		y, ok := b.(bool)
		return ok && x == y
	}
	return false
}

// deepCopy copies objects and arrays.
func deepCopy(value any) any {
	switch v := value.(type) {
	case map[string]any:
		out := make(map[string]any, len(v))
		for key, child := range v {
			out[key] = deepCopy(child)
		}
		return out
	case []any:
		out := make([]any, len(v))
		for i, child := range v {
			out[i] = deepCopy(child)
		}
		return out
	}
	return value
}
