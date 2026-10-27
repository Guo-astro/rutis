package rutis

import (
	"encoding"
	"encoding/json"
	"fmt"
	"reflect"
	"runtime/debug"
	"strings"
	"sync"
)

// Schemer is a configuration type that gives its own JSON Schema.
type Schemer interface {
	JSONSchema() map[string]any
}

var (
	schemerType         = reflect.TypeFor[Schemer]()
	jsonUnmarshalerType = reflect.TypeFor[json.Unmarshaler]()
	textUnmarshalerType = reflect.TypeFor[encoding.TextUnmarshaler]()
	jsonNumberType      = reflect.TypeFor[json.Number]()
)

// jsonField is a field of a struct as encoding/json decodes it.
type jsonField struct {
	name, goName, doc string
	typ               reflect.Type
	// The `,string` option: a number, boolean or string written inside
	// a JSON string.
	quoted bool
	depth  int
	// Named by its tag: it wins over untagged fields of the name at its
	// depth.
	tagged bool
}

// jsonFields is the fields encoding/json decodes a struct's keys into:
// exported fields by their tag name or Go name; an untagged embedded
// struct's fields promoted (the shallower of two of one name wins, two at
// one depth both drop, as encoding/json has it); a tagged embedded struct,
// or an embedded type that is not a struct, as one field.
func jsonFields(t reflect.Type) []jsonField {
	byName := map[string][]jsonField{}
	var order []string
	var walk func(t reflect.Type, depth int, visited map[reflect.Type]bool)
	walk = func(t reflect.Type, depth int, visited map[reflect.Type]bool) {
		if visited[t] {
			return
		}
		visited[t] = true
		defer delete(visited, t)
		for i := 0; i < t.NumField(); i++ {
			f := t.Field(i)
			tag, tagged := f.Tag.Lookup("json")
			tagName, rest, _ := strings.Cut(tag, ",")
			if tagged && tagName == "-" && rest == "" {
				continue
			}
			inner := f.Type
			if inner.Kind() == reflect.Pointer {
				inner = inner.Elem()
			}
			if f.Anonymous {
				if !f.IsExported() && inner.Kind() != reflect.Struct {
					continue
				}
				if tagName == "" && inner.Kind() == reflect.Struct {
					walk(inner, depth+1, visited)
					continue
				}
			} else if !f.IsExported() {
				continue
			}
			name := f.Name
			if tagName != "" {
				name = tagName
			}
			if _, ok := byName[name]; !ok {
				order = append(order, name)
			}
			byName[name] = append(byName[name], jsonField{
				name: name, goName: f.Name, doc: f.Tag.Get("doc"), typ: f.Type,
				quoted: strings.Contains(","+rest+",", ",string,"), depth: depth, tagged: tagName != "",
			})
		}
	}
	walk(t, 0, map[reflect.Type]bool{})
	var out []jsonField
	for _, name := range order {
		candidates := byName[name]
		depth := candidates[0].depth
		for _, c := range candidates {
			depth = min(depth, c.depth)
		}
		// At the shallowest depth: the one field, or the one tagged
		// field among them; none when that does not tell.
		var at, tagged []jsonField
		for _, c := range candidates {
			if c.depth == depth {
				at = append(at, c)
				if c.tagged {
					tagged = append(tagged, c)
				}
			}
		}
		switch {
		case len(at) == 1:
			out = append(out, at[0])
		case len(tagged) == 1:
			out = append(out, tagged[0])
		}
	}
	return out
}

// quotedSchema is a `,string` field's schema: a string (or null for a
// pointer) where its kind takes the option, else its own.
func quotedSchema(t reflect.Type) map[string]any {
	inner := t
	if inner.Kind() == reflect.Pointer {
		inner = inner.Elem()
	}
	switch inner.Kind() {
	case reflect.String, reflect.Bool, reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64, reflect.Uintptr,
		reflect.Float32, reflect.Float64:
		if t.Kind() == reflect.Pointer {
			return map[string]any{"type": []any{"string", "null"}}
		}
		return map[string]any{"type": "string"}
	}
	schema, _ := schemaOf(t)
	return schema
}

// decodesItself: the type (or its pointer) implements `iface`.
func decodesItself(t reflect.Type, iface reflect.Type) bool {
	return t.Implements(iface) || reflect.PointerTo(t).Implements(iface)
}

// schemaOf derives the JSON Schema of a configuration type.
func schemaOf(t reflect.Type) (map[string]any, error) {
	return schemaFor(t, map[reflect.Type]bool{})
}

func schemaFor(t reflect.Type, seen map[reflect.Type]bool) (map[string]any, error) {
	if t.Implements(schemerType) {
		return reflect.Zero(t).Interface().(Schemer).JSONSchema(), nil
	}
	if reflect.PointerTo(t).Implements(schemerType) {
		return reflect.New(t).Interface().(Schemer).JSONSchema(), nil
	}
	// A type that decodes itself has its own rules, which a schema of its
	// Go shape would contradict: only what encoding/json itself requires
	// (config schema design §3.3).
	if t.Kind() != reflect.Pointer {
		if decodesItself(t, jsonUnmarshalerType) {
			return map[string]any{}, nil
		}
		if decodesItself(t, textUnmarshalerType) {
			return map[string]any{"type": "string"}, nil
		}
	}
	if t == jsonNumberType {
		// A number, or a string holding one.
		return map[string]any{"type": []any{"number", "string"}}, nil
	}
	switch t.Kind() {
	case reflect.String:
		return map[string]any{"type": "string"}, nil
	case reflect.Bool:
		return map[string]any{"type": "boolean"}, nil
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		return map[string]any{"type": "integer"}, nil
	case reflect.Float32, reflect.Float64:
		return map[string]any{"type": "number"}, nil
	case reflect.Pointer:
		// A pointer tells "not written" (nil) apart: null is a value of
		// it, as encoding/json decodes it.
		schema, err := schemaFor(t.Elem(), seen)
		if err != nil {
			return nil, err
		}
		var types []any
		switch ty := schema["type"].(type) {
		case string:
			types = []any{ty}
		case []any:
			types = ty
		case []string:
			for _, name := range ty {
				types = append(types, name)
			}
		default:
			return schema, nil
		}
		for _, name := range types {
			if name == "null" {
				return schema, nil
			}
		}
		nullable := map[string]any{}
		for key, value := range schema {
			nullable[key] = value
		}
		nullable["type"] = append(append([]any{}, types...), "null")
		return nullable, nil
	case reflect.Interface:
		return map[string]any{}, nil
	case reflect.Slice, reflect.Array:
		// A byte slice crosses as a base64 string, as encoding/json has it.
		if t.Kind() == reflect.Slice && t.Elem().Kind() == reflect.Uint8 {
			return map[string]any{"type": "string"}, nil
		}
		items, err := schemaFor(t.Elem(), seen)
		if err != nil {
			return nil, err
		}
		return map[string]any{"type": "array", "items": items}, nil
	case reflect.Map:
		// Keys encoding/json takes: strings, integers, TextUnmarshalers.
		switch key := t.Key(); {
		case key.Kind() == reflect.String, key.Kind() >= reflect.Int && key.Kind() <= reflect.Uint64, decodesItself(key, textUnmarshalerType):
		default:
			return nil, fmt.Errorf("%s: only string, integer or text keys are configuration", t)
		}
		values, err := schemaFor(t.Elem(), seen)
		if err != nil {
			return nil, err
		}
		return map[string]any{"type": "object", "additionalProperties": values}, nil
	case reflect.Struct:
		if seen[t] {
			return nil, fmt.Errorf("%s refers to itself", t)
		}
		seen[t] = true
		defer delete(seen, t)
		// No `required`: a field not written is its zero value, as
		// encoding/json and the Go plugin guide have it (config schema
		// design §3.3); a pointer tells "not written" apart.
		properties := map[string]any{}
		for _, field := range jsonFields(t) {
			property, err := schemaFor(field.typ, seen)
			if err != nil {
				return nil, fmt.Errorf("%s.%s: %w", t, field.goName, err)
			}
			if field.quoted {
				property = quotedSchema(field.typ)
			}
			if field.doc != "" {
				copied := map[string]any{}
				for key, value := range property {
					copied[key] = value
				}
				copied["description"] = field.doc
				property = copied
			}
			properties[field.name] = property
		}
		return map[string]any{"type": "object", "properties": properties}, nil
	}
	return nil, fmt.Errorf("%s cannot be configuration", t)
}

var (
	buildInfo     *debug.BuildInfo
	buildInfoOnce sync.Once
)

// moduleVersion is the version of the module the package `pkg` belongs to:
// a dependency's version, or the main module's; for a local build of the
// main module, its VCS revision (+dirty when modified). "" when unknown.
func moduleVersion(pkg string) string {
	buildInfoOnce.Do(func() { buildInfo, _ = debug.ReadBuildInfo() })
	if buildInfo == nil || pkg == "" {
		return ""
	}
	within := func(module string) bool { return pkg == module || strings.HasPrefix(pkg, module+"/") }
	best := ""
	version := ""
	for _, dep := range buildInfo.Deps {
		if within(dep.Path) && len(dep.Path) > len(best) {
			best, version = dep.Path, dep.Version
			if dep.Replace != nil && dep.Replace.Version != "" {
				version = dep.Replace.Version
			}
		}
	}
	if best == "" && within(buildInfo.Main.Path) {
		version = buildInfo.Main.Version
	}
	if version == "" || version == "(devel)" {
		revision, modified := "", false
		for _, setting := range buildInfo.Settings {
			switch setting.Key {
			case "vcs.revision":
				revision = setting.Value
			case "vcs.modified":
				modified = setting.Value == "true"
			}
		}
		if revision == "" || best != "" {
			return ""
		}
		if modified {
			revision += "+dirty"
		}
		return revision
	}
	return version
}
