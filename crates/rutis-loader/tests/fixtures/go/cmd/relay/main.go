package main

import (
	"example.com/fixtures/plugins/relay"

	"github.com/arcships/rutis/go/rutis"
)

func main() { rutis.Serve(relay.Plugin) }
