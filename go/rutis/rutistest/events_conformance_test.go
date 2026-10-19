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
// tools are checked against the same list. What each expectation means is
// crates/rutis-bridge/src/runtime/testing/events.rs.
const eventExpectations = "../../../crates/rutis-bridge/src/runtime/testing/events.expected.json"

const (
	// eventLanguage is the language of the Go testing tool in the file.
	eventLanguage = "go"
	// eventSettle is how long a log expectation may take to hold
	// (events.rs SETTLE).
	eventSettle = 5 * time.Second
	// eventLacks is how long `logLacks` / `reported` wait after the rest
	// held (events.rs LACKS).
	eventLacks = 300 * time.Millisecond
)

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

// eventStep is the part of a step the case's labels come from.
type eventStep struct {
	Emit *struct {
		Row string `json:"row"`
	} `json:"emit"`
	Load *struct {
		Label string `json:"label"`
	} `json:"load"`
	Concurrent []eventStep `json:"concurrent"`
}

// emitRows are the emitter rows of a step: its emit's, or its concurrent
// ones'.
func (s eventStep) emitRows() []string {
	if s.Emit != nil {
		if s.Emit.Row == "" {
			return []string{"e"}
		}
		return []string{s.Emit.Row}
	}
	var rows []string
	for _, inner := range s.Concurrent {
		rows = append(rows, inner.emitRows()...)
	}
	return rows
}

// labels are the labels of the case (events.rs `places`): its listeners,
// its loaded participants and its emitter rows.
func (c *eventCase) labels(t *testing.T) map[string]bool {
	labels := map[string]bool{}
	for _, raw := range c.Listeners {
		var listener struct{ Label string }
		if err := json.Unmarshal(raw, &listener); err != nil {
			t.Fatal(err)
		}
		labels[listener.Label] = true
	}
	for _, raw := range c.Steps {
		var step eventStep
		if err := json.Unmarshal(raw, &step); err != nil {
			t.Fatal(err)
		}
		if step.Load != nil {
			labels[step.Load.Label] = true
		}
		for _, row := range step.emitRows() {
			labels[row] = true
		}
	}
	return labels
}

// holds is whether a limit (an only / except item) holds with every label
// at X in Go (events.rs `limit_holds`): every label it names (X, Y or the
// case's) is of one of its languages.
func holds(t *testing.T, limit map[string]json.RawMessage, labels map[string]bool) bool {
	for key, raw := range limit {
		if key == "why" {
			continue
		}
		var list []string
		if err := json.Unmarshal(raw, &list); err != nil {
			t.Fatal(err)
		}
		if !(key == "X" || key == "Y" || labels[key]) || !slices.Contains(list, eventLanguage) {
			return false
		}
	}
	return true
}

func why(limit map[string]json.RawMessage) string {
	var text string
	_ = json.Unmarshal(limit["why"], &text)
	return text
}

// forGo reports whether a case runs on the Go testing tool, where every
// participant is in this runtime, and why not (events.rs `tool_combo`).
func (c *eventCase) forGo(t *testing.T) (bool, string) {
	if c.Tool != nil && !*c.Tool {
		return false, "tool: false"
	}
	labels := c.labels(t)
	for _, limit := range c.Only {
		if !holds(t, limit, labels) {
			return false, "only: " + why(limit)
		}
	}
	for _, limit := range c.Except {
		if holds(t, limit, labels) {
			return false, "except: " + why(limit)
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
	// Whether a background dispatch is running, for the step that ends it.
	background bool
	reported   int
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
	if spec.Mode == "bail" || spec.Mode == "waterfall-sync" {
		method = "dispatchSync"
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	value, err := loaded.Service("emitter-"+key).call(ctx, method, request)
	return outcome{value, err}
}

type expectation struct {
	Returns      json.RawMessage `json:"returns"`
	Throws       *thrown         `json:"throws"`
	Cancelled    bool            `json:"cancelled"`
	Log          []string        `json:"log"`
	LogUnordered []string        `json:"logUnordered"`
	LogContains  []string        `json:"logContains"`
	LogLacks     []string        `json:"logLacks"`
	Reported     []struct {
		Label  string `json:"label"`
		Name   string `json:"name"`
		Filter bool   `json:"filter"`
	} `json:"reported"`
	FilterSees []string `json:"filterSees"`
	ViaHost    *bool    `json:"viaHost"`
	Settles    bool     `json:"settles"`
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

// reports is the error name of a §5.6 report `line` about `event` naming
// `row`, or "" when the line is about something else (events.rs
// `reports`): `rutis: listener of <event> in row <row> failed: <Name>:
// <message>`, or `rutis: filter of <event> from row <row> failed: ...`.
func reports(line, event string, filter bool, row string) string {
	kind, at := "listener", "in"
	if filter {
		kind, at = "filter", "from"
	}
	rest, ok := strings.CutPrefix(line, fmt.Sprintf("rutis: %s of %s %s row %s failed: ", kind, event, at, row))
	if !ok {
		return ""
	}
	name, _, ok := strings.Cut(rest, ": ")
	if !ok {
		return ""
	}
	return name
}

// expectation is a step's expectation, or the empty one.
func (r *eventRun) expectation(raw json.RawMessage) expectation {
	var e expectation
	if raw == nil {
		return e
	}
	if err := json.Unmarshal(raw, &e); err != nil {
		r.t.Fatal(err)
	}
	return e
}

// checkOutcomes checks `returns` and `throws` against the dispatches that
// end in a step (events.rs `check_outcomes`): without `throws` every one
// must succeed.
func (r *eventRun) checkOutcomes(where string, e expectation, outcomes []outcome) {
	if e.Cancelled {
		r.t.Errorf("%s: cancelled: the testing tool does not give up a dispatch", where)
		return
	}
	if (e.Returns != nil || e.Throws != nil) && len(outcomes) == 0 {
		r.t.Errorf("%s: expects a result, but no dispatch ends in this step", where)
		return
	}
	for _, got := range outcomes {
		if e.Throws == nil && got.err != nil {
			r.t.Errorf("%s: expected the dispatch to succeed, it threw %v", where, got.err)
			continue
		}
		if e.Returns != nil {
			var want any
			_ = json.Unmarshal(e.Returns, &want)
			normalized, _ := json.Marshal(written(r.t, got.value))
			var have any
			_ = json.Unmarshal(normalized, &have)
			if !reflect.DeepEqual(have, want) {
				r.t.Errorf("%s: got %s, want %s", where, normalized, e.Returns)
			}
		}
	}
	if e.Throws != nil {
		problem := fmt.Sprintf("expected an error, got %v", outcomes)
		for _, got := range outcomes {
			if got.err == nil {
				continue
			}
			if problem = matches(got.err, *e.Throws); problem == "" {
				break
			}
		}
		if problem != "" {
			r.t.Errorf("%s: %s", where, problem)
		}
	}
}

// logProblems is what of a step's log expectations does not hold now, or
// "" (events.rs `log_holds`).
func (r *eventRun) logProblems(e expectation, event string) string {
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
	lines := r.root.Reported()[r.reported:]
	for _, want := range e.Reported {
		var about []string
		for _, line := range lines {
			if reports(line, event, want.Filter, want.Label) != "" {
				about = append(about, line)
			}
		}
		switch {
		case len(about) != 1:
			out = append(out, fmt.Sprintf("reported %q: want one line for %s about %s, got %d", lines, want.Label, event, len(about)))
		case reports(about[0], event, want.Filter, want.Label) != want.Name:
			out = append(out, fmt.Sprintf("reported %q: %s should fail with %s", about[0], want.Label, want.Name))
		}
	}
	return strings.Join(out, "; ")
}

// checkLog checks a step's log expectations until they hold, at most
// eventSettle; with `logLacks` or `reported`, once more a moment later
// (events.rs `check_log`).
func (r *eventRun) checkLog(where string, e expectation, event string) {
	deadline := time.Now().Add(eventSettle)
	for {
		problem := r.logProblems(e, event)
		if problem == "" {
			break
		}
		if time.Now().After(deadline) {
			r.t.Errorf("%s: %s", where, problem)
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	if e.LogLacks != nil || e.Reported != nil {
		time.Sleep(eventLacks)
		if problem := r.logProblems(e, event); problem != "" {
			r.t.Errorf("%s: %s (after the step had settled)", where, problem)
		}
	}
}

func (r *eventRun) waitFor(label string) {
	deadline := time.Now().Add(eventSettle)
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
	e := r.expectation(step.Expect)
	if e.ViaHost != nil && *e.ViaHost {
		r.t.Errorf("%s: the testing tool always dispatches on the fast path", where)
	}
	// The event the step's reports are about: its emit's name, else the
	// case's.
	event := r.c.Name
	if step.Emit != nil {
		var spec struct {
			Name string `json:"name"`
		}
		_ = json.Unmarshal(step.Emit, &spec)
		if spec.Name != "" {
			event = spec.Name
		}
	}
	var outcomes []outcome
	started := time.Now()
	switch {
	case step.Emit != nil && step.Background:
		r.background = true
		go func() { r.pending <- r.emit(step.Emit) }()
		r.checkLog(where, e, event)
		return
	case step.Emit != nil:
		outcomes = append(outcomes, r.emit(step.Emit))
	case step.Load != nil:
		r.load(step.Load)
	case step.Update != "":
		r.update(step.Update)
	case step.Unload != "":
		if step.When != "" {
			r.waitFor(step.When)
		}
		r.rows[step.Unload].Unload()
		if r.background {
			r.background = false
			select {
			case got := <-r.pending:
				outcomes = append(outcomes, got)
			case <-time.After(eventSettle):
				r.t.Errorf("%s: the background dispatch did not settle within %v", where, eventSettle)
			}
		}
		if e.Settles && time.Since(started) >= eventSettle {
			r.t.Errorf("%s: did not settle within %v", where, eventSettle)
		}
	default:
		r.t.Fatalf("%s: a step the testing tool cannot take: %s", where, raw)
	}
	r.checkOutcomes(where, e, outcomes)
	r.checkLog(where, e, event)
}

// knownFailing are the cases a product bug fails here, and why: skipped,
// and counted so that the tool still runs as many as the file says.
var knownFailing = map[string]string{}

func TestRutistestMeetsTheEventExpectations(t *testing.T) {
	data, err := os.ReadFile(filepath.FromSlash(eventExpectations))
	if err != nil {
		t.Fatal(err)
	}
	var expected struct {
		Scopes    map[string]string `json:"scopes"`
		ToolCases map[string]int    `json:"toolCases"`
		Cases     []eventCase       `json:"cases"`
	}
	if err := json.Unmarshal(data, &expected); err != nil {
		t.Fatal(err)
	}
	ran, known := 0, 0
	for i := range expected.Cases {
		c := &expected.Cases[i]
		t.Run(c.Case, func(t *testing.T) {
			if ok, why := c.forGo(t); !ok {
				t.Skipf("not for the Go testing tool: %s", why)
			}
			if why, failing := knownFailing[c.Case]; failing {
				known++
				t.Skipf("known failing: %s", why)
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
	// The skip logic agrees with events.rs `tool_combo`.
	if ran+known != expected.ToolCases[eventLanguage] {
		t.Fatalf("ran %d cases and %d known failing, the file gives %s %d", ran, known, eventLanguage, expected.ToolCases[eventLanguage])
	}
}
