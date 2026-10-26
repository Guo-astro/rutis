package main

import (
	"example.com/fixtures/plugins/multilang"
	"example.com/fixtures/plugins/values"

	"github.com/arcships/rutis/go/rutis"
)

func main() {
	rutis.Serve(multilang.Provider, multilang.Consumer, multilang.Local, multilang.Gated, multilang.Row, multilang.Registrant, multilang.CalendarUser, multilang.GoTaker, values.Origin, values.Crash)
}
