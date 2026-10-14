package peer

import (
	"encoding/json"
	"errors"
	"fmt"
	"reflect"
	"strings"
)

// RemoteError is an error thrown on the other side of the session.
type RemoteError struct {
	Name    string
	Message string
	Graph   json.RawMessage
}

func (e *RemoteError) Error() string {
	if e.Name == "" {
		return e.Message
	}
	return e.Name + ": " + e.Message
}

// Is makes errors.Is(err, ErrSyncWaitCycle) hold for a received cycle.
func (e *RemoteError) Is(target error) bool {
	return target == ErrSyncWaitCycle && e.Name == "SyncWaitCycle"
}

// ErrSyncWaitCycle matches a SyncWaitCycle error received from the far end.
var ErrSyncWaitCycle = errors.New("SyncWaitCycle")

// AggregateError holds every failed event listener in registration order.
// It crosses as Cordis's AggregateError (an empty message, the failures in
// `errors`); Error reads as Rust's CordisError::Aggregate.
type AggregateError struct{ Errors []error }

func (e *AggregateError) Error() string {
	messages := make([]string, 0, len(e.Errors))
	for _, err := range e.Errors {
		if err != nil {
			messages = append(messages, err.Error())
		}
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
	var nodes []any
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
			if json.Unmarshal(remote.Graph, &graph) == nil && graph.Root != nil {
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
// RemoteError, none for an AggregateError (as Cordis's), else Error().
func wireMessage(err error) string {
	switch err := err.(type) {
	case *RemoteError:
		return err.Message
	case *AggregateError:
		return ""
	}
	return err.Error()
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
	return &RemoteError{Name: name, Message: message, Graph: graph}
}
