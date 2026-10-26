package peer

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"math/big"
	"reflect"
	"sort"
	"strconv"
	"strings"
)

// RemoteError is an error thrown on the other side of the session.
type RemoteError struct {
	Name    string
	Message string
	Graph   json.RawMessage
	// Code and Data are what a Rust service's Err(e) carries (#440): its
	// code (the variant's name) and data (e's serde value, decoded as
	// encoding/json does, big integers as *big.Int); empty and nil otherwise.
	Code string
	Data any
}

func (e *RemoteError) Error() string {
	if e.Name == "" {
		return e.Message
	}
	return e.Name + ": " + e.Message
}

// Is makes errors.Is(err, ErrSyncWaitCycle) hold for a received cycle,
// and errors.Is(err, sentinel) for a Sentinel of its name.
func (e *RemoteError) Is(target error) bool {
	if sentinel, ok := target.(*Sentinel); ok {
		return sentinel.name == e.Name
	}
	return target == ErrSyncWaitCycle && e.Name == "SyncWaitCycle"
}

// Sentinel matches (errors.Is) every error of one cross-language name,
// made in this process (an *Error) or received (a *RemoteError).
type Sentinel struct{ name string }

// NewSentinel is the sentinel of the error name `name`.
func NewSentinel(name string) *Sentinel { return &Sentinel{name} }

func (s *Sentinel) Error() string { return s.name }

// Error is an error with a cross-language name, made in this process.
type Error struct{ name, message string }

// Errorf makes an *Error named `name`.
func Errorf(name, format string, args ...any) error {
	return &Error{name: name, message: fmt.Sprintf(format, args...)}
}

func (e *Error) Error() string { return e.message }
func (e *Error) Name() string  { return e.name }
func (e *Error) Is(target error) bool {
	sentinel, ok := target.(*Sentinel)
	return ok && sentinel.name == e.name
}

// ErrIncompatible is what a session ends with when the far end greeted in
// a way this side cannot speak (another protocol version, no valid
// endpoint): retrying cannot fix it.
var ErrIncompatible = errors.New("incompatible session")

// ErrEndpointMismatch is what a session ends with when the far end greeted
// as another endpoint than the expected one.
var ErrEndpointMismatch = errors.New("endpoint mismatch")

// ErrSyncWaitCycle matches a SyncWaitCycle error received from the far end.
var ErrSyncWaitCycle = errors.New("SyncWaitCycle")

// AggregateError holds every failed event listener in registration order.
// It crosses as Cordis's AggregateError (its Message, the failures in
// `errors`).
type AggregateError struct {
	// Message is what it crosses with: `<n> listeners of <name> failed`
	// for a parallel dispatch.
	Message string
	Errors  []error
}

func (e *AggregateError) Error() string {
	messages := make([]string, 0, len(e.Errors))
	for _, err := range e.Errors {
		if err != nil {
			messages = append(messages, err.Error())
		}
	}
	if e.Message != "" {
		return e.Message + ": [" + strings.Join(messages, "; ") + "]"
	}
	return "multiple errors: [" + strings.Join(messages, "; ") + "]"
}
func (e *AggregateError) Name() string    { return "AggregateError" }
func (e *AggregateError) Unwrap() []error { return e.Errors }

// PanicError is a recovered panic, thrown with its stack.
type PanicError struct {
	Value any
	Stack string
}

func (e *PanicError) Error() string { return fmt.Sprintf("panic: %v", e.Value) }
func (e *PanicError) Name() string  { return "Panic" }

// ErrorName is the name an error crosses with: Name() when it has one, the
// far end's name for a RemoteError, "Error" for the standard library's
// plain errors, else its type name (*fs.PathError -> PathError).
func ErrorName(err error) string {
	if remote, ok := err.(*RemoteError); ok {
		return remote.Name
	}
	if named, ok := err.(interface{ Name() string }); ok {
		return named.Name()
	}
	t := reflect.TypeOf(err)
	for t.Kind() == reflect.Pointer {
		t = t.Elem()
	}
	switch t.PkgPath() + "." + t.Name() {
	case "errors.errorString", "fmt.wrapError", "fmt.wrapErrors", "errors.joinError":
		return "Error"
	}
	if t.Name() == "" {
		return "Error"
	}
	return t.Name()
}

// encodeError is the wire form of err: its error graph (an AggregateError's
// children in order, cyclic aggregates once). A RemoteError thrown on,
// inside an aggregate or alone, crosses with the far end's graph spliced
// in (nodes appended, references shifted), as Node does: it decodes into a
// real Error whose re-encoding carries the whole graph.
func encodeError(err error) map[string]any {
	if remote, ok := err.(*RemoteError); ok && remote.Name == "SyncWaitCycle" && remote.Graph == nil {
		return map[string]any{"name": remote.Name, "message": remote.Message}
	}
	// Never null on the wire, even when nothing is added to it (a thrown
	// value relayed, its root plain data): JavaScript refuses such a graph
	// (#675).
	nodes := []any{}
	seen := map[*AggregateError]int{}
	var visit func(error) map[string]any
	visit = func(err error) map[string]any {
		if aggregate, ok := err.(*AggregateError); ok {
			if id, found := seen[aggregate]; found {
				return map[string]any{"type": "reference", "value": id}
			}
			seen[aggregate] = len(nodes)
		}
		if remote, ok := err.(*RemoteError); ok && len(remote.Graph) != 0 {
			var graph struct {
				Root  map[string]any
				Nodes []any
			}
			// A graph without its nodes (what Go sent before #675) is not
			// spliced in: the error crosses as a node of its own.
			if json.Unmarshal(remote.Graph, &graph) == nil && graph.Root != nil && graph.Nodes != nil {
				offset := len(nodes)
				var shift func(any) any
				shift = func(value any) any {
					switch value := value.(type) {
					case map[string]any:
						if value["type"] == "reference" {
							if id, ok := value["value"].(float64); ok {
								value["value"] = id + float64(offset)
							}
						} else {
							for key, item := range value {
								value[key] = shift(item)
							}
						}
					case []any:
						for i, item := range value {
							value[i] = shift(item)
						}
					}
					return value
				}
				for _, node := range graph.Nodes {
					nodes = append(nodes, shift(node))
				}
				return shift(graph.Root).(map[string]any)
			}
		}
		name, message := ErrorName(err), wireMessage(err)
		id := len(nodes)
		node := map[string]any{"type": "error", "name": name, "message": message, "stack": name + ": " + message}
		nodes = append(nodes, node)
		// A code and its data cross, as from JavaScript and Rust (#440):
		// an error with `Code() string` and `Data() any`. Data that cannot
		// cross is left out, nothing of it kept.
		if coded, ok := err.(interface{ Code() string }); ok {
			node["code"] = coded.Code()
		}
		if carrying, ok := err.(interface{ Data() any }); ok {
			mark := len(nodes)
			if data, ok := graphData(carrying.Data(), &nodes); ok {
				node["data"] = data
			} else {
				nodes = nodes[:mark]
			}
		}
		if aggregate, ok := err.(*AggregateError); ok {
			children := make([]any, 0, len(aggregate.Errors))
			for _, child := range aggregate.Errors {
				if child != nil {
					children = append(children, visit(child))
				}
			}
			node["errors"] = children
		} else {
			var panicked *PanicError
			if errors.As(err, &panicked) {
				node["stack"] = node["stack"].(string) + "\n" + panicked.Stack
			}
		}
		return map[string]any{"type": "reference", "value": id}
	}
	root := visit(err)
	return map[string]any{"name": ErrorName(err), "message": wireMessage(err), "graph": map[string]any{"root": root, "nodes": nodes}}
}

// wireMessage is the message an error crosses with: the far end's own for a
// RemoteError, its Message for an AggregateError, else Error().
func wireMessage(err error) string {
	switch err := err.(type) {
	case *RemoteError:
		return err.Message
	case *AggregateError:
		return err.Message
	}
	return err.Error()
}

// Message is the message an error crosses with.
func Message(err error) string { return wireMessage(err) }

// Aggregated is a received AggregateError as an *AggregateError whose
// failures are the graph's children in order (each a *RemoteError carrying
// the graph from it on); any other error is returned as it is.
func Aggregated(err error) error {
	remote, ok := err.(*RemoteError)
	if !ok || remote.Name != "AggregateError" || len(remote.Graph) == 0 {
		return err
	}
	var graph struct {
		Root  graphValue                   `json:"root"`
		Nodes []map[string]json.RawMessage `json:"nodes"`
	}
	if json.Unmarshal(remote.Graph, &graph) != nil || graph.Root.Type != "reference" {
		return err
	}
	var id int
	if json.Unmarshal(graph.Root.Value, &id) != nil || id < 0 || id >= len(graph.Nodes) {
		return err
	}
	var children []graphValue
	if json.Unmarshal(graph.Nodes[id]["errors"], &children) != nil {
		return err
	}
	nodes, marshalErr := json.Marshal(graph.Nodes)
	if marshalErr != nil {
		return err
	}
	out := &AggregateError{Message: remote.Message}
	for _, child := range children {
		var childID int
		if child.Type != "reference" || json.Unmarshal(child.Value, &childID) != nil || childID < 0 || childID >= len(graph.Nodes) {
			out.Errors = append(out.Errors, &RemoteError{Name: "Error", Message: "malformed error"})
			continue
		}
		node := graph.Nodes[childID]
		var name, message string
		_ = json.Unmarshal(node["name"], &name)
		_ = json.Unmarshal(node["message"], &message)
		if name == "" {
			name = "Error"
		}
		sub, _ := json.Marshal(map[string]any{"root": map[string]any{"type": "reference", "value": childID}, "nodes": json.RawMessage(nodes)})
		failure := &RemoteError{Name: name, Message: message, Graph: sub}
		failure.Code, failure.Data = codeAndData(sub)
		out.Errors = append(out.Errors, failure)
	}
	return out
}

func decodeError(raw json.RawMessage) error {
	var failure struct {
		Name    any             `json:"name"`
		Message any             `json:"message"`
		Graph   json.RawMessage `json:"graph"`
	}
	if err := json.Unmarshal(raw, &failure); err != nil {
		return &RemoteError{Name: "Error", Message: "malformed error"}
	}
	name, message := "Error", ""
	if failure.Name != nil {
		name = fmt.Sprint(failure.Name)
	}
	if failure.Message != nil {
		message = fmt.Sprint(failure.Message)
	}
	graph := failure.Graph
	if strings.TrimSpace(string(graph)) == "null" {
		graph = nil
	}
	remote := &RemoteError{Name: name, Message: message, Graph: graph}
	remote.Code, remote.Data = codeAndData(graph)
	return remote
}

// codeAndData reads the code and data of the error a graph's root refers to.
func codeAndData(raw json.RawMessage) (string, any) {
	if len(raw) == 0 {
		return "", nil
	}
	var graph struct {
		Root  graphValue                   `json:"root"`
		Nodes []map[string]json.RawMessage `json:"nodes"`
	}
	if json.Unmarshal(raw, &graph) != nil || graph.Root.Type != "reference" {
		return "", nil
	}
	node := func(value json.RawMessage) map[string]json.RawMessage {
		var id int
		if json.Unmarshal(value, &id) != nil || id < 0 || id >= len(graph.Nodes) {
			return nil
		}
		return graph.Nodes[id]
	}
	root := node(graph.Root.Value)
	if root == nil {
		return "", nil
	}
	var code string
	_ = json.Unmarshal(root["code"], &code)
	var read func(json.RawMessage, int) (any, bool)
	read = func(raw json.RawMessage, depth int) (any, bool) {
		var value graphValue
		if depth > 64 || json.Unmarshal(raw, &value) != nil {
			return nil, false
		}
		switch value.Type {
		case "undefined":
			return nil, true
		case "data":
			var data any
			return data, json.Unmarshal(value.Value, &data) == nil
		case "number":
			var text string
			if json.Unmarshal(value.Value, &text) != nil {
				return nil, false
			}
			switch text {
			case "NaN":
				return math.NaN(), true
			case "Infinity":
				return math.Inf(1), true
			case "-Infinity":
				return math.Inf(-1), true
			}
			number, err := strconv.ParseFloat(text, 64)
			return number, err == nil
		case "map", "set":
			var items []json.RawMessage
			if json.Unmarshal(value.Value, &items) != nil {
				return nil, false
			}
			var out []any
			for _, item := range items {
				parts := []json.RawMessage{item}
				if value.Type == "map" {
					if json.Unmarshal(item, &parts) != nil || len(parts) != 2 {
						return nil, false
					}
				}
				for _, part := range parts {
					decoded, ok := read(part, depth+1)
					if !ok {
						return nil, false
					}
					out = append(out, decoded)
				}
			}
			if value.Type == "set" {
				return Set(out), CheckKeys(out, "set element") == nil
			}
			m := make(Map, len(out)/2)
			for i := range m {
				m[i] = MapEntry{Key: out[2*i], Value: out[2*i+1]}
			}
			return m, CheckKeys(m.keys(), "map key") == nil
		case "reference":
			target := node(value.Value)
			var kind string
			if target == nil || json.Unmarshal(target["type"], &kind) != nil {
				return nil, false
			}
			switch kind {
			case "array":
				var items []json.RawMessage
				if json.Unmarshal(target["values"], &items) != nil {
					return nil, false
				}
				list := make([]any, 0, len(items))
				for _, item := range items {
					element, ok := read(item, depth+1)
					if !ok {
						return nil, false
					}
					list = append(list, element)
				}
				return list, true
			case "object":
				var entries [][2]json.RawMessage
				if json.Unmarshal(target["values"], &entries) != nil {
					return nil, false
				}
				object := make(map[string]any, len(entries))
				for _, entry := range entries {
					var key string
					if json.Unmarshal(entry[0], &key) != nil {
						return nil, false
					}
					element, ok := read(entry[1], depth+1)
					if !ok {
						return nil, false
					}
					object[key] = element
				}
				return object, true
			}
		default:
			// The other tagged values, as the session decodes them.
			var text string
			if !isTag(value.Type) || json.Unmarshal(value.Value, &text) != nil {
				return nil, false
			}
			decoded, err := parseTagged(value.Type, text)
			return decoded, err == nil
		}
		return nil, false
	}
	var data any
	if raw, found := root["data"]; found {
		data, _ = read(raw, 0)
	}
	return code, data
}

// graphData is `value` (an error's Data) as an error graph value: its JSON
// form, integers beyond ±(2^53−1) as bigints, arrays and objects as nodes.
func graphData(value any, nodes *[]any) (map[string]any, bool) {
	raw, err := json.Marshal(value)
	if err != nil {
		return nil, false
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.UseNumber()
	var data any
	if decoder.Decode(&data) != nil {
		return nil, false
	}
	var visit func(any, int) (map[string]any, bool)
	visit = func(value any, depth int) (map[string]any, bool) {
		if depth > 64 {
			return nil, false
		}
		switch value := value.(type) {
		case json.Number:
			if integer, ok := new(big.Int).SetString(string(value), 10); ok && integer.CmpAbs(big.NewInt(1<<53-1)) > 0 {
				return map[string]any{"type": "bigint", "value": integer.String()}, true
			}
			return map[string]any{"type": "data", "value": value}, true
		case []any:
			id := len(*nodes)
			*nodes = append(*nodes, nil)
			values := make([]any, 0, len(value))
			for _, item := range value {
				encoded, ok := visit(item, depth+1)
				if !ok {
					return nil, false
				}
				values = append(values, encoded)
			}
			(*nodes)[id] = map[string]any{"type": "array", "values": values}
			return map[string]any{"type": "reference", "value": id}, true
		case map[string]any:
			id := len(*nodes)
			*nodes = append(*nodes, nil)
			keys := make([]string, 0, len(value))
			for key := range value {
				keys = append(keys, key)
			}
			sort.Strings(keys)
			values := make([]any, 0, len(value))
			for _, key := range keys {
				encoded, ok := visit(value[key], depth+1)
				if !ok {
					return nil, false
				}
				values = append(values, []any{key, encoded})
			}
			(*nodes)[id] = map[string]any{"type": "object", "values": values}
			return map[string]any{"type": "reference", "value": id}, true
		default:
			return map[string]any{"type": "data", "value": value}, true
		}
	}
	return visit(data, 0)
}

type graphValue struct {
	Type  string          `json:"type"`
	Value json.RawMessage `json:"value"`
}
