package rutis

import (
	"context"
	"net"
	"reflect"
	"sort"
	"strings"

	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// Tester is the runtime rutistest runs in the test process: an ordinary
// runtime whose plugins are found by name as rows load them (`find`), the
// plugins the tests load.
type Tester struct{ rt *runtimeState }

// NewTester is a runtime finding its plugins with `find`.
func NewTester(find func(name string) *Definition) *Tester {
	rt := newRuntime(nil)
	rt.find = find
	return &Tester{rt: rt}
}

// Serve serves the session on `conn` (one message per line), greeting as
// the endpoint `id`, until it ends, then unloads every row.
func (t *Tester) Serve(conn net.Conn, id string) error {
	return serveRuntime(peer.NewLines(conn), runnerEndpoint(id, ""), t.rt)
}

// Provided are the services the row `id` (its id in the configuration)
// provides now, sorted.
func (t *Tester) Provided(id string) []string {
	t.rt.mu.Lock()
	defer t.rt.mu.Unlock()
	var names []string
	for _, service := range t.rt.services {
		if r := t.rt.rows[service.row]; r != nil && r.name == id {
			for scoped, provided := range t.rt.services {
				if provided.row == service.row {
					names = append(names, strings.SplitN(scoped, "\x00", 2)[0])
				}
			}
			break
		}
	}
	sort.Strings(names)
	return names
}

// ShapeOf is the shape of a service of the type `t`: its exported
// methods, all asynchronous (MethodsOf, for a type known only as a value).
func ShapeOf(t reflect.Type) *Shape { return shapeOf(t, nil) }

// CallRaw calls `method` of `s` and gives its result as it crossed,
// settled, not decoded: for tests checking values on the wire.
func CallRaw(ctx context.Context, s *Service, method string, args ...any) (any, error) {
	converted, err := convert.ToPeerArgs(args)
	if err != nil {
		return nil, err
	}
	value, err := s.call(ctx, method, converted)
	if err != nil {
		return nil, err
	}
	if s.local != nil {
		// A row of this process: its result as it would cross.
		return convert.ToPeer(value)
	}
	return peer.Settle(ctx, value)
}
