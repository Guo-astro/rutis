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
)

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
		if ty, ok := schema["type"].(string); ok {
			nullable := map[string]any{}
			for key, value := range schema {
				nullable[key] = value
			}
			nullable["type"] = []any{ty, "null"}
			return nullable, nil
		}
		return schema, nil
	case reflect.Interface:
		return map[string]any{}, nil
	case reflect.Slice, reflect.Array:
		items, err := schemaFor(t.Elem(), seen)
		if err != nil {
			return nil, err
		}
		return map[string]any{"type": "array", "items": items}, nil
	case reflect.Map:
		if t.Key().Kind() != reflect.String {
			return nil, fmt.Errorf("%s: only string keys are configuration", t)
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
		for _, field := range reflect.VisibleFields(t) {
			if !field.IsExported() || field.Anonymous {
				continue
			}
			name := field.Name
			if tag, ok := field.Tag.Lookup("json"); ok {
				tagName, rest, _ := strings.Cut(tag, ",")
				if tagName == "-" && rest == "" {
					continue
				}
				if tagName != "" {
					name = tagName
				}
			}
			property, err := schemaFor(field.Type, seen)
			if err != nil {
				return nil, fmt.Errorf("%s.%s: %w", t, field.Name, err)
			}
			if doc, ok := field.Tag.Lookup("doc"); ok {
				copied := map[string]any{}
				for key, value := range property {
					copied[key] = value
				}
				copied["description"] = doc
				property = copied
			}
			properties[name] = property
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
