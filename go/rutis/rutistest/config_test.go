package rutistest_test

import (
	"testing"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/rutistest"
)

type forecast struct {
	City string `json:"city"`
	Days int    `json:"days"`
}

// A typed config through the test tool: the host checks it against the
// schema derived from the type, the plugin's row failing with the host's
// line; a config it passes reaches Apply, at load and at Update; one
// Update refuses leaves the row as it was.
func TestATypedConfigInTheTestTool(t *testing.T) {
	var got forecast
	plugin := rutis.Define(rutis.Plugin[forecast]{
		Name:  "weather",
		Apply: func(_ *rutis.Ctx, config forecast) error { got = config; return nil },
	})
	_, err := rutistest.TryLoad(t, plugin, map[string]any{"days": "three"}, nil)
	if err == nil || err.Error() != `weather: config: days should be an integer, got "three"` {
		t.Fatalf("%v", err)
	}
	loaded := rutistest.Load(t, plugin, forecast{City: "Oslo"}, nil)
	if got != (forecast{City: "Oslo"}) {
		t.Fatalf("%+v", got)
	}
	loaded.Update(map[string]any{"city": "Bergen", "days": 2})
	if got != (forecast{City: "Bergen", Days: 2}) {
		t.Fatalf("%+v", got)
	}
}
