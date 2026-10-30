// Package optional has plugins using a service when it is there
// (`Optional`), and one declaring a name both required and optional.
package optional

import (
	"context"
	"errors"
	"fmt"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/advanced"
)

// Config names the service the row provides (`as`); `round` only restarts
// it when changed.
type Config struct {
	As    string `json:"as"`
	Round int    `json:"round"`
}

// Of answers with what the optional service's method returns, the error's
// code and message, or `none` when the service was not there at start.
type Of struct {
	service *advanced.Service
	method  string
}

func (o *Of) Of(ctx context.Context) (string, error) {
	if o.service == nil {
		return "none", nil
	}
	var answer any
	if err := o.service.Call(ctx, o.method, &answer); err != nil {
		var remote *rutis.RemoteError
		if errors.As(err, &remote) {
			return remote.Code + ": " + remote.Message, nil
		}
		return "", err
	}
	return fmt.Sprint(answer), nil
}

// Uses is a plugin `name` using `service` when there, calling its
// `method`.
func Uses(name, service, method string) *rutis.Definition {
	return rutis.Define(rutis.Plugin[Config]{
		Name:     name,
		Optional: []string{service},
		Apply: func(ctx *rutis.Ctx, config Config) error {
			of := &Of{method: method}
			var found advanced.Service
			ok, err := ctx.Get(service, &found)
			if err != nil {
				return err
			}
			if ok {
				of.service = &found
			}
			ctx.Provide(config.As, of)
			return nil
		},
	})
}

// Both declares `llm` both required and optional.
var Both = rutis.Define(rutis.Plugin[rutis.NoConfig]{
	Name:     "both",
	Inject:   []string{"llm"},
	Optional: []string{"llm"},
	Apply:    func(*rutis.Ctx, rutis.NoConfig) error { return nil },
})
