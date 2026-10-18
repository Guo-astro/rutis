package rutistest

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"sort"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis/internal/fixtures/plugins/conformance"
	"github.com/arcships/rutis/go/rutis/internal/peer"
	"github.com/arcships/rutis/go/rutis/internal/values"
)

// The event expectations (#464 §13): the real runtimes and the testing
// tools are checked against the same list. Until the file is in the
// checkout, its starting cases are read from the specification, whose
// §13.3 is the file's first content.
const (
	eventExpectations = "../../../crates/rutis-bridge/src/runtime/testing/events.expected.json"
	eventSpec         = "../../../docs/design-events-protocol-2026-10-18.md"
)

func readEventExpectations(t *testing.T) []byte {
	if data, err := os.ReadFile(filepath.FromSlash(eventExpectations)); err == nil {
		return data
	}
	doc, err := os.ReadFile(filepath.FromSlash(eventSpec))
	if err != nil {
		t.Skipf("neither the event expectations nor the specification are in this checkout: %v", err)
	}
	text := string(doc)
	start := strings.Index(text, "### 13.3")
	if start < 0 {
		t.Fatal("the specification has no §13.3")
	}
	text = text[start:]
	open := strings.Index(text, "```json\n")
	close := strings.Index(text[open+8:], "\n```")
	if open < 0 || close < 0 {
		t.Fatal("§13.3 has no JSON block")
	}
	return []byte(text[open+8 : open+8+close])
}

type eventCase struct {
	Case      string                       `json:"case"`
	Rule      string                       `json:"rule"`
	Name      string                       `json:"name"`
	Listeners []json.RawMessage            `json:"listeners"`
	Steps     []json.RawMessage            `json:"steps"`
	Only      []map[string]json.RawMessage `json:"only"`
	Except    []map[string]json.RawMessage `json:"except"`
	Tool      *bool                        `json:"tool"`
}

// languages are the language lists of an only / except item.
func languages(item map[string]json.RawMessage) [][]string {
	var out [][]string
	for key, raw := range item {
		if key == "why" {
			continue
		}
		var list []string
		if json.Unmarshal(raw, &list) == nil {
			out = append(out, list)
		}
	}
	return out
}

// forGo reports whether a case runs on the Go testing tool, where every
// participant is in this runtime, and why not.
func (c *eventCase) forGo() (bool, string) {
	if c.Tool != nil && !*c.Tool {
		return false, "it needs another process, rutis/* or the host"
	}
	for _, item := range c.Only {
		if goAlso[c.Case] {
			break
		}
		for _, list := range languages(item) {
			if !slices.Contains(list, "go") {
				return false, fmt.Sprintf("only %v", list)
			}
		}
	}
	for _, item := range c.Except {
		for _, list := range languages(item) {
			if slices.Contains(list, "go") {
				return false, fmt.Sprintf("except %v", list)
			}
		}
	}
	return true, ""
}

type eventLog struct {
	mu      sync.Mutex
	entries []string
}

// Add is events-log.add.
func (l *eventLog) Add(entry string) {
	l.mu.Lock()
	l.entries = append(l.entries, entry)
	l.mu.Unlock()
}

// Filter is events-log.filter: it logs filter:<row> of the description.
func (l *eventLog) Filter(description map[string]any) {
	row, _ := description["row"].(string)
	l.Add("filter:" + row)
}

func (l *eventLog) list() []string {
	l.mu.Lock()
	defer l.mu.Unlock()
	return append([]string(nil), l.entries...)
}

// fromWritten is a value as the expectations write it, as the session
// carries it: tagged values from {"$<tag>": …}, a callback from
// {"$callback": "upper"}.
func fromWritten(t *testing.T, raw json.RawMessage) any {
	var object map[string]json.RawMessage
	if json.Unmarshal(raw, &object) == nil && len(object) == 1 {
		for key, inner := range object {
			if !strings.HasPrefix(key, "$") {
				break
			}
			tag := key[1:]
			switch tag {
			case "callback":
				return func(_ context.Context, text string) (string, error) { return strings.ToUpper(text), nil }
			case "map":
				var pairs [][2]json.RawMessage
				if err := json.Unmarshal(inner, &pairs); err != nil {
					t.Fatal(err)
				}
				m := make(peer.Map, len(pairs))
				for i, pair := range pairs {
					m[i] = peer.MapEntry{Key: fromWritten(t, pair[0]), Value: fromWritten(t, pair[1])}
				}
				return m
			case "set":
				var items []json.RawMessage
				if err := json.Unmarshal(inner, &items); err != nil {
					t.Fatal(err)
				}
				s := make(peer.Set, len(items))
				for i, item := range items {
					s[i] = fromWritten(t, item)
				}
				return s
			}
			var text string
			if err := json.Unmarshal(inner, &text); err != nil {
				t.Fatal(err)
			}
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
				value = values.Decimal(text)
			case "uuid":
				value = values.UUID(text)
			default:
				t.Fatalf("unknown tag %s", tag)
			}
			if err != nil {
				t.Fatal(err)
			}
			return value
		}
	}
	var list []json.RawMessage
	if json.Unmarshal(raw, &list) == nil && list != nil && strings.HasPrefix(strings.TrimSpace(string(raw)), "[") {
		out := make([]any, len(list))
		for i, item := range list {
			out[i] = fromWritten(t, item)
		}
		return out
	}
	return raw
}

// eventRun is one case on one harness.
type eventRun struct {
	t      *testing.T
	c      *eventCase
	groups map[string]string
	log    *eventLog
	root   *Loaded
	rows   map[string]*Loaded
	// The listener rows' configs, by label, and the last config generation.
	participants map[string]map[string]any
	generation   int
	pending      chan outcome
	reported     int
}

type outcome struct {
	value any
	err   error
}

func (r *eventRun) options(label, path string) []Option {
	if path == "" {
		path = "root"
	}
	var groups []string
	if path != "root" {
		segments := strings.Split(path, "/")
		for i := range segments {
			groups = append(groups, r.groups[strings.Join(segments[:i+1], "/")])
		}
	}
	return []Option{Row(label), InScope(path, groups...)}
}

// load loads a listener row.
func (r *eventRun) load(raw json.RawMessage) {
	var listener map[string]any
	if err := json.Unmarshal(raw, &listener); err != nil {
		r.t.Fatal(err)
	}
	if _, ok := listener["name"]; !ok {
		listener["name"] = r.c.Name
	}
	label, _ := listener["label"].(string)
	scope, _ := listener["scope"].(string)
	r.rows[label] = r.root.Load(listener, r.options(label, scope)...)
	r.participants[label] = listener
}

// update changes the config of the row `label` (a new generation), so it
// registers its listeners again.
func (r *eventRun) update(label string) {
	participant, ok := r.participants[label]
	if !ok {
		r.t.Fatalf("no listener row %s to update", label)
	}
	r.generation++
	config := make(map[string]any, len(participant)+1)
	for key, value := range participant {
		config[key] = value
	}
	config["generation"] = r.generation
	r.rows[label].Update(config)
	r.participants[label] = config
}

// emitter is the emitter row of `label` in `scope`, created on first use.
func (r *eventRun) emitter(label, scope string) (*Loaded, string) {
	key := label
	if scope != "" && scope != "root" {
		key = label + "-" + strings.ReplaceAll(scope, "/", "-")
	}
	if _, ok := r.rows[key]; !ok {
		r.rows[key] = r.root.Load(map[string]any{"label": key, "emitter": true}, r.options(key, scope)...)
	}
	return r.rows[key], key
}

type emitSpec struct {
	From     string            `json:"from"`
	Row      string            `json:"row"`
	Scope    string            `json:"scope"`
	Mode     string            `json:"mode"`
	Name     string            `json:"name"`
	Args     []json.RawMessage `json:"args"`
	Filter   json.RawMessage   `json:"filter"`
	Terminal json.RawMessage   `json:"terminal"`
	Cancel   json.RawMessage   `json:"cancel"`
	Via      string            `json:"via"`
}

func (r *eventRun) emit(raw json.RawMessage) outcome {
	var spec emitSpec
	if err := json.Unmarshal(raw, &spec); err != nil {
		r.t.Fatal(err)
	}
	if spec.Row == "" {
		spec.Row = "e"
	}
	if spec.Name == "" {
		spec.Name = r.c.Name
	}
	loaded, key := r.emitter(spec.Row, spec.Scope)
	args := make([]any, len(spec.Args))
	for i, arg := range spec.Args {
		args[i] = fromWritten(r.t, arg)
	}
	request := map[string]any{"mode": spec.Mode, "name": spec.Name, "args": args}
	if spec.Filter != nil {
		request["filter"] = spec.Filter
	}
	if spec.Terminal != nil {
		var terminal any
		if err := json.Unmarshal(spec.Terminal, &terminal); err != nil {
			r.t.Fatal(err)
		}
		request["terminal"] = terminal
	}
	if spec.Cancel != nil {
		request["cancel"] = spec.Cancel
	}
	method := "dispatch"
	if spec.Mode == "bail" {
		method = "dispatchSync"
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	value, err := loaded.Service("emitter-"+key).call(ctx, method, request)
	return outcome{value, err}
}

type expectation struct {
	Returns       json.RawMessage `json:"returns"`
	Throws        *thrown         `json:"throws"`
	Log           []string        `json:"log"`
	LogUnordered  []string        `json:"logUnordered"`
	LogContains   []string        `json:"logContains"`
	LogLacks      []string        `json:"logLacks"`
	Reported      []struct{ Label, Name string }
	FilterSees    []string `json:"filterSees"`
	ViaHost       *bool    `json:"viaHost"`
	Settles       bool     `json:"settles"`
	ReturnsGiven  bool     `json:"-"`
	ThrowsUnnamed bool     `json:"-"`
}

type thrown struct {
	Name    string   `json:"name"`
	Message string   `json:"message"`
	Errors  []thrown `json:"errors"`
}

// failure is the error as the emitter's runtime threw it.
func matches(err error, want thrown) string {
	var remote *peer.RemoteError
	if !errors.As(err, &remote) {
		return fmt.Sprintf("not an error of the plugin: %v", err)
	}
	if want.Name != "" && remote.Name != want.Name {
		return fmt.Sprintf("name %s, want %s (%v)", remote.Name, want.Name, err)
	}
	if want.Message != "" && !strings.Contains(remote.Message, want.Message) {
		return fmt.Sprintf("message %q, want %q", remote.Message, want.Message)
	}
	if want.Errors != nil {
		var aggregate *peer.AggregateError
		if !errors.As(peer.Aggregated(remote), &aggregate) || len(aggregate.Errors) != len(want.Errors) {
			return fmt.Sprintf("errors of %v, want %d", err, len(want.Errors))
		}
		for i, child := range want.Errors {
			if problem := matches(aggregate.Errors[i], child); problem != "" {
				return fmt.Sprintf("errors[%d]: %s", i, problem)
			}
		}
	}
	return ""
}

func multiset(list []string) []string {
	out := append([]string(nil), list...)
	sort.Strings(out)
	return out
}

func containsInOrder(log, want []string) bool {
	i := 0
	for _, entry := range log {
		if i < len(want) && entry == want[i] {
			i++
		}
	}
	return i == len(want)
}

// check checks a step's expectation, waiting up to 5 s for the log.
func (r *eventRun) check(where string, raw json.RawMessage, got *outcome) {
	if raw == nil {
		return
	}
	var e expectation
	if err := json.Unmarshal(raw, &e); err != nil {
		r.t.Fatal(err)
	}
	if e.ViaHost != nil && *e.ViaHost {
		r.t.Errorf("%s: the testing tool always dispatches on the fast path", where)
	}
	if got != nil {
		switch {
		case e.Throws != nil:
			if got.err == nil {
				r.t.Errorf("%s: expected an error, got %v", where, got.value)
			} else if problem := matches(got.err, *e.Throws); problem != "" {
				r.t.Errorf("%s: %s", where, problem)
			}
		case e.Returns != nil:
			if got.err != nil {
				r.t.Errorf("%s: expected %s, got %v", where, e.Returns, got.err)
				break
			}
			var want any
			_ = json.Unmarshal(e.Returns, &want)
			value := written(r.t, got.value)
			normalized, _ := json.Marshal(value)
			var have any
			_ = json.Unmarshal(normalized, &have)
			if !reflect.DeepEqual(have, want) {
				r.t.Errorf("%s: got %s, want %s", where, normalized, e.Returns)
			}
		}
	}
	problems := func() string {
		log := r.log.list()
		var out []string
		if e.Log != nil && !slices.Equal(log, e.Log) {
			out = append(out, fmt.Sprintf("log %q, want %q", log, e.Log))
		}
		if e.LogUnordered != nil && !slices.Equal(multiset(log), multiset(e.LogUnordered)) {
			out = append(out, fmt.Sprintf("log %q, want in any order %q", log, e.LogUnordered))
		}
		if e.LogContains != nil && !containsInOrder(log, e.LogContains) {
			out = append(out, fmt.Sprintf("log %q, want it to contain %q", log, e.LogContains))
		}
		for _, lacking := range e.LogLacks {
			if slices.Contains(log, lacking) {
				out = append(out, fmt.Sprintf("log %q has %q", log, lacking))
			}
		}
		if e.FilterSees != nil {
			var seen []string
			for _, entry := range log {
				if row, ok := strings.CutPrefix(entry, "filter:"); ok {
					seen = append(seen, row)
				}
			}
			if !slices.Equal(seen, e.FilterSees) {
				out = append(out, fmt.Sprintf("the filter saw %q, want %q", seen, e.FilterSees))
			}
		}
		reported := r.root.Reported()[r.reported:]
		for _, want := range e.Reported {
			prefix := fmt.Sprintf("rutis: listener of %s in row %s failed: %s:", r.c.Name, want.Label, want.Name)
			if !slices.ContainsFunc(reported, func(line string) bool { return strings.HasPrefix(line, prefix) }) {
				out = append(out, fmt.Sprintf("reported %q, want %s", reported, prefix))
			}
		}
		return strings.Join(out, "; ")
	}
	deadline := time.Now().Add(5 * time.Second)
	for {
		problem := problems()
		if problem == "" {
			return
		}
		if time.Now().After(deadline) {
			r.t.Errorf("%s: %s", where, problem)
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func (r *eventRun) waitFor(label string) {
	deadline := time.Now().Add(5 * time.Second)
	for !slices.Contains(r.log.list(), label) {
		if time.Now().After(deadline) {
			r.t.Fatalf("%s did not start: %q", label, r.log.list())
		}
		time.Sleep(5 * time.Millisecond)
	}
}

func (r *eventRun) step(i int, raw json.RawMessage) {
	var step struct {
		Emit       json.RawMessage `json:"emit"`
		Background bool            `json:"background"`
		Expect     json.RawMessage `json:"expect"`
		Load       json.RawMessage `json:"load"`
		Unload     string          `json:"unload"`
		Update     string          `json:"update"`
		When       string          `json:"when"`
	}
	if err := json.Unmarshal(raw, &step); err != nil {
		r.t.Fatal(err)
	}
	where := fmt.Sprintf("%s step %d", r.c.Case, i)
	switch {
	case step.Emit != nil && step.Background:
		go func() { r.pending <- r.emit(step.Emit) }()
	case step.Emit != nil:
		got := r.emit(step.Emit)
		r.check(where, step.Expect, &got)
	case step.Load != nil:
		r.load(step.Load)
		r.check(where, step.Expect, nil)
	case step.Update != "":
		r.update(step.Update)
		r.check(where, step.Expect, nil)
	case step.Unload != "":
		if step.When != "" {
			r.waitFor(step.When)
		}
		r.rows[step.Unload].Unload()
		var got *outcome
		select {
		case background := <-r.pending:
			got = &background
		default:
		}
		r.check(where, step.Expect, got)
	default:
		r.t.Fatalf("%s: a step the testing tool cannot take: %s", where, raw)
	}
}

// goAlso are cases whose `only` predates every language having Prepend and
// Global listener options (a maintainer's correction to §15.27): Go runs
// them too.
var goAlso = map[string]bool{"filter/global-skips-filter": true, "order/prepend": true}

// goDiffers are the cases a Go listener cannot meet, and why: bounded,
// written-down differences of Go's value model, to be marked in the
// expectations (except) rather than here once the file is in the checkout.
var goDiffers = map[string]string{
	"values/every-tag": "a set received where any value is expected decodes into a slice in Go (Go has no set type; design V, convert.Plain), so an echoing Go listener returns it as a list",
}

func TestRutistestMeetsTheEventExpectations(t *testing.T) {
	var expected struct {
		Scopes map[string]string `json:"scopes"`
		Cases  []eventCase       `json:"cases"`
	}
	if err := json.Unmarshal(readEventExpectations(t), &expected); err != nil {
		t.Fatal(err)
	}
	ran := 0
	for i := range expected.Cases {
		c := &expected.Cases[i]
		t.Run(c.Case, func(t *testing.T) {
			if ok, why := c.forGo(); !ok {
				t.Skipf("not for the Go testing tool: %s", why)
			}
			if why, differs := goDiffers[c.Case]; differs {
				t.Skipf("a written-down difference of Go: %s", why)
			}
			ran++
			log := &eventLog{}
			r := &eventRun{t: t, c: c, groups: expected.Scopes, log: log, rows: map[string]*Loaded{}, participants: map[string]map[string]any{}, pending: make(chan outcome, 1)}
			r.root = Load(t, conformance.Plugin, map[string]any{"label": "e", "emitter": true}, map[string]any{"events-log": log}, Row("e"))
			r.rows["e"] = r.root
			r.reported = len(r.root.Reported())
			for _, listener := range c.Listeners {
				r.load(listener)
			}
			for i, step := range c.Steps {
				r.step(i, step)
			}
		})
	}
	if ran == 0 {
		t.Fatal("no case ran")
	}
}
