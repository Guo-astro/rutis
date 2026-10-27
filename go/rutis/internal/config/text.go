package config

import (
	"encoding/json"
	"fmt"
	"math"
	"strconv"
	"strings"
)

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
			out.WriteString("[" + quoted(s, -1) + "]")
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

// ValueText is how a value is written after `got` (and in `enum` lists):
// strings, numbers, booleans and `null` as JSON, a string past 40 scalar
// values cut with `…`; `an object`, `an array of N`.
func ValueText(value any) string {
	switch v := value.(type) {
	case nil:
		return "null"
	case bool:
		return strconv.FormatBool(v)
	case json.Number:
		return NumberText(v)
	case string:
		return quoted(v, 40)
	case []any:
		return fmt.Sprintf("an array of %d", len(v))
	}
	return "an object"
}

// NumberText is a number as JavaScript's `String(number)` writes it; an
// integer written without a fraction as it is (when it fits 64 bits).
func NumberText(n json.Number) string {
	if integerText(n) {
		// As parsed (`-0` is 0, as in JavaScript).
		if i, err := strconv.ParseInt(string(n), 10, 64); err == nil {
			return strconv.FormatInt(i, 10)
		}
		if u, err := strconv.ParseUint(string(n), 10, 64); err == nil {
			return strconv.FormatUint(u, 10)
		}
	}
	f, _ := number(n)
	return FloatText(f)
}

// FloatText is `f` as JavaScript's `String(number)` writes it; an integral
// value of magnitude at most 2^53 without a decimal point.
func FloatText(f float64) string {
	switch {
	case math.IsNaN(f):
		return "NaN"
	case math.IsInf(f, 1):
		return "Infinity"
	case math.IsInf(f, -1):
		return "-Infinity"
	case f == 0:
		return "0"
	case f == math.Trunc(f) && math.Abs(f) <= exact:
		return strconv.FormatInt(int64(f), 10)
	}
	// The shortest round-trip digits and the exponent, as `d.ddde±n`.
	shortest := strconv.FormatFloat(math.Abs(f), 'e', -1, 64)
	mantissa, exponent, _ := strings.Cut(shortest, "e")
	digits := strings.ReplaceAll(mantissa, ".", "")
	e, _ := strconv.Atoi(exponent)
	k := len(digits)
	// JavaScript's n: the position of the decimal point.
	n := e + 1
	sign := ""
	if f < 0 {
		sign = "-"
	}
	var body string
	switch {
	case k <= n && n <= 21:
		body = digits + strings.Repeat("0", n-k)
	case 0 < n && n <= 21:
		body = digits[:n] + "." + digits[n:]
	case -6 < n && n <= 0:
		body = "0." + strings.Repeat("0", -n) + digits
	default:
		mark := fmt.Sprintf("e+%d", n-1)
		if n-1 < 0 {
			mark = fmt.Sprintf("e-%d", 1-n)
		}
		if k == 1 {
			body = digits + mark
		} else {
			body = digits[:1] + "." + digits[1:] + mark
		}
	}
	return sign + body
}

// quoted is `s` as a JSON string, its content cut to `limit` scalar values
// with `…` (no cut for a negative limit); `"`, `\` and control characters
// (U+0000–U+001F) escaped, everything else as it is.
func quoted(s string, limit int) string {
	var out strings.Builder
	out.WriteByte('"')
	count := 0
	cut := false
	for _, c := range s {
		if limit >= 0 && count == limit {
			cut = true
			break
		}
		count++
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
	if cut {
		out.WriteString("…")
	}
	out.WriteByte('"')
	return out.String()
}

// Closest is the key of `known` (sorted by code point) closest to `name`,
// when it is close enough to be a misspelling of it: none for a name of
// 0–2 scalar values, at most one edit for 3–6, two for longer.
func Closest(name string, known []string) (string, bool) {
	count := len([]rune(name))
	if count <= 2 {
		return "", false
	}
	limit := 1
	if count > 6 {
		limit = 2
	}
	best, found, at := 0, false, ""
	for _, candidate := range known {
		if candidate == name {
			continue
		}
		d := distance(name, candidate)
		if d <= limit && (!found || d < best) {
			best, found, at = d, true, candidate
		}
	}
	return at, found
}

// distance is the edits (insert, delete, replace, swap two neighbours)
// between `a` and `b`, by scalar value.
func distance(a, b string) int {
	x, y := []rune(a), []rune(b)
	rows := make([][]int, len(x)+1)
	for i := range rows {
		rows[i] = make([]int, len(y)+1)
		rows[i][0] = i
	}
	for j := range rows[0] {
		rows[0][j] = j
	}
	for i := 1; i <= len(x); i++ {
		for j := 1; j <= len(y); j++ {
			cost := 1
			if x[i-1] == y[j-1] {
				cost = 0
			}
			best := min(rows[i-1][j]+1, rows[i][j-1]+1, rows[i-1][j-1]+cost)
			if i > 1 && j > 1 && x[i-1] == y[j-2] && x[i-2] == y[j-1] {
				best = min(best, rows[i-2][j-2]+1)
			}
			rows[i][j] = best
		}
	}
	return rows[len(x)][len(y)]
}
