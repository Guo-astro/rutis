//! S2, the plugin author's development loop (#186): `rutis-host new`, the
//! template's own tests, `check`, `dev`, an edit, a broken edit, a fix,
//! Ctrl-C, in each language `new` supports. Scenario A of quality-status
//! §4.2; the residue checks end it.
//!
//! v1 is the template as `new` writes it; v2 and v3 are written whole by
//! the scenario and print a line when applied and when cleaned up, which is
//! how "the old instance was unloaded, exactly once" is seen. The template
//! tests run against this checkout's packages (linked, never installed
//! from a registry: that is S9, #193). On Windows (#232) each host is in a
//! Job Object, which the residue checks read, and Ctrl-C is Ctrl-Break to
//! the host's console process group ([`Host::ctrl_c`]); Bun is not declared
//! for Windows (Bun design §10), so its test is ignored there; Swift runs
//! on macOS only (Swift SDK design §13).

use std::path::{Path, PathBuf};

use rutis_e2e::{build_guard, repo, Host, Lang, Probe, Scenario};
use serde_json::{json, Value};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Node,
    Python,
    Bun,
    Go,
    Swift,
}

impl Kind {
    fn lang(self) -> &'static str {
        match self {
            Kind::Node => "node",
            Kind::Python => "python",
            Kind::Bun => "bun",
            Kind::Go => "go",
            Kind::Swift => "swift",
        }
    }

    /// The plugin's source file, relative to the project.
    fn source(self) -> &'static str {
        match self {
            Kind::Node | Kind::Bun => "src/index.ts",
            Kind::Python => "src/demo/__init__.py",
            Kind::Go => "plugin.go",
            Kind::Swift => "Sources/DemoPlugin/Plugin.swift",
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
            Kind::Swift => swift_version(n),
        }
    }

    /// v2 with line 5 replaced by a line that does not compile.
    fn broken(self) -> String {
        let mut lines: Vec<String> = self.version(2).lines().map(str::to_owned).collect();
        lines[4] = match self {
            Kind::Node | Kind::Bun => "const const = 1",
            Kind::Python => "def broken(:",
            Kind::Go | Kind::Swift => "func broken( {",
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
            Kind::Bun | Kind::Go | Kind::Swift => (Lang::Python, "dev"),
        }
    }

    /// A compiled language: an edit rebuilds the binary and restarts its
    /// runtime instead of reloading the rows.
    fn compiled(self) -> bool {
        matches!(self, Kind::Go | Kind::Swift)
    }

    /// What `rutis.dev.json` adds for the probe.
    fn runtimes(self) -> Value {
        match self {
            Kind::Node | Kind::Python => Value::Null,
            Kind::Bun | Kind::Go | Kind::Swift => json!({ "py": { "project": "dev" } }),
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
	Name:   "demo",
	Inject: []string{{}},
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

/// The template's Plugin.swift: its lines go to stderr, as Go's.
fn swift_version(n: u32) -> String {
    format!(
        r#"// The demo rutis plugin.
import Foundation
import Rutis

@RutisConfig
public struct Config: Decodable {{
    /// how to greet
    public var greeting: String?
}}

@RutisService
public final class Greeter: @unchecked Sendable {{
    let greeting: String

    init(greeting: String) {{ self.greeting = greeting }}

    public func hello(_ name: String) -> String {{ "\(greeting), \(name)! (v{n})" }}
}}

func say(_ line: String) {{
    FileHandle.standardError.write(Data((line + "\n").utf8))
}}

public let plugin = Rutis.define(Plugin(
    name: "demo",
    inject: [],
    config: Config.self,
    apply: {{ ctx, config in
        say("demo v{n} applied \(CommandLine.arguments[0])")
        ctx.provide("greeter", Greeter(greeting: config.greeting ?? "Hello"))
        ctx.effect {{ say("demo v{n} cleanup") }}
    }}
))
"#
    )
}

/// The Python on PATH: `python3`, `python` on Windows.
fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

/// npm: on Windows its `npm.cmd`, which a program has to name.
fn npm() -> &'static str {
    if cfg!(windows) {
        "npm.cmd"
    } else {
        "npm"
    }
}

/// The interpreter of the virtual environment `venv`.
fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

/// `path` as go.mod takes it: on Windows without the `\\?\` a canonical
/// path has, with `/` (an absolute `C:/…` is a directory for go).
fn go_mod_path(path: &Path) -> String {
    let path = path.display().to_string();
    match cfg!(windows) {
        true => path
            .strip_prefix(r"\\?\")
            .unwrap_or(&path)
            .replace('\\', "/"),
        false => path,
    }
}

/// Make the template project's dependencies this checkout's packages.
fn prepare(s: &Scenario, kind: Kind) {
    match kind {
        Kind::Node => {
            s.link(
                "demo/node_modules/@arcships/rutis",
                &repo().join("node/rutis"),
            );
            s.link(
                "demo/node_modules/tsx",
                &repo().join("node/rutis-runtime/node_modules/tsx"),
            );
        }
        Kind::Bun => {
            s.link(
                "demo/node_modules/@arcships/rutis",
                &repo().join("node/rutis"),
            );
            s.link(
                "demo/node_modules/@arcships/rutis-bun",
                &repo().join("bun/rutis-bun"),
            );
        }
        Kind::Python => {
            let mut venv = s.program(
                "demo",
                python(),
                ["-m", "venv", "--without-pip", ".venv"],
                &[],
            );
            assert!(
                venv.wait_exit_within(build_guard()).success(),
                "{} -m venv",
                python()
            );
        }
        Kind::Go => {
            let sdk = repo().join("go/rutis");
            s.edit("demo/go.mod", |module| {
                format!(
                    "{module}\nreplace github.com/arcships/rutis/go/rutis => {}\n",
                    go_mod_path(&sdk)
                )
            });
        }
        Kind::Swift => {
            // A path dependency's package is named after its directory:
            // this checkout as `rutis`.
            s.link("rutis", &repo());
            s.edit("demo/Package.swift", |manifest| {
                let start = manifest
                    .find(".package(url: \"https://github.com/arcships/rutis\"")
                    .expect("the rutis dependency");
                let end = start + manifest[start..].find(')').unwrap() + 1;
                format!(
                    "{}.package(path: \"../rutis\"){}",
                    &manifest[..start],
                    &manifest[end..]
                )
            });
        }
    }
}

/// Go builds use the module cache, never the network.
const GO_ENV: &[(&str, &str)] = &[
    ("GOPROXY", "off"),
    ("GOTOOLCHAIN", "local"),
    ("GOFLAGS", "-mod=mod"),
];

/// The template's own tests.
fn template_tests(s: &Scenario, kind: Kind) -> Host {
    match kind {
        Kind::Node => s.program("demo", npm(), ["test"], &[]),
        Kind::Bun => s.program("demo", "bun", ["test"], &[]),
        Kind::Python => {
            let path = std::env::join_paths([PathBuf::from("src"), repo().join("python/rutis")])
                .expect("PYTHONPATH");
            let path = path.to_string_lossy();
            // By absolute path: a relative program is not looked up in the
            // child's directory on Windows.
            s.program(
                "demo",
                venv_python(&s.path("demo/.venv")),
                ["-m", "unittest", "discover", "-s", "tests"],
                &[("PYTHONPATH", &path)],
            )
        }
        Kind::Go => s.program("demo", "go", ["test", "./..."], GO_ENV),
        // The Command Line Tools (the self-hosted runners' toolchain) have
        // neither XCTest nor Swift Testing's macro plugin: there the test
        // target cannot build, and only the plugin and its binary are built.
        Kind::Swift if command_line_tools() => {
            s.program("demo", "swift", ["build", "--product", "demo"], &[])
        }
        Kind::Swift => s.program("demo", "swift", ["test"], &[]),
    }
}

/// Whether the active developer directory is the Command Line Tools, not
/// Xcode.
fn command_line_tools() -> bool {
    std::process::Command::new("xcode-select")
        .arg("-p")
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains("CommandLineTools"))
        .unwrap_or(false)
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
        Kind::Swift => {
            host.expect("swift-demo: rebuilt and restarted");
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
        Kind::Go | Kind::Swift => (
            "the failed build",
            "demo: the build failed; the last build keeps running",
        ),
        _ => ("the failed reload", "demo: cannot reload: "),
    };
    let line = host.wait_for(what, |line| line.starts_with(prefix));
    if !kind.compiled() {
        // Every row reloads; the probe's does, and it starts again.
        host.expect("probe: reloaded");
        // `wait_for` takes lines in any order: the probe's start comes later.
    }
    line
}

fn dev_loop(kind: Kind) {
    let s = Scenario::new(&format!("dev-loop-{}", kind.lang()));
    let source = format!("demo/{}", kind.source());
    let file = Path::new(kind.source())
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
    assert!(
        tests.wait_exit_within(build_guard()).success(),
        "the template's tests failed"
    );

    // 3. check, on the template project as generated.
    // A compiled language's check builds the binary first: a build's time.
    let mut check = s.host_in_with("demo", ["check"], GO_ENV);
    let checked = match kind.compiled() {
        true => check.wait_exit_within(build_guard()),
        false => check.wait_exit(),
    };
    assert!(checked.success(), "rutis-host check");
    // The template declares no `provides`: its greeter is known once it
    // runs.
    check.expect("demo");

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
        // A8: the project's own interpreter (`Scripts\python.exe` on
        // Windows).
        let venv = venv_python(&Path::new("demo").join(".venv"));
        let venv = venv.parent().unwrap().display().to_string();
        assert!(
            applied.contains(&format!("{}{venv}", std::path::MAIN_SEPARATOR)),
            "{applied}"
        );
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
    if !kind.compiled() {
        probe.started(&mut dev);
    }
    assert_eq!(greet(&probe, &mut dev), json!("Hello, Ada! (v2)"));
    assert_eq!(
        dev.count("demo v2 cleanup"),
        0,
        "v2 was unloaded by a failed edit"
    );

    // 7. A fix: v3 runs; v2 was cleaned up exactly once.
    s.replace(&source, kind.version(3));
    edited(kind, &mut dev, &probe);
    dev.expect("demo v3 applied ");
    assert_eq!(greet(&probe, &mut dev), json!("Hello, Ada! (v3)"));
    if kind.compiled() {
        // Only the build that runs is kept.
        let builds = format!("demo/.rutis/{}", kind.lang());
        let builds: Vec<_> = std::fs::read_dir(s.path(&builds))
            .expect(&builds)
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(builds.len(), 1, "builds left: {builds:?}");
        // Only the plugin's runtime restarted: the probe's Python runtime
        // ran on, never down.
        assert_eq!(dev.count("runtime py"), 0, "the Python runtime restarted");
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
/// guarantee: host/stop-on-signal, host/dev-reload, host/dev-failed-reload-keeps-old, host/new, host/check
#[test]
fn the_node_plugin_development_loop() {
    dev_loop(Kind::Node);
}

/// risk: A2, A3, A4, A7, A8, B2 (Python)
/// guarantee: host/stop-on-signal, host/dev-reload, host/dev-failed-reload-keeps-old, host/new, host/check
#[test]
fn the_python_plugin_development_loop() {
    dev_loop(Kind::Python);
}

/// risk: A2, A3, A4, A7, B2 (Bun)
/// guarantee: host/stop-on-signal, host/dev-reload, host/dev-failed-reload-keeps-old, host/new, host/check
#[test]
#[cfg_attr(windows, ignore = "Bun is not declared for Windows (Bun design §10)")]
fn the_bun_plugin_development_loop() {
    dev_loop(Kind::Bun);
}

/// risk: A2, A3, A4, A7, B2 (Go: a rebuild and a runtime restart, not a reload)
/// guarantee: host/stop-on-signal, host/dev-reload, host/dev-failed-reload-keeps-old, host/new, host/check
#[test]
fn the_go_plugin_development_loop() {
    dev_loop(Kind::Go);
}

/// risk: A2, A3, A4, A7, B2 (Swift: a rebuild and a restart of its runtime
/// only, as Go's)
/// guarantee: host/stop-on-signal, host/dev-reload, host/dev-failed-reload-keeps-old, host/new, host/check
#[test]
#[cfg_attr(
    not(target_os = "macos"),
    ignore = "Swift plugin binaries are macOS only (Swift SDK design §13)"
)]
fn the_swift_plugin_development_loop() {
    dev_loop(Kind::Swift);
}
