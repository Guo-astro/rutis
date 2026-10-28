// Package advanced holds what a plugin author rarely writes (#692): the
// SDK's version constants, the errors one way of emitting returns, the
// values Go has no type for, serving and manifests by hand, the options
// beyond `MethodsOf` and `Sync`, and the types of objects of another
// process. Package rutis has the plugin, its config, services and events.
//
//	import "github.com/arcships/rutis/go/rutis/advanced"
//
//	day := advanced.DateOf(time.Now())
package advanced

import (
	"net"
	"time"

	"github.com/arcships/rutis/go/rutis"
	"github.com/arcships/rutis/go/rutis/internal/convert"
	"github.com/arcships/rutis/go/rutis/internal/events"
	"github.com/arcships/rutis/go/rutis/internal/peer"
	runtime "github.com/arcships/rutis/go/rutis/internal/runtime"
	"github.com/arcships/rutis/go/rutis/internal/values"
)

// ── Versions ─────────────────────────────────────────────────────

// Version is this SDK's version: the runtime's implementation version and
// the manifest's `sdk`. The release train checks it.
const Version = runtime.Version

// PluginAPI is the plugin API this SDK writes plugins against and its
// runtime supports.
const PluginAPI = runtime.PluginAPI

// Implementation is the name the runtime reports itself by: the module it
// is published as.
const Implementation = runtime.Implementation

// ── Errors of the ways of emitting ───────────────────────────────

// The names of the event model's errors, for errors.Is: they match an
// error of that name made here or received from another process.
var (
	// ErrReservedEvent: a plugin emitted a `rutis/*` event.
	ErrReservedEvent = events.ErrReservedEvent
	// ErrWaterfallNext: `next` called twice, or after its waterfall ended.
	ErrWaterfallNext = events.ErrWaterfallNext
	// ErrAsyncListener: a Bail reached an asynchronous listener of another
	// language.
	ErrAsyncListener = events.ErrAsyncListener
)

// ErrSyncWaitCycle matches (errors.Is) a SyncWaitCycle error from another
// process.
var ErrSyncWaitCycle = peer.ErrSyncWaitCycle

// AggregateError reports every failed listener of a Parallel dispatch, in
// registration order, even when only one failed. Unwrap lets errors.Is/As
// inspect each failure.
type AggregateError = peer.AggregateError

// ── Serving by hand ──────────────────────────────────────────────

// Manifest is what the plugins `defs` declare, as --rutis-manifest prints
// it.
func Manifest(defs ...*rutis.Definition) ([]byte, error) { return runtime.Manifest(defs...) }

// Project is the project directory the host gave this runtime.
func Project() string { return runtime.Project() }

// ServeArgs is Serve with the arguments given, returning instead of
// exiting: for binaries with commands of their own.
func ServeArgs(args []string, defs ...*rutis.Definition) error {
	return runtime.ServeArgs(args, defs...)
}

// ServeConn serves the plugins `defs` on `conn` (one message per line),
// greeting as the endpoint `id`, until the session ends, then unloads them.
func ServeConn(conn net.Conn, id string, defs ...*rutis.Definition) error {
	return runtime.ServeConn(conn, id, defs...)
}

// ── Values ───────────────────────────────────────────────────────

// DateTime is a local date and time without a time zone (tag
// `datetime`): it crosses as it is and is never read as a point in time
// in some zone (Python's datetime without tzinfo, Kotlin's LocalDateTime).
// DateTimeOf and DateTime.In convert from and to time.Time. Its text is
// `2026-10-15T14:30:05.25`.
type DateTime = values.DateTime

// Date is a calendar date (tag `date`), years 0001–9999: `2026-10-15`.
type Date = values.Date

// TimeOfDay is a time of day without a time zone (tag `time`), 24-hour,
// to the nanosecond: `14:30:05.25`.
type TimeOfDay = values.TimeOfDay

// Decimal is a decimal number kept as its text (tag `decimal`): an
// optional `-`, digits, an optional fraction; no exponent; its trailing
// zeros kept (`1.10` is not `1.1`). A Decimal that is not such text fails
// the call that sends it; ParseDecimal checks it first.
type Decimal = values.Decimal

// UUID is a UUID (tag `uuid`) in lowercase `8-4-4-4-12` hex. A UUID that
// is not such text fails the call that sends it; ParseUUID checks it
// first and lowercases it.
type UUID = values.UUID

// DateOf is the date of t in t's location.
func DateOf(t time.Time) Date { return values.DateOf(t) }

// TimeOfDayOf is the time of day of t in t's location.
func TimeOfDayOf(t time.Time) TimeOfDay { return values.TimeOfDayOf(t) }

// DateTimeOf is the date and time of day of t in t's location.
func DateTimeOf(t time.Time) DateTime { return values.DateTimeOf(t) }

// ParseDate reads `YYYY-MM-DD`.
func ParseDate(text string) (Date, error) { return values.ParseDate(text) }

// ParseTimeOfDay reads `HH:MM:SS` with an optional fraction of 1–9
// digits.
func ParseTimeOfDay(text string) (TimeOfDay, error) { return values.ParseTimeOfDay(text) }

// ParseDateTime reads `YYYY-MM-DDTHH:MM:SS` with an optional fraction of
// 1–9 digits and no time zone.
func ParseDateTime(text string) (DateTime, error) { return values.ParseDateTime(text) }

// ParseDecimal checks decimal text: `-?(0|[1-9][0-9]*)(\.[0-9]+)?`, at
// most 4300 digits.
func ParseDecimal(text string) (Decimal, error) { return values.ParseDecimal(text) }

// ParseUUID reads `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` in either case
// and returns it in lowercase.
func ParseUUID(text string) (UUID, error) { return values.ParseUUID(text) }

// ── Events and services ──────────────────────────────────────────

// Next is the rest of a waterfall: a waterfall listener gets it as its last
// argument, and calls it at most once.
type Next = events.Next

// Prepend puts the listener before every listener registered so far,
// instead of after them.
func Prepend() rutis.ListenOption { return runtime.Prepend() }

// Global exempts the listener from emitters' filters (Filter).
func Global() rutis.ListenOption { return runtime.Global() }

// Rename gives the Go method `method` the wire name `wire`.
func Rename(method, wire string) rutis.ShapeOption { return runtime.Rename(method, wire) }

// Function is a function of another process, received where a value of
// any type was expected.
type Function = convert.Function

// Future is an asynchronous result of another process.
type Future = convert.Future

// Service is a service used by name, or an object with methods of another
// process received as `any` (#687 §四): its methods call the original
// object; it has no method table.
type Service = runtime.Service

// Schemer is a configuration type that gives its own JSON Schema.
type Schemer = runtime.Schemer
