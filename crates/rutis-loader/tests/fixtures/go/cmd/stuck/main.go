// A binary whose plugin never finishes its cleanup.
package main

import (
	"example.com/fixtures/plugins/stuck"

	"github.com/arcships/rutis/go/rutis"
)

func main() { rutis.Serve(stuck.Plugin) }
