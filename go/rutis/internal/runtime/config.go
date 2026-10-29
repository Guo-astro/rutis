package rutis

import (
	"encoding/json"
	"errors"
	"reflect"
	"strings"

	"github.com/arcships/rutis/go/rutis/internal/config"
	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/peer"
)

// configured is the config `t` a row's Apply gets: the config as it came,
// decoded. The host's loader validated it against the plugin's schema and
// filled its defaults in (config schema design §2.1). With a schema
// (`schema`), a config that does not decode into `t` (the schema and the
// type disagree) fails with a `bad-config` BindingError, before Apply
// runs, written `<path> is not a value the plugin accepts`, never the
// decoder's text (§3.3); without one, with the decoder's error.
func configured(schema bool, given any, t reflect.Type) (reflect.Value, error) {
	decoded, err := convert.FromPeer(given, t)
	if err == nil || !schema {
		return decoded, err
	}
	return reflect.Value{}, badConfig([]config.Issue{config.At(decoderPath(err, t), "is not a value the plugin accepts")})
}

// decoderPath is where encoding/json says a value did not decode, as far
// as it can be told: its `Field` names struct fields only, so the path
// stops at the first map or array on the way (whose key or index it does
// not say).
func decoderPath(err error, t reflect.Type) []any {
	var mismatch *json.UnmarshalTypeError
	if !errors.As(err, &mismatch) || mismatch.Field == "" {
		return nil
	}
	path := []any{}
	at := t
	for _, key := range strings.Split(mismatch.Field, ".") {
		for at.Kind() == reflect.Pointer {
			at = at.Elem()
		}
		if at.Kind() != reflect.Struct {
			break
		}
		var next reflect.Type
		for _, field := range jsonFields(at) {
			if field.name == key {
				next = field.typ
			}
		}
		if next == nil {
			// The Go name of an embedded struct whose fields are
			// promoted (encoding/json names it in Field): not a key.
			if embedded, ok := at.FieldByName(key); ok && embedded.Anonymous {
				at = embedded.Type
				continue
			}
			break
		}
		path = append(path, key)
		at = next
	}
	return path
}

// badConfig is the error a row fails with when its config does not
// match: a BindingError with code `bad-config`, its message a line
// `config: <problem>` per problem (at most 20), its data `{issues}`
// (design §5.2).
func badConfig(issues []config.Issue) error {
	return peer.NewCoded("BindingError", "bad-config", config.Data(issues), "%s", strings.Join(config.Lines(issues), "\n"))
}
