// Package errs declares the error types a plugin author meets. Its
// package name is rutis, the package those types are used through, so
// that `%T` and `%#v` print *rutis.RemoteError and not an internal
// package's name (#677).
package rutis

import (
	"encoding/json"
	"errors"
	"fmt"
	"strings"
)

// ErrSyncWaitCycle matches a SyncWaitCycle error received from the far end.
var ErrSyncWaitCycle = errors.New("SyncWaitCycle")

// RemoteError is an error thrown on the other side of the session.
type RemoteError struct {
	Name    string
	Message string
	Graph   json.RawMessage
	// Code and Data are what a Rust service's Err(e) carries (#440): its
	// code (the variant's name) and data (e's serde value, decoded as
	// encoding/json does, big integers as *big.Int); empty and nil otherwise.
	Code string
	Data any
}

func (e *RemoteError) Error() string {
	if e.Name == "" {
		return e.Message
	}
	return e.Name + ": " + e.Message
}

// Is makes errors.Is(err, ErrSyncWaitCycle) hold for a received cycle,
// and errors.Is(err, sentinel) for a Sentinel of its name.
func (e *RemoteError) Is(target error) bool {
	if sentinel, ok := target.(*Sentinel); ok {
		return sentinel.name == e.Name
	}
	return target == ErrSyncWaitCycle && e.Name == "SyncWaitCycle"
}

// Sentinel matches (errors.Is) every error of one cross-language name,
// made in this process (an *Error) or received (a *RemoteError).
type Sentinel struct{ name string }

// NewSentinel is the sentinel of the error name `name`.
func NewSentinel(name string) *Sentinel { return &Sentinel{name} }

func (s *Sentinel) Error() string { return s.name }

// Error is an error with a cross-language name, made in this process.
type Error struct{ name, message string }

// Errorf makes an *Error named `name`.
func Errorf(name, format string, args ...any) error {
	return &Error{name: name, message: fmt.Sprintf(format, args...)}
}

func (e *Error) Error() string { return e.message }
func (e *Error) Name() string  { return e.name }
func (e *Error) Is(target error) bool {
	sentinel, ok := target.(*Sentinel)
	return ok && sentinel.name == e.name
}

// Coded is an error this side makes with a code and data (#677): a refused
// call (`no-method`, `bad-argument`, …), a service not declared or not
// available, the host gone. It crosses with its name, code and data, as an
// error with `Code() string` and `Data() any` does.
type Coded struct {
	name, code, message string
	data                map[string]any
	cause               error
}

// NewCoded makes a *Coded named `name` with `code` and `data`.
func NewCoded(name, code string, data map[string]any, format string, args ...any) *Coded {
	return &Coded{name: name, code: code, message: fmt.Sprintf(format, args...), data: data}
}

func (e *Coded) Error() string { return e.message }
func (e *Coded) Name() string  { return e.name }
func (e *Coded) Code() string  { return e.code }
func (e *Coded) Data() any     { return e.data }
func (e *Coded) Unwrap() error { return e.cause }
func (e *Coded) Is(target error) bool {
	sentinel, ok := target.(*Sentinel)
	return ok && sentinel.name == e.name
}

// HostGone is the error calls fail with once the host disconnected, code
// `peer-gone`; `cause` is why (it unwraps to it), nil when the connection
// just ended.
func HostGone(cause error) *Coded {
	if cause == nil {
		return NewCoded("BindingError", "peer-gone", map[string]any{"reason": "disconnected"}, "the host disconnected")
	}
	gone := NewCoded("BindingError", "peer-gone", map[string]any{"reason": cause.Error()}, "the host disconnected (%s)", cause)
	gone.cause = cause
	return gone
}

// AggregateError holds every failed event listener in registration order.
// It crosses as an AggregateError (its Message, the failures in
// `errors`).
type AggregateError struct {
	// Message is what it crosses with: `<n> listeners of <name> failed`
	// for a parallel dispatch.
	Message string
	Errors  []error
}

func (e *AggregateError) Error() string {
	messages := make([]string, 0, len(e.Errors))
	for _, err := range e.Errors {
		if err != nil {
			messages = append(messages, err.Error())
		}
	}
	if e.Message != "" {
		return e.Message + ": [" + strings.Join(messages, "; ") + "]"
	}
	return "multiple errors: [" + strings.Join(messages, "; ") + "]"
}
func (e *AggregateError) Name() string    { return "AggregateError" }
func (e *AggregateError) Unwrap() []error { return e.Errors }

// PanicError is a recovered panic, thrown with its stack.
type PanicError struct {
	Value any
	Stack string
}

func (e *PanicError) Error() string { return fmt.Sprintf("panic: %v", e.Value) }
func (e *PanicError) Name() string  { return "Panic" }
