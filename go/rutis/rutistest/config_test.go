package rutistest_test

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/rutistest"
)

type configCase struct {
	Name     string                     `json:"name"`
	Schema   map[string]any             `json:"schema"`
	Config   json.RawMessage            `json:"config"`
	Then     map[string]json.RawMessage `json:"then"`
	Warnings []string                   `json:"warnings"`
}

func configCases(t *testing.T) []configCase {
	t.Helper()
	data, err := os.ReadFile(filepath.Join("..", "..", "..", "specs", "config-schema", "cases.json"))
	if err != nil {
		t.Fatal(err)
	}
	var list struct {
		Cases []configCase `json:"cases"`
	}
	if err := json.Unmarshal(data, &list); err != nil {
		t.Fatal(err)
	}
	return list.Cases
}

// stderrOf runs `run` and gives what it wrote to standard error.
func stderrOf(t *testing.T, run func()) string {
	t.Helper()
	reader, writer, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	saved := os.Stderr
	os.Stderr = writer
	done := make(chan string)
	go func() {
		var out bytes.Buffer
		_, _ = io.Copy(&out, reader)
		done <- out.String()
	}()
	func() {
		defer func() { os.Stderr = saved; writer.Close() }()
		run()
	}()
	return <-done
}

// decoded is JSON with numbers kept as they are written.
func decoded(t *testing.T, data []byte) any {
	t.Helper()
	if len(data) == 0 {
		return nil
	}
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.UseNumber()
	var value any
	if err := decoder.Decode(&value); err != nil {
		t.Fatal(err)
	}
	return value
}

// The shared case list (config schema design §8) through rutistest, which
// runs the real runtime: what Apply gets, the error TryLoad gives back,
// the warnings on standard error.
func TestEveryConfigCase(t *testing.T) {
	for _, c := range configCases(t) {
		t.Run(c.Name, func(t *testing.T) {
			var got json.RawMessage
			applied := false
			plugin := rutis.Define(rutis.Plugin[json.RawMessage]{
				Name:   "weather",
				Schema: c.Schema,
				Apply: func(_ *rutis.Ctx, config json.RawMessage) error {
					got, applied = config, true
					return nil
				},
			})
			var given any
			if c.Config != nil {
				given = c.Config
			}
			var err error
			stderr := stderrOf(t, func() {
				_, err = rutistest.TryLoad(t, plugin, given, nil, rutistest.Row("weather"))
			})
			var warned []string
			for _, line := range strings.Split(strings.TrimRight(stderr, "\n"), "\n") {
				if line != "" {
					warned = append(warned, strings.TrimPrefix(line, "weather: "))
				}
			}
			if !reflect.DeepEqual(warned, c.Warnings) && (len(warned) > 0 || len(c.Warnings) > 0) {
				t.Errorf("warnings %q, want %q", warned, c.Warnings)
			}
			if errors, failing := c.Then["errors"]; failing {
				var want []string
				if err := json.Unmarshal(errors, &want); err != nil {
					t.Fatal(err)
				}
				if applied {
					t.Fatal("Apply ran")
				}
				remote := badConfig(t, err)
				if remote.Message != strings.Join(want, "\n") {
					t.Fatalf("%q, want %q", remote.Message, strings.Join(want, "\n"))
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			if !applied {
				t.Fatal("Apply did not run")
			}
			if want := decoded(t, c.Then["config"]); !reflect.DeepEqual(decoded(t, got), want) {
				t.Fatalf("Apply got %s, want %s", got, c.Then["config"])
			}
		})
	}
}

func badConfig(t *testing.T, err error) *rutis.RemoteError {
	t.Helper()
	var remote *rutis.RemoteError
	if !errors.As(err, &remote) || remote.Name != "BindingError" || remote.Code != "bad-config" {
		t.Fatalf("not a bad-config BindingError: %#v", err)
	}
	return remote
}

type forecast struct {
	City string `json:"city"`
	Days int    `json:"days"`
}

// A typed config through the test tool: the error crosses with its data;
// an Update to a bad config fails as a load does; an extra key is warned
// about once per row and config.
func TestATypedConfigInTheTestTool(t *testing.T) {
	var got forecast
	plugin := rutis.Define(rutis.Plugin[forecast]{
		Name:  "weather",
		Apply: func(_ *rutis.Ctx, config forecast) error { got = config; return nil },
	})
	_, err := rutistest.TryLoad(t, plugin, map[string]any{"days": "three"}, nil)
	remote := badConfig(t, err)
	if remote.Message != `config: days should be an integer, got "three"` {
		t.Fatalf("%q", remote.Message)
	}
	want := map[string]any{"issues": []any{map[string]any{"path": []any{"days"}, "message": `days should be an integer, got "three"`}}}
	if data, _ := json.Marshal(remote.Data); string(data) != mustJSON(t, want) {
		t.Fatalf("%s", data)
	}
	var loaded *rutistest.Loaded
	stderr := stderrOf(t, func() {
		loaded = rutistest.Load(t, plugin, forecast{City: "Oslo"}, nil, rutistest.Row("weather"))
		loaded.Update(map[string]any{"cty": "Oslo"})
		loaded.Update(map[string]any{"cty": "Oslo"})
	})
	if stderr != "weather: config: cty is not a config key (did you mean city?)\n" {
		t.Fatalf("%q", stderr)
	}
	if got != (forecast{}) {
		t.Fatalf("%+v", got)
	}
}

func mustJSON(t *testing.T, value any) string {
	t.Helper()
	data, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}
