package main

import (
	"example.com/fixtures/plugins/multilang"

	"github.com/arcships/rutis/go/rutis"
)

func main() {
	rutis.Serve(multilang.Provider, multilang.Consumer, multilang.Local, multilang.Gated, multilang.Row, multilang.Registrant, multilang.CalendarUser)
}
