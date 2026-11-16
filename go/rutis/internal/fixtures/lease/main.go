// The lease test plugin (crates/rutis-host/tests/lease_e2e.rs), alone in a
// binary: it writes its start, its calls and its cleanup to the log its
// config names, provides the `log` service, and its cleanup waits for a
// gate file, so the test decides when the old lease is gone.
package main

import (
	"context"
	"fmt"
	"os"
	"time"

	"github.com/arcships/rutis/go/rutis"
)

// Config says who this lease serves, where the log is, and the two gates:
// the cleanup waits for Stop, Hold waits for Hold.
type Config struct {
	Who  string `json:"who"`
	Log  string `json:"log"`
	Stop string `json:"stop"`
	Hold string `json:"hold"`
}

// Logger is the `log` service the plugin provides.
type Logger struct {
	who  string
	log  string
	hold string
}

// Who answers with the name this lease serves.
func (l *Logger) Who(ctx context.Context) (string, error) { return l.who, nil }

// Hold is an instruction of this lease that waits until the test opens its
// gate.
func (l *Logger) Hold(ctx context.Context) (string, error) {
	if err := write(l.log, "hold "+l.who); err != nil {
		return "", err
	}
	waitFor(l.hold)
	return "held " + l.who, nil
}

func write(path, line string) error {
	file, err := os.OpenFile(path, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return err
	}
	defer file.Close()
	_, err = fmt.Fprintln(file, line)
	return err
}

func waitFor(path string) {
	for {
		if _, err := os.Stat(path); err == nil {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
}

var plugin = rutis.Define(rutis.Plugin[Config]{
	Name:     "logger",
	Provides: rutis.Provides{"log": rutis.MethodsOf[*Logger]()},
	Apply: func(ctx *rutis.Ctx, config Config) error {
		if err := write(config.Log, "start "+config.Who); err != nil {
			return err
		}
		ctx.Provide("log", &Logger{who: config.Who, log: config.Log, hold: config.Hold})
		ctx.Effect(func(context.Context) error {
			if err := write(config.Log, "cleanup "+config.Who); err != nil {
				return err
			}
			waitFor(config.Stop)
			return write(config.Log, "stop "+config.Who)
		})
		return nil
	},
})

func main() { rutis.Serve(plugin) }
