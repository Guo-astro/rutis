// The test plugins in one binary: weather, probe and conformance-events.
package main

import (
	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/internal/fixtures/plugins/conformance"
	"github.com/arcships/rutis/go/rutis/internal/fixtures/plugins/probe"
	"github.com/arcships/rutis/go/rutis/internal/fixtures/plugins/weather"
)

func main() { rutis.Serve(weather.Plugin, probe.Plugin, conformance.Plugin) }
