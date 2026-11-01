// Plugins using services when they are there.
package main

import (
	"example.com/fixtures/plugins/optional"

	"github.com/arcships/rutis/go/rutis"
)

func main() {
	rutis.Serve(
		optional.Uses("uses_llm", "llm", "ask"),
		optional.Uses("uses_late", "late", "ask"),
		optional.Uses("uses_counter", "counter", "count"),
		optional.Uses("uses_ping", "ping", "echo"),
		optional.Both,
	)
}
