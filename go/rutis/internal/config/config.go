// Package config writes the error a row fails with when its config does
// not decode into the plugin's config type: `config: <path> is not a value
// the plugin accepts` (config schema design,
// docs/design-config-schema-2026-10-27.en.md §3.3, §5.2). Validating a
// config and filling its defaults is the kernel's (`rutis::config::check`):
// the host's loader sends the runtime the config with its defaults filled
// in, and the runtime only decodes it.
package config

import (
	"fmt"
	"slices"
	"strconv"
	"strings"
)

// Shown is how many problems are written; the rest as one last line.
const Shown = 20

// Issue is one problem: where it is (keys as strings, indices as ints)
// and its sentence, path included (`home.lat is not a value the plugin
// accepts`).
type Issue struct {
	Path    []any
	Message string
}

// At is the problem at `path`, its sentence `<path> <reason>`.
func At(path []any, reason string) Issue {
	return Issue{Path: slices.Clone(path), Message: PathText(path) + " " + reason}
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

// PathText is how a path is written: `home.lat`, `cities[2]`,
// `headers["x-api"]`; `config` for the root (and before a first segment
// that is not an identifier key).
func PathText(path []any) string {
	var out strings.Builder
	for i, segment := range path {
		switch s := segment.(type) {
		case string:
			if identifier(s) {
				if i > 0 {
					out.WriteByte('.')
				}
				out.WriteString(s)
				continue
			}
			if i == 0 {
				out.WriteString("config")
			}
			out.WriteString("[" + quoted(s) + "]")
		case int:
			if i == 0 {
				out.WriteString("config")
			}
			out.WriteString("[" + strconv.Itoa(s) + "]")
		}
	}
	if out.Len() == 0 {
		return "config"
	}
	return out.String()
}

// identifier: `[A-Za-z_][A-Za-z0-9_]*`.
func identifier(key string) bool {
	if key == "" {
		return false
	}
	for i := 0; i < len(key); i++ {
		c := key[i]
		letter := c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c == '_'
		if !letter && (i == 0 || c < '0' || c > '9') {
			return false
		}
	}
	return true
}

// quoted is `s` as a JSON string: `"`, `\` and control characters
// (U+0000–U+001F) escaped, everything else as it is.
func quoted(s string) string {
	var out strings.Builder
	out.WriteByte('"')
	for _, c := range s {
		switch c {
		case '"':
			out.WriteString(`\"`)
		case '\\':
			out.WriteString(`\\`)
		case '\n':
			out.WriteString(`\n`)
		case '\r':
			out.WriteString(`\r`)
		case '\t':
			out.WriteString(`\t`)
		case '\b':
			out.WriteString(`\b`)
		case '\f':
			out.WriteString(`\f`)
		default:
			if c < 0x20 {
				fmt.Fprintf(&out, `\u%04x`, c)
			} else {
				out.WriteRune(c)
			}
		}
	}
	out.WriteByte('"')
	return out.String()
}
