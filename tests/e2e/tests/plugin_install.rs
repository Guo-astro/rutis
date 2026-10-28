//! Plugin install (#691, design docs/design-plugin-install-2026-10-27.md
//! §十三 P3): in each of Node, Bun, Python and Go, a package made from the
//! `rutis-host new` template → `add --dry-run` → `add` → `check` → `run`,
//! a probe calling its service → a new version and `upgrade` → `run` again
//! → `remove`. Python starts with no environment, from `pypi:` with a local
//! `--find-links`; Go from its module path through a `GOPROXY=file://`
//! module proxy. And `dev`: `add` writes `rutis.dev.json`, the new row is
//! `added` and the project's own row does not restart.
//!
//! The packages depend on this checkout's SDKs (by path, a local module
//! proxy), never on a registry: nothing is downloaded. Bun is not declared
//! for Windows (Bun design §10); the scenarios run on Unix.

#![cfg(unix)]

use std::path::Path;

use rutis_e2e::{build_guard, repo, Host, Lang, Probe, Scenario};
use serde_json::{json, Value};

/// npm without the network or its notices.
const NPM_ENV: &[(&str, &str)] = &[
    ("npm_config_offline", "true"),
    ("npm_config_audit", "false"),
    ("npm_config_fund", "false"),
    ("npm_config_update_notifier", "false"),
];

/// `rutis-host <args>` in the project's `dir`, to its end: whether it
/// succeeded, and everything it wrote.
fn rutis_host(s: &Scenario, dir: &str, args: &[&str], env: &[(&str, &str)]) -> (bool, String) {
    let mut host = s.host_in_with(dir, args, env);
    let ended = host.wait_exit_within(build_guard());
    let said = host
        .lines()
        .iter()
        .map(|line| line.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    (ended.success(), said)
}

/// [`rutis_host`], which has to succeed and say each of `lines`.
fn says(s: &Scenario, args: &[&str], env: &[(&str, &str)], lines: &[&str]) -> String {
    let (ok, said) = rutis_host(s, "", args, env);
    assert!(ok, "rutis-host {args:?} failed:\n{said}");
    for line in lines {
        assert!(
            said.lines().any(|said| said == *line),
            "rutis-host {args:?} did not say {line:?}:\n{said}"
        );
    }
    said
}

/// A program run to its end in the project's `dir`, which has to succeed.
fn program(s: &Scenario, dir: &str, program: &str, args: &[&str], env: &[(&str, &str)]) {
    let mut run = s.program(dir, program, args, env);
    assert!(
        run.wait_exit_within(build_guard()).success(),
        "{program} {args:?}"
    );
}

/// `rutis-host new demo --lang <lang>` in the project.
fn new(s: &Scenario, name: &str, lang: &str) {
    let (ok, said) = rutis_host(s, "", &["new", name, "--lang", lang], &[]);
    assert!(ok, "rutis-host new:\n{said}");
}

/// The rows the scenarios run: the probe, which calls the greeter, and
/// the Python runtime it runs in.
fn configuration(s: &Scenario) -> Probe {
    let probe = s.probe("probe", Lang::Python, &["greeter"]);
    s.write_json(
        "rutis.json",
        &json!({ "format": 1, "runtimes": { "py": {} }, "rows": [probe.row()] }),
    );
    probe
}

/// Run the configuration: the probe's call to the greeter answers with
/// version `n`'s words; then Ctrl-C.
fn runs(s: &Scenario, probe: &Probe, env: &[(&str, &str)], n: u32) {
    let mut host: Host = s.host_with(["run"], env);
    probe.started(&mut host);
    let hello = probe
        .call(&mut host, "greeter", "hello", json!(["Ada"]))
        .expect("greeter.hello");
    assert_eq!(hello, json!(format!("Hello, Ada! (v{n})")));
    host.ctrl_c();
    assert!(host.wait_exit().success(), "rutis-host run ends with 0");
}

/// The rows of `rutis.json` and its records.
fn written(s: &Scenario) -> Value {
    serde_json::from_str(&std::fs::read_to_string(s.path("rutis.json")).unwrap()).unwrap()
}

/// The template's plugin, version `n`, in JavaScript (Node's `dist/`, and
/// Bun's source, which it runs as it is).
fn js_plugin(n: u32) -> String {
    format!(
        r#"import {{ definePlugin }} from '@arcships/rutis'

class Greeter {{
  constructor(greeting) {{ this.greeting = greeting }}
  hello(name) {{ return `${{this.greeting}}, ${{name}}! (v{n})` }}
}}

export default definePlugin({{
  inject: [],
  config: {{ type: 'object', properties: {{ greeting: {{ type: 'string', default: 'Hello' }} }} }},
  apply(ctx, config) {{
    ctx.provide('greeter', new Greeter(config.greeting ?? 'Hello'))
  }},
}})
"#
    )
}

/// This checkout's TypeScript SDK packed (`npm pack`): what the plugins
/// depend on, by path (Bun reads a directory a packed package names by
/// path relative to the package; a packed file it finds).
fn node_sdk(s: &Scenario) -> std::path::PathBuf {
    let out = s.dir().join("sdk");
    if !out.exists() {
        std::fs::create_dir_all(&out).unwrap();
        let mut pack = s.program(
            "",
            "npm",
            [
                "pack",
                "--ignore-scripts",
                "--pack-destination",
                &out.display().to_string(),
                &repo().join("node/rutis").display().to_string(),
            ],
            NPM_ENV,
        );
        assert!(
            pack.wait_exit_within(build_guard()).success(),
            "npm pack node/rutis"
        );
    }
    std::fs::read_dir(&out)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|extension| extension == "tgz"))
        .expect("the packed SDK")
}

/// The template's npm package at version `n` (`0.<n>.0`), packed into the
/// project as `demo.tgz`: its SDK this checkout's, by path.
fn pack_js(s: &Scenario, file: &str, n: u32) {
    let sdk = node_sdk(s);
    s.edit("demo/package.json", |text| {
        let mut manifest: Value = serde_json::from_str(&text).unwrap();
        manifest["version"] = json!(format!("0.{n}.0"));
        manifest["dependencies"] = json!({ "@arcships/rutis": format!("file:{}", sdk.display()) });
        serde_json::to_string_pretty(&manifest).unwrap()
    });
    s.write(format!("demo/{file}"), js_plugin(n));
    // The template builds with tsc before packing; the file is written.
    program(
        s,
        "demo",
        "npm",
        &["pack", "--ignore-scripts", "--pack-destination", ".."],
        NPM_ENV,
    );
    std::fs::rename(s.path(format!("demo-0.{n}.0.tgz")), s.path("demo.tgz")).unwrap();
}

/// risk: B1
/// guarantee: host/plugin-install
#[test]
fn a_node_plugin_installs_runs_upgrades_and_goes() {
    let s = Scenario::new("install-node");
    new(&s, "demo", "node");
    pack_js(&s, "dist/index.js", 1);
    let probe = configuration(&s);

    says(
        &s,
        &["add", "--dry-run", "./demo.tgz"],
        NPM_ENV,
        &[
            "./demo.tgz 0.1.0 (node): plugin demo",
            "demo",
            "  version: \"0.1.0\"",
            "nothing was installed (--dry-run)",
        ],
    );
    assert!(
        !s.path("node_modules/demo").exists(),
        "--dry-run installs nothing here"
    );

    let said = says(
        &s,
        &["add", "./demo.tgz"],
        NPM_ENV,
        &[
            "demo (demo): ok",
            "  version: \"0.1.0\"",
            "rutis.json: added row demo, runtimes.node",
        ],
    );
    assert!(
        said.contains("installed ./demo.tgz 0.1.0 (node) into "),
        "{said}"
    );
    let file = written(&s);
    assert_eq!(file["installed"]["demo"], json!({ "from": "./demo.tgz" }));
    assert_eq!(file["rows"][1], json!({ "id": "demo", "name": "demo" }));
    says(&s, &["check"], &[], &["demo (demo): ok"]);
    runs(&s, &probe, &[], 1);

    pack_js(&s, "dist/index.js", 2);
    says(
        &s,
        &["upgrade"],
        NPM_ENV,
        &[
            "./demo.tgz: 0.1.0 → 0.2.0",
            "a running host keeps the old versions until it restarts",
        ],
    );
    runs(&s, &probe, &[], 2);

    let said = says(
        &s,
        &["remove", "demo"],
        NPM_ENV,
        &["rutis.json: removed row demo"],
    );
    assert!(said.contains("uninstalled demo from "), "{said}");
    assert!(!s.path("node_modules/demo").exists());
    assert_eq!(written(&s)["installed"], json!({}));
    s.finish();
}

/// A local npm registry with one package, this checkout's Bun runtime
/// (`@arcships/rutis-bun`): `add --dry-run` installs it into a scratch
/// project, and the version of this checkout may not be published yet.
/// Its address, for `BUN_CONFIG_REGISTRY`; it serves until the test ends.
fn bun_registry(s: &Scenario) -> String {
    use std::io::{BufRead, BufReader, Write};

    let out = s.dir().join("bun-registry");
    std::fs::create_dir_all(&out).unwrap();
    program(
        s,
        "",
        "npm",
        &[
            "pack",
            "--ignore-scripts",
            "--pack-destination",
            &out.display().to_string(),
            &repo().join("bun/rutis-bun").display().to_string(),
        ],
        NPM_ENV,
    );
    let tarball = std::fs::read_dir(&out)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|extension| extension == "tgz"))
        .expect("the packed Bun runtime");
    let tarball = std::fs::read(tarball).unwrap();
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(repo().join("bun/rutis-bun/package.json")).unwrap())
            .unwrap();
    let version = manifest["version"].as_str().unwrap().to_owned();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}/", listener.local_addr().unwrap());
    let package = json!({
        "name": "@arcships/rutis-bun",
        "dist-tags": { "latest": version },
        "versions": { &version: {
            "name": "@arcships/rutis-bun",
            "version": version,
            "dist": { "tarball": format!("{address}rutis-bun.tgz") },
        } },
    })
    .to_string()
    .into_bytes();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(&stream);
            let mut request = String::new();
            if reader.read_line(&mut request).is_err() {
                continue;
            }
            // The headers, to the empty line.
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let path = request.split(' ').nth(1).unwrap_or_default();
            let (status, body) = if path.ends_with("/rutis-bun.tgz") {
                ("200 OK", &tarball)
            } else if path.contains("rutis-bun") {
                ("200 OK", &package)
            } else {
                ("404 Not Found", &Vec::new())
            };
            let mut stream = &stream;
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(body);
        }
    });
    address
}

/// risk: B1
/// guarantee: host/plugin-install
#[test]
fn a_bun_plugin_installs_runs_upgrades_and_goes() {
    let s = Scenario::new("install-bun");
    new(&s, "demo", "bun");
    pack_js(&s, "src/index.ts", 1);
    let probe = configuration(&s);
    // The Bun runtime of this checkout, as the host's project has it.
    s.write_json(
        "package.json",
        &json!({
            "name": "app",
            "private": true,
            "devDependencies": {
                "@arcships/rutis-bun": format!("file:{}", repo().join("bun/rutis-bun").display())
            }
        }),
    );
    // Bun leaves its own temporary files behind (`.<hash>-1.package`):
    // in a directory of their own, not the scenario's TMPDIR, which the
    // residue checks read.
    let bun_tmp = s.dir().join("bun-tmp");
    std::fs::create_dir_all(&bun_tmp).unwrap();
    let bun_tmp = bun_tmp.display().to_string();
    let registry = bun_registry(&s);
    let env: &[(&str, &str)] = &[("TMPDIR", &bun_tmp), ("BUN_CONFIG_REGISTRY", &registry)];
    program(&s, "", "bun", &["install"], env);

    says(
        &s,
        &["add", "--dry-run", "./demo.tgz"],
        env,
        &[
            "./demo.tgz 0.1.0 (bun): plugin demo",
            "nothing was installed (--dry-run)",
        ],
    );
    let said = says(
        &s,
        &["add", "./demo.tgz"],
        env,
        &[
            "demo (bun:demo): ok",
            "rutis.json: added row demo, runtimes.bun",
        ],
    );
    assert!(
        said.contains("installed ./demo.tgz 0.1.0 (bun) into "),
        "{said}"
    );
    let file = written(&s);
    assert_eq!(
        file["installed"]["bun:demo"],
        json!({ "from": "./demo.tgz" })
    );
    assert_eq!(file["rows"][1], json!({ "id": "demo", "name": "bun:demo" }));
    says(&s, &["check"], &[], &["demo (bun:demo): ok"]);
    runs(&s, &probe, &[], 1);

    pack_js(&s, "src/index.ts", 2);
    says(
        &s,
        &["upgrade", "demo"],
        env,
        &["./demo.tgz: 0.1.0 → 0.2.0"],
    );
    runs(&s, &probe, &[], 2);

    says(
        &s,
        &["remove", "./demo.tgz"],
        env,
        &["rutis.json: removed row demo"],
    );
    assert_eq!(written(&s)["installed"], json!({}));
    s.finish();
}

/// The template's Python plugin, version `n`.
fn python_plugin(n: u32) -> String {
    format!(
        r#"from rutis import define_plugin


class Greeter:
    def __init__(self, greeting):
        self.greeting = greeting

    def hello(self, name):
        return f"{{self.greeting}}, {{name}}! (v{n})"


def apply(ctx, config):
    ctx.provide("greeter", Greeter(config.get("greeting", "Hello")))


plugin = define_plugin(
    apply,
    inject=[],
    config={{"type": "object", "properties": {{"greeting": {{"type": "string", "default": "Hello"}}}}}},
)
"#
    )
}

/// Builds a wheel of the module directory `src` as the distribution `name`
/// at `version`, its entry point `name` in the group `rutis.plugins` (the
/// template's), into `out`: what `uv build` makes of the template, without
/// a build backend to download.
const WHEEL: &str = r#"
import base64, hashlib, os, sys, zipfile
src, name, version, out = sys.argv[1:5]
info = f"{name}-{version}.dist-info"
files = {}
for root, _, names in os.walk(src):
    for each in names:
        if each.endswith(".pyc"):
            continue
        path = os.path.join(root, each)
        files[(name + "/" + os.path.relpath(path, src)).replace(os.sep, "/")] = open(path, "rb").read()
files[f"{info}/METADATA"] = f"Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n".encode()
files[f"{info}/WHEEL"] = b"Wheel-Version: 1.0\nGenerator: rutis-e2e\nRoot-Is-Purelib: true\nTag: py3-none-any\n"
files[f"{info}/entry_points.txt"] = f"[rutis.plugins]\n{name} = {name}\n".encode()
record = []
for path, data in files.items():
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
    record.append(f"{path},sha256={digest},{len(data)}")
record.append(f"{info}/RECORD,,")
files[f"{info}/RECORD"] = ("\n".join(record) + "\n").encode()
os.makedirs(out, exist_ok=True)
with zipfile.ZipFile(os.path.join(out, f"{name}-{version}-py3-none-any.whl"), "w") as wheel:
    for path, data in files.items():
        wheel.writestr(path, data)
"#;

/// The template's Python package `name` at version `n`, as a wheel in the
/// project's `wheels/`.
fn wheel(s: &Scenario, name: &str, source: &str) {
    s.write(format!("{name}/src/{name}/__init__.py"), source);
    let file = s.path(format!("{name}/version"));
    let version = if file.exists() {
        std::fs::read_to_string(file).unwrap()
    } else {
        "0.1.0".into()
    };
    let src = s.path(format!("{name}/src/{name}"));
    let out = s.path("wheels");
    program(
        s,
        "",
        "python3",
        &[
            "-I",
            "-c",
            WHEEL,
            &src.display().to_string(),
            name,
            &version,
            &out.display().to_string(),
        ],
        &[],
    );
}

/// PyPI is the project's `wheels/`, for uv and for pip.
fn find_links(s: &Scenario) -> Vec<(&'static str, String)> {
    let wheels = s.path("wheels").display().to_string();
    // An index with nothing in it (uv before 0.7 has no UV_NO_INDEX), and
    // a cache of the scenario's own.
    let index = s.dir().join("index");
    std::fs::create_dir_all(&index).unwrap();
    let index = format!("file://{}", index.display());
    vec![
        ("UV_FIND_LINKS", wheels.clone()),
        ("UV_INDEX_URL", index.clone()),
        ("UV_DEFAULT_INDEX", index),
        ("UV_CACHE_DIR", s.dir().join(".uv").display().to_string()),
        ("PIP_FIND_LINKS", wheels),
        ("PIP_NO_INDEX", "1".into()),
        ("PIP_DISABLE_PIP_VERSION_CHECK", "1".into()),
    ]
}

fn borrowed<'a>(env: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    env.iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect()
}

/// From no environment: `add` creates `.venv` and installs from `pypi:`
/// (a local `--find-links`); a local wheel for `--dry-run`.
/// risk: B1
/// guarantee: host/plugin-install
#[test]
fn a_python_plugin_installs_runs_upgrades_and_goes() {
    let s = Scenario::new("install-python");
    new(&s, "demo", "python");
    wheel(&s, "demo", &python_plugin(1));
    let probe = configuration(&s);
    let links = find_links(&s);
    let env = borrowed(&links);
    assert!(!s.path(".venv").exists());

    says(
        &s,
        &["add", "--dry-run", "./wheels/demo-0.1.0-py3-none-any.whl"],
        &env,
        &[
            "./wheels/demo-0.1.0-py3-none-any.whl 0.1.0 (py): plugin demo",
            "  version: \"0.1.0\"",
            "nothing was installed (--dry-run)",
        ],
    );
    assert!(
        !s.path(".venv").exists(),
        "--dry-run makes no environment here"
    );

    let said = says(
        &s,
        &["add", "pypi:demo"],
        &env,
        &[
            "demo (py:demo): ok",
            "  version: \"0.1.0\"",
            "rutis.json: added row demo",
        ],
    );
    assert!(said.contains("installed demo 0.1.0 (py) into "), "{said}");
    assert!(s.path(".venv").exists(), "add made the environment");
    assert_eq!(
        written(&s)["installed"]["py:demo"],
        json!({ "from": "pypi:demo" })
    );
    says(&s, &["check"], &[], &["demo (py:demo): ok"]);
    runs(&s, &probe, &[], 1);

    s.write("demo/version", "0.2.0");
    wheel(&s, "demo", &python_plugin(2));
    says(&s, &["upgrade"], &env, &["demo: 0.1.0 → 0.2.0"]);
    runs(&s, &probe, &[], 2);

    says(
        &s,
        &["remove", "demo"],
        &env,
        &["rutis.json: removed row demo"],
    );
    assert_eq!(written(&s)["installed"], json!({}));
    s.finish();
}

/// The template's Go plugin `netkit`, version `n`.
fn go_plugin(n: u32) -> String {
    format!(
        r#"// Package netkit is a rutis plugin.
package netkit

import "github.com/arcships/rutis/go/rutis"

type Config struct {{
	Greeting string `json:"greeting,omitempty" doc:"how to greet"`
}}

type Greeter struct {{
	greeting string
}}

func (g *Greeter) Hello(name string) string {{ return g.greeting + ", " + name + "! (v{n})" }}

var Plugin = rutis.Define(rutis.Plugin[Config]{{
	Name:   "netkit",
	Inject: []string{{}},
	Apply: func(ctx *rutis.Ctx, config Config) error {{
		greeting := config.Greeting
		if greeting == "" {{
			greeting = "Hello"
		}}
		ctx.Provide("greeter", &Greeter{{greeting: greeting}})
		return nil
	}},
}})
"#
    )
}

/// Adds the module in `src` at `version` to the module proxy `proxy`
/// (`<module>/@v/`: list, .info, .mod, .zip), as `go mod download` finds
/// it there.
const MODULE: &str = r#"
import os, sys, zipfile
src, module, version, proxy = sys.argv[1:5]
at = os.path.join(proxy, module, "@v")
os.makedirs(at, exist_ok=True)
with zipfile.ZipFile(os.path.join(at, version + ".zip"), "w") as archive:
    for root, dirs, names in os.walk(src):
        dirs[:] = sorted(d for d in dirs if not d.startswith(".") and not os.path.exists(os.path.join(root, d, "go.mod")))
        for each in sorted(names):
            if each.startswith("."):
                continue
            path = os.path.join(root, each)
            archive.write(path, f"{module}@{version}/" + os.path.relpath(path, src).replace(os.sep, "/"))
open(os.path.join(at, version + ".mod"), "w").write(open(os.path.join(src, "go.mod")).read())
open(os.path.join(at, version + ".info"), "w").write('{"Version":"%s","Time":"2026-10-28T00:00:00Z"}' % version)
listed = os.path.join(at, "list")
versions = set(open(listed).read().split()) if os.path.exists(listed) else set()
open(listed, "w").write("\n".join(sorted(versions | {version})) + "\n")
"#;

fn module(s: &Scenario, src: &Path, module: &str, version: &str) {
    program(
        s,
        "",
        "python3",
        &[
            "-I",
            "-c",
            MODULE,
            &src.display().to_string(),
            module,
            version,
            &s.dir().join("proxy").display().to_string(),
        ],
        &[],
    );
}

/// Go with the module proxy of the scenario only, and its own module
/// cache.
fn go_env(s: &Scenario) -> Vec<(&'static str, String)> {
    vec![
        (
            "GOPROXY",
            format!("file://{}", s.dir().join("proxy").display()),
        ),
        ("GOSUMDB", "off".into()),
        ("GOFLAGS", "".into()),
        ("GOTOOLCHAIN", "local".into()),
        (
            "GOMODCACHE",
            s.dir().join("gomodcache").display().to_string(),
        ),
    ]
}

/// The template's Go module at `v0.<n>.0` in the module proxy.
fn publish_go(s: &Scenario, n: u32, env: &[(&str, &str)]) {
    s.write("netkit/plugin.go", go_plugin(n));
    program(s, "netkit", "go", &["mod", "tidy"], env);
    module(
        s,
        &s.path("netkit"),
        "example.com/netkit",
        &format!("v0.{n}.0"),
    );
}

/// From the Go module path, through a `GOPROXY=file://` module proxy that
/// also has this checkout's Go SDK; the module's directory for
/// `--dry-run`.
/// risk: B1
/// guarantee: host/plugin-install
#[test]
fn a_go_plugin_installs_runs_upgrades_and_goes() {
    let s = Scenario::new("install-go");
    new(&s, "netkit", "go");
    let vars = go_env(&s);
    let env = borrowed(&vars);
    // The SDK the template requires, served from this checkout.
    let required = std::fs::read_to_string(s.path("netkit/go.mod")).unwrap();
    let sdk = required
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("require github.com/arcships/rutis/go/rutis ")
        })
        .expect("the template requires the Go SDK")
        .trim()
        .to_owned();
    module(
        &s,
        &repo().join("go/rutis"),
        "github.com/arcships/rutis/go/rutis",
        &sdk,
    );
    publish_go(&s, 1, &env);
    let probe = configuration(&s);

    says(
        &s,
        &["add", "--dry-run", "./netkit"],
        &env,
        &["netkit", "nothing was installed (--dry-run)"],
    );
    assert!(
        !s.path("plugins/go").exists(),
        "--dry-run installs nothing here"
    );

    let said = says(
        &s,
        &["add", "example.com/netkit/cmd/netkit@v0.1.0"],
        &env,
        &[
            "netkit (go:netkit): ok",
            "rutis.json: added row netkit, runtimes.go",
        ],
    );
    assert!(
        said.contains("installed example.com/netkit/cmd/netkit 0.1.0 (go) into ")
            || said.contains("installed example.com/netkit/cmd/netkit v0.1.0 (go) into "),
        "{said}"
    );
    let file = written(&s);
    assert_eq!(
        file["installed"]["go:netkit"],
        json!({ "from": "example.com/netkit/cmd/netkit" })
    );
    assert_eq!(file["runtimes"]["go"], json!({ "dir": "plugins/go" }));
    says(&s, &["check"], &[], &["netkit (go:netkit): ok"]);
    runs(&s, &probe, &[], 1);

    publish_go(&s, 2, &env);
    let said = says(
        &s,
        &["upgrade"],
        &env,
        &["a running host keeps the old versions until it restarts"],
    );
    assert!(
        said.contains("example.com/netkit/cmd/netkit: ") && said.contains(" → "),
        "{said}"
    );
    runs(&s, &probe, &[], 2);

    let said = says(
        &s,
        &["remove", "netkit"],
        &env,
        &["rutis.json: removed row netkit"],
    );
    assert!(said.contains("uninstalled netkit from "), "{said}");
    assert!(!s.path("plugins/go/netkit").exists());
    // The module cache is read-only: removed by go.
    program(&s, "", "go", &["clean", "-modcache"], &env);
    s.finish();
}

/// `add` in a plugin project writes `rutis.dev.json`; `dev` reads it again:
/// the new row is added and runs, the project's own row does not restart.
/// risk: B1
/// guarantee: host/plugin-install
#[test]
fn add_in_a_dev_project_adds_its_row() {
    let s = Scenario::new("install-dev");
    new(&s, "demo", "python");
    s.write(
        "demo/src/demo/__init__.py",
        python_plugin(1).replace(
            "def apply(ctx, config):\n",
            "def apply(ctx, config):\n    print(\"demo applied\", flush=True)\n",
        ),
    );
    // The other plugin, a wheel the project installs.
    new(&s, "other", "python");
    wheel(
        &s,
        "other",
        "def apply(ctx, config):\n    print(\"other applied\", flush=True)\n",
    );
    let mut venv = s.program("demo", "python3", ["-m", "venv", ".venv"], &[]);
    assert!(
        venv.wait_exit_within(build_guard()).success(),
        "python3 -m venv"
    );
    let links = find_links(&s);
    let env = borrowed(&links);

    let mut dev = s.host_in_with("demo", ["dev"], &env);
    dev.expect("demo: running");
    let (ok, said) = rutis_host(&s, "demo", &["add", "pypi:other"], &env);
    assert!(ok, "rutis-host add:\n{said}");
    assert!(said.contains("rutis.dev.json: added row other"), "{said}");
    dev.expect("other: added");
    dev.expect("other: running");
    assert_eq!(
        dev.count("demo applied"),
        1,
        "the project's own row did not restart"
    );
    dev.ctrl_c();
    assert!(dev.wait_exit().success(), "rutis-host dev ends with 0");
    s.finish();
}
