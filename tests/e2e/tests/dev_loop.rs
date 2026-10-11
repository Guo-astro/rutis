//! S2, the plugin author's development loop (#186): `rutis-host new`, the
//! template's own tests, `check`, `dev`, an edit, a broken edit, a fix,
//! Ctrl-C, in each language `new` supports. Scenario A of quality-status
//! §4.2; the residue checks end it.
//!
//! v1 is the template as `new` writes it; v2 and v3 are written whole by
//! the scenario and print a line when applied and when cleaned up, which is
//! how "the old instance was unloaded, exactly once" is seen. The template
//! tests run against this checkout's packages (linked, never installed
//! from a registry: that is S9, #193). Unix only for now: the residue
//! checks need a Job Object on Windows (#232), and Ctrl-C there is sent
//! differently.

#![cfg(unix)]

use rutis_e2e::{repo, Host, Lang, Probe, Scenario};
use serde_json::{json, Value};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Node,
    Python,
    Bun,
    Go,
}

impl Kind {
    fn lang(self) -> &'static str {
        match self {
            Kind::Node => "node",
            Kind::Python => "python",
            Kind::Bun => "bun",
            Kind::Go => "go",
        }
    }

    /// The plugin's source file, relative to the project.
    fn source(self) -> &'static str {
        match self {
            Kind::Node | Kind::Bun => "src/index.ts",
            Kind::Python => "src/demo/__init__.py",
            Kind::Go => "plugin.go",
        }
    }

    /// The plugin's source for version `n`: the template's greeter, whose
    /// `hello` adds ` (v<n>)`, printing `demo v<n> applied <runtime>` and
    /// `demo v<n> cleanup`.
    fn version(self, n: u32) -> String {
        match self {
            Kind::Node | Kind::Bun => ts_version(n),
            Kind::Python => python_version(n),
            Kind::Go => go_version(n),
        }
    }

    /// v2 with line 5 replaced by a line that does not compile.
    fn broken(self) -> String {
        let mut lines: Vec<String> = self.version(2).lines().map(str::to_owned).collect();
        lines[4] = match self {
            Kind::Node | Kind::Bun => "const const = 1",
            Kind::Python => "def broken(:",
            Kind::Go => "func broken( {",
        }
        .to_owned();
        lines.join("\n") + "\n"
    }

    /// The probe's language, and the directory its file goes in: a Python
    /// probe goes in the Python runtime's project.
    fn probe(self) -> (Lang, &'static str) {
        match self {
            Kind::Node => (Lang::Node, ""),
            // The src layout: the Python runtime's project is src/.
            Kind::Python => (Lang::Python, "src"),
            Kind::Bun | Kind::Go => (Lang::Python, "dev"),
        }
    }

    /// What `rutis.dev.json` adds for the probe.
    fn runtimes(self) -> Value {
        match self {
            Kind::Node | Kind::Python => Value::Null,
            Kind::Bun | Kind::Go => json!({ "py": { "project": "dev" } }),
        }
    }
}

fn ts_version(n: u32) -> String {
    format!(
        r#"import {{ definePlugin }} from '@arcships/rutis'

export interface Config {{
  greeting?: string
}}

class Greeter {{
  constructor(private readonly greeting: string) {{}}

  hello(name: string): string {{
    return `${{this.greeting}}, ${{name}}! (v{n})`
  }}
}}

export default definePlugin<Config>({{
  inject: [],
  provides: {{ greeter: {{ hello: 'sync' }} }},
  config: {{ type: 'object', properties: {{ greeting: {{ type: 'string', default: 'Hello' }} }} }},
  apply(ctx, config) {{
    console.log(`demo v{n} applied ${{process.execPath}}`)
    ctx.provide('greeter', new Greeter(config.greeting ?? 'Hello'))
    return () => console.log('demo v{n} cleanup')
  }},
}})
"#
    )
}

fn python_version(n: u32) -> String {
    format!(
        r#""""demo: a rutis plugin."""

import sys

from rutis import define_plugin


class Greeter:
    def __init__(self, greeting: str) -> None:
        self.greeting = greeting

    def hello(self, name: str) -> str:
        return f"{{self.greeting}}, {{name}}! (v{n})"


def apply(ctx, config):
    print(f"demo v{n} applied {{sys.executable}}", flush=True)
    ctx.provide("greeter", Greeter(config.get("greeting", "Hello")))
    return lambda: print("demo v{n} cleanup", flush=True)


plugin = define_plugin(
    apply,
    inject=[],
    provides={{"greeter": Greeter}},
    config={{"type": "object", "properties": {{"greeting": {{"type": "string", "default": "Hello"}}}}}},
)
"#
    )
}

/// Go plugins print on stderr: stdout is the runtime's manifest channel.
fn go_version(n: u32) -> String {
    format!(
        r#"// Package demo is a rutis plugin.
package demo

import (
	"context"
	"fmt"
	"os"

	"github.com/arcships/rutis/go/rutis"
)

type Config struct {{
	Greeting string `json:"greeting,omitempty" doc:"how to greet"`
}}

type Greeter struct {{
	greeting string
}}

func (g *Greeter) Hello(name string) string {{ return g.greeting + ", " + name + "! (v{n})" }}

var Plugin = rutis.Define(rutis.Plugin[Config]{{
	Name:     "demo",
	Inject:   []string{{}},
	Provides: rutis.Provides{{"greeter": rutis.MethodsOf[*Greeter](rutis.Sync("Hello"))}},
	Apply: func(ctx *rutis.Ctx, config Config) error {{
		greeting := config.Greeting
		if greeting == "" {{
			greeting = "Hello"
		}}
		executable, _ := os.Executable()
		fmt.Fprintf(os.Stderr, "demo v{n} applied %s\n", executable)
		ctx.Provide("greeter", &Greeter{{greeting: greeting}})
		ctx.Effect(func(context.Context) error {{
			fmt.Fprintln(os.Stderr, "demo v{n} cleanup")
			return nil
		}})
		return nil
	}},
}})
"#
    )
}

/// Make the template project's dependencies this checkout's packages.
fn prepare(s: &Scenario, kind: Kind) {
    match kind {
        Kind::Node => {
            s.link("demo/node_modules/@arcships/rutis", &repo().join("node/rutis"));
            s.link(
                "demo/node_modules/tsx",
                &repo().join("node/rutis-runtime/node_modules/tsx"),
            );
        }
        Kind::Bun => {
            s.link("demo/node_modules/@arcships/rutis", &repo().join("node/rutis"));
            s.link("demo/node_modules/@arcships/rutis-bun", &repo().join("bun/rutis-bun"));
        }
        Kind::Python => {
            let mut venv = s.program("demo", "python3", ["-m", "venv", "--without-pip", ".venv"], &[]);
            assert!(venv.wait_exit().success(), "python3 -m venv");
        }
        Kind::Go => {
            let sdk = repo().join("go/rutis");
            s.edit("demo/go.mod", |module| {
                format!("{module}\nreplace github.com/arcships/rutis/go/rutis => {}\n", sdk.display())
            });
        }
    }
}

/// Go builds use the module cache, never the network.
const GO_ENV: &[(&str, &str)] = &[("GOPROXY", "off"), ("GOTOOLCHAIN", "local"), ("GOFLAGS", "-mod=mod")];

/// The template's own tests.
fn template_tests(s: &Scenario, kind: Kind) -> Host {
    match kind {
        Kind::Node => s.program("demo", "npm", ["test"], &[]),
        Kind::Bun => s.program("demo", "bun", ["test"], &[]),
        Kind::Python => {
            let path = format!("src:{}", repo().join("python/rutis").display());
            s.program(
                "demo",
                ".venv/bin/python",
                ["-m", "unittest", "discover", "-s", "tests"],
                &[("PYTHONPATH", &path)],
            )
        }
        Kind::Go => s.program("demo", "go", ["test", "./..."], GO_ENV),
    }
}

fn greet(probe: &Probe, host: &mut Host) -> Value {
    probe
        .call(host, "greeter", "hello", json!(["Ada"]))
        .expect("greeter.hello")
}

/// After a good edit: the new version runs, and the probe has started
/// again (it is reloaded with every row; a Go project's restarts with the
/// Go runtime its service lives in).
fn edited(kind: Kind, host: &mut Host, probe: &Probe) {
    match kind {
        Kind::Go => {
            host.expect("go-demo: rebuilt and restarted");
        }
        _ => {
            host.expect("demo: reloaded");
            host.expect("probe: reloaded");
        }
    }
    probe.started(host);
}

/// After a broken edit: the error line, which names the source file.
fn failed(kind: Kind, host: &mut Host) -> String {
    let (what, prefix) = match kind {
        Kind::Go => ("the failed build", "demo: the build failed; the last build keeps running"),
        _ => ("the failed reload", "demo: cannot reload: "),
    };
    let line = host.wait_for(what, |line| line.starts_with(prefix));
    if kind != Kind::Go {
        // Every row reloads; the probe's does, and it starts again.
        host.expect("probe: reloaded");
        // `wait_for` takes lines in any order: the probe's start comes later.
    }
    line
}

fn dev_loop(kind: Kind) {
    let s = Scenario::new(&format!("dev-loop-{}", kind.lang()));
    let source = format!("demo/{}", kind.source());
    let file = std::path::Path::new(kind.source())
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    // 1. new
    let mut new = s.host(["new", "demo", "--lang", kind.lang()]);
    assert!(new.wait_exit().success(), "rutis-host new");
    new.expect("created demo/");
    assert!(s.path(&source).exists(), "{source} was not created");

    // 2. The template's own tests, on this checkout's packages. For Go
    // this also fills the build cache the first dev build uses.
    prepare(&s, kind);
    let mut tests = template_tests(&s, kind);
    assert!(tests.wait_exit().success(), "the template's tests failed");

    // 3. check, on the template project as generated.
    let mut check = s.host_in_with("demo", ["check"], GO_ENV);
    assert!(check.wait_exit().success(), "rutis-host check");
    check.expect("demo");
    check.expect("greeter");

    // 4. dev, with a probe that calls the greeter.
    let (lang, dir) = kind.probe();
    let probe = s.probe_in(&format!("demo/{dir}"), "probe", lang, &["greeter"]);
    let mut dev_json = json!({
        "rows": [{ "id": "demo", "config": { "greeting": "Hello" } }, probe.row()],
    });
    if !kind.runtimes().is_null() {
        dev_json["runtimes"] = kind.runtimes();
    }
    s.write_json("demo/rutis.dev.json", &dev_json);
    let mut dev = s.host_in_with("demo", ["dev"], GO_ENV);
    dev.expect("rutis-host dev: running demo; changes reload it (Ctrl-C ends)");
    probe.started(&mut dev);
    assert_eq!(greet(&probe, &mut dev), json!("Hello, Ada!"));

    // 5. An edit: v2 runs, after the probe started again.
    s.replace(&source, kind.version(2));
    edited(kind, &mut dev, &probe);
    let applied = dev.expect("demo v2 applied ");
    assert_eq!(greet(&probe, &mut dev), json!("Hello, Ada! (v2)"));
    if kind == Kind::Python {
        // A8: the project's own interpreter.
        assert!(applied.contains("/demo/.venv/"), "{applied}");
    }

    // 6. A broken edit: it does not load, v2 keeps serving and is not
    // unloaded.
    s.replace(&source, kind.broken());
    let error = failed(kind, &mut dev);
    // A7: the error names the file and line 5, on that line (Python) or in
    // the compiler's report on the following lines (`index.ts:5:…`,
    // `plugin.go:5:…`).
    let located =
        |line: &str| line.contains(&file) && (line.contains(":5:") || line.contains("line 5"));
    if !located(&error) {
        dev.wait_for(&format!("the error's location ({file}, line 5)"), located);
    }
    if kind != Kind::Go {
        probe.started(&mut dev);
    }
    assert_eq!(greet(&probe, &mut dev), json!("Hello, Ada! (v2)"));
    assert_eq!(dev.count("demo v2 cleanup"), 0, "v2 was unloaded by a failed edit");

    // 7. A fix: v3 runs; v2 was cleaned up exactly once.
    s.replace(&source, kind.version(3));
    edited(kind, &mut dev, &probe);
    dev.expect("demo v3 applied ");
    assert_eq!(greet(&probe, &mut dev), json!("Hello, Ada! (v3)"));
    if kind == Kind::Go {
        // Only the build that runs is kept.
        let builds: Vec<_> = std::fs::read_dir(s.path("demo/.rutis/go"))
            .expect(".rutis/go")
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(builds.len(), 1, "builds left: {builds:?}");
    }

    // 8. Ctrl-C: v3 is cleaned up once, the probe stops, exit code 0.
    dev.ctrl_c();
    let status = dev.wait_exit();
    dev.wait_output_end();
    assert_eq!(status.code(), Some(0), "dev after Ctrl-C: {status}");
    assert_eq!(dev.count("demo v2 cleanup"), 1, "v2 cleanup");
    assert_eq!(dev.count("demo v3 cleanup"), 1, "v3 cleanup");
    probe.event(&mut dev, "stopped");

    s.finish();
}

/// risk: A2, A3, A4, A7, B2 (Node)
#[test]
fn the_node_plugin_development_loop() {
    dev_loop(Kind::Node);
}

/// risk: A2, A3, A4, A7, A8, B2 (Python)
#[test]
fn the_python_plugin_development_loop() {
    dev_loop(Kind::Python);
}

/// risk: A2, A3, A4, A7, B2 (Bun)
#[test]
fn the_bun_plugin_development_loop() {
    dev_loop(Kind::Bun);
}

/// risk: A2, A3, A4, A7, B2 (Go: a rebuild and a runtime restart, not a reload)
#[test]
fn the_go_plugin_development_loop() {
    dev_loop(Kind::Go);
}
