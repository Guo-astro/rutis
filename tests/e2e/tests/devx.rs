//! The developer's view, as a release gate (#690, part of #663): what a
//! plugin author and someone assembling plugins do, with the packages a
//! user installs (the ones the release gate built, `RUTIS_E2E_ARTIFACTS`),
//! step by step as the guides say. A step that fails stops the release.
//!
//! - Writing a plugin, once per language (TypeScript, Python, Go, Bun,
//!   Rust): a project from the template (`rutis-host new`; Rust has none:
//!   `cargo new`), a `weather` plugin (`weather.today(city)`, which asks a
//!   fake `llm.ask(question)`; config `{ city }` with a default; each call
//!   emits `weather/asked`), its own tests with the language's testing
//!   tool, `rutis-host check`, `rutis-host dev` with one edit reloading it,
//!   and packing it (not publishing). Rust plugins are compiled into a Rust
//!   host (`rutis-host` does not run them): no `dev` step.
//! - Assembling plugins: `rutis-host` with a TypeScript plugin using a
//!   Python plugin's service: `check`, `run`, a call end to end, a change
//!   of configuration while it runs, a row disabled, a stop.
//! - A host owner: a small Rust host (`tests/e2e/fixture-toolhost`) opens
//!   an extension point, `tools.register(name, description, run)`; a
//!   plugin from the template registers a tool; the host lists it and
//!   calls it, and when the tool fails the host gets the plugin's error
//!   structured, with its code.
//!
//! The JavaScript and Python packages are this build's (npm tarballs,
//! wheels); the Go SDK and the Rust crates are this checkout's (an
//! unreleased version is on no proxy or registry), as install.rs does.
//! install.yml runs them on Linux, in the release gate and every night;
//! `cargo test` skips them:
//!
//!   tools/install/local-artifacts.sh   # once: this checkout's packages
//!   RUTIS_E2E_ARTIFACTS=<dir> cargo test -p rutis-e2e --test devx -- --ignored

use std::path::{Path, PathBuf};

use rutis_e2e::{build_guard, repo, Host, Lang, Scenario};
use serde_json::{json, Value};

// ── The packages ────────────────────────────────────────────────

/// Where the built packages are (npm/, wheels/, binary/).
fn artifacts() -> PathBuf {
    PathBuf::from(std::env::var_os("RUTIS_E2E_ARTIFACTS").expect(
        "RUTIS_E2E_ARTIFACTS: the directory package.yml or tools/install/local-artifacts.sh made",
    ))
}

/// The version under test: `RUTIS_E2E_VERSION`, else this checkout's.
fn version() -> String {
    std::env::var("RUTIS_E2E_VERSION").unwrap_or_else(|_| {
        let manifest: Value =
            serde_json::from_str(include_str!("../../../node/rutis-host/package.json")).unwrap();
        manifest["version"].as_str().unwrap().to_owned()
    })
}

/// The npm tarball of `@arcships/<package>` at this version.
fn npm_tarball(package: &str) -> String {
    let file = artifacts()
        .join("npm")
        .join(format!("arcships-{package}-{}.tgz", version()));
    assert!(
        file.exists(),
        "{} is not among the artifacts",
        file.display()
    );
    file.display().to_string()
}

/// The wheels whose name starts with `prefix` (`rutis-`, `rutis_host-`).
fn wheel(prefix: &str) -> String {
    let dir = artifacts().join("wheels");
    let found: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().unwrap().to_string_lossy();
            name.starts_with(prefix) && name.ends_with(".whl")
        })
        .collect();
    assert_eq!(
        found.len(),
        1,
        "{prefix}*.whl in {}: {found:?}",
        dir.display()
    );
    found[0].display().to_string()
}

/// The npm platform of this machine (`linux-x64`, `darwin-arm64`, ...).
fn npm_platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let cpu = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os}-{cpu}")
}

/// The `@arcships/*` packages a JavaScript project installs from the
/// artifacts, by name, as `file:` dependencies.
fn npm_files(packages: &[&str]) -> serde_json::Map<String, Value> {
    packages
        .iter()
        .map(|package| {
            (
                format!("@arcships/{package}"),
                json!(format!("file:{}", npm_tarball(package))),
            )
        })
        .collect()
}

fn npm() -> &'static str {
    if cfg!(windows) {
        "npm.cmd"
    } else {
        "npm"
    }
}

/// A program of a venv.
fn venv_program(venv: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join(format!("{name}.exe"))
    } else {
        venv.join("bin").join(name)
    }
}

/// Run a program to its end; it must succeed.
fn run(
    s: &Scenario,
    dir: &str,
    program: impl AsRef<std::ffi::OsStr>,
    args: &[&str],
    env: &[(&str, &str)],
) -> Host {
    let what = format!("{} {}", program.as_ref().to_string_lossy(), args.join(" "));
    let mut command = s.program(dir, program, args, env);
    let status = command.wait_exit_within(build_guard());
    if !status.success() {
        let output: Vec<String> = command.lines().into_iter().map(|line| line.text).collect();
        panic!("{what}: {status}\n  {}", output.join("\n  "));
    }
    command
}

/// `rutis-host` as a user installs it from npm, in `tools/`: its launcher.
fn tools(s: &Scenario) -> PathBuf {
    s.write_json(
        "tools/package.json",
        &json!({ "name": "tools", "private": true, "type": "module" }),
    );
    let platform = format!("rutis-host-{}", npm_platform());
    let files: Vec<String> = ["rutis-host", platform.as_str(), "rutis-runtime", "rutis"]
        .into_iter()
        .map(npm_tarball)
        .collect();
    let mut args = vec!["install", "--omit=optional"];
    args.extend(files.iter().map(String::as_str));
    run(s, "tools", npm(), &args, &[]);
    s.path(if cfg!(windows) {
        "tools/node_modules/.bin/rutis-host.cmd"
    } else {
        "tools/node_modules/.bin/rutis-host"
    })
}

/// `rutis-host new weather --lang <lang>` in `dir`: the project `dir/weather`.
fn new(s: &Scenario, launcher: &Path, dir: &str, lang: &str) -> String {
    std::fs::create_dir_all(s.path(dir)).unwrap();
    let new = run(s, dir, launcher, &["new", "weather", "--lang", lang], &[]);
    let created = new
        .lines()
        .into_iter()
        .any(|line| line.text.contains("created weather/"));
    assert!(
        created,
        "rutis-host new --lang {lang} said no `created weather/`"
    );
    format!("{dir}/weather")
}

/// A project's `rutis-host check`: it knows the plugin provides `weather`.
fn check(s: &Scenario, project: &str, launcher: &Path, env: &[(&str, &str)]) {
    let mut check = run(s, project, launcher, &["check"], env);
    check.expect("\"weather\":{\"today\":\"async\"}");
}

/// `rutis-host dev` in `project`: the plugin applies (`weather v1
/// applied`), an edit of `source` reloads it (`weather v2 applied`),
/// Ctrl-C ends it with status 0. `reloaded`: the line saying it reloaded.
fn dev(
    s: &Scenario,
    project: &str,
    launcher: &Path,
    source: &str,
    reloaded: &str,
    env: &[(&str, &str)],
) {
    let mut dev = s.program(project, launcher, ["dev"], env);
    dev.wait_for_within(build_guard(), "the plugin to apply", |line| {
        line.contains("weather v1 applied")
    });
    // (A Go binary's: it names one of the binary's plugins, not always
    // the project's own.)
    dev.expect("; changes reload it (Ctrl-C ends)");
    // Saved whole, as an editor does: a watcher never reads half a file.
    let file = format!("{project}/{source}");
    let text = std::fs::read_to_string(s.path(&file)).unwrap();
    assert!(
        text.contains("weather v1 applied"),
        "{source} has no `weather v1 applied`"
    );
    s.replace(
        &file,
        text.replace("weather v1 applied", "weather v2 applied"),
    );
    dev.wait_for_within(build_guard(), "the edit to reload the plugin", |line| {
        line.contains(reloaded)
    });
    dev.wait_for_within(build_guard(), "the edited plugin to apply", |line| {
        line.contains("weather v2 applied")
    });
    dev.ctrl_c();
    let status = dev.wait_exit();
    assert!(status.success(), "rutis-host dev after Ctrl-C: {status}");
}

/// The file `name` exists in `dir` (what packing made).
fn packed(s: &Scenario, dir: &str, name: &str) {
    let path = s.path(dir).join(name);
    assert!(path.is_file(), "packing made no {}", path.display());
}

// ── TypeScript and Bun ──────────────────────────────────────────

/// The `weather` plugin in TypeScript (Node and Bun).
const WEATHER_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

export interface Config {
  city?: string
}

interface Llm {
  ask(question: string): Promise<string>
}

export default definePlugin<Config>({
  inject: ['llm'],
  provides: { weather: { today: 'async' } },
  config: { type: 'object', properties: { city: { type: 'string', default: 'Oslo' } } },
  apply(ctx, config) {
    const llm = ctx.use('llm') as Llm
    const home = config.city ?? 'Oslo'
    ctx.provide('weather', {
      async today(city?: string): Promise<string> {
        const where = city ?? home
        const answer = await llm.ask(`weather in ${where}?`)
        ctx.emit('weather/asked', where)
        return `${where}: ${answer}`
      },
    })
    console.log('weather v1 applied')
  },
})
"#;

/// A fake `llm` for `dev`.
const FAKE_LLM_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  provides: { llm: { ask: 'async' } },
  apply(ctx) {
    ctx.provide('llm', { async ask(question: string) { return question.includes('Oslo') ? 'sunny' : 'rain' } })
  },
})
"#;

/// The plugin's own tests (`node:test` or `bun:test`, by `header`).
fn weather_test_ts(header: &str, equal: &str) -> String {
    format!(
        r#"{header}
import {{ definePlugin }} from '@arcships/rutis'
import {{ host, load }} from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

const llm = {{ async ask(question: string) {{ return question.includes('Oslo') ? 'sunny' : 'rain' }} }}

test('today asks the llm, for the configured city by default', async () => {{
  const t = await load(plugin, {{ config: {{}}, services: {{ llm }} }})
  {eq_default}
  {eq_bergen}
  await t.unload()
}})

test('each call emits weather/asked', async () => {{
  const asked: string[] = []
  const h = host({{ services: {{ llm }} }})
  const listener = await h.load(definePlugin({{ apply(ctx) {{ ctx.on('weather/asked', (city: string) => {{ asked.push(city) }}) }} }}))
  const t = await h.load(plugin, {{ config: {{ city: 'Bergen' }} }})
  await t.service('weather').today()
  {eq_asked}
  await t.unload()
  await listener.unload()
}})
"#,
        eq_default = equal
            .replace("ACTUAL", "await t.service('weather').today()")
            .replace("EXPECTED", "'Oslo: sunny'"),
        eq_bergen = equal
            .replace("ACTUAL", "await t.service('weather').today('Bergen')")
            .replace("EXPECTED", "'Bergen: rain'"),
        eq_asked = equal
            .replace("ACTUAL", "asked")
            .replace("EXPECTED", "['Bergen']"),
    )
}

/// The template's `package.json` with its `@arcships/*` packages from the
/// artifacts.
fn from_artifacts(s: &Scenario, project: &str, dev: &[&str]) {
    let file = format!("{project}/package.json");
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(s.path(&file)).unwrap()).unwrap();
    manifest["dependencies"] = Value::Object(npm_files(&["rutis"]));
    let mut dev_dependencies = manifest["devDependencies"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    dev_dependencies.extend(npm_files(dev));
    manifest["devDependencies"] = Value::Object(dev_dependencies);
    s.write_json(&file, &manifest);
}

/// The `@arcships/*` packages npm installed in `project` came from the
/// artifacts, not a registry (as install.rs checks).
fn npm_from_artifacts(s: &Scenario, project: &str, packages: &[&str]) {
    let lock: Value = serde_json::from_str(
        &std::fs::read_to_string(s.path(project).join("package-lock.json")).unwrap(),
    )
    .unwrap();
    for package in packages {
        let resolved = lock["packages"][format!("node_modules/@arcships/{package}")]["resolved"]
            .as_str()
            .unwrap_or_default();
        assert!(
            resolved.starts_with("file:"),
            "@arcships/{package} came from {resolved:?}"
        );
    }
}

/// The same for what Bun installed: `bun.lock` names the tarball among
/// the artifacts (a registry's package is `<name>@<version>`, its URL
/// left out for the default registry).
/// Point every `@arcships/*` package Bun resolves, the ones other packages
/// depend on too (rutis-host on rutis-runtime), at this build's tarballs.
/// Without it Bun may resolve such a dependency from the registry, which has
/// no package of an unreleased version.
fn bun_overrides(s: &Scenario, project: &str, packages: &[&str]) {
    let file = format!("{project}/package.json");
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(s.path(&file)).unwrap()).unwrap();
    manifest["overrides"] = Value::Object(npm_files(packages));
    s.write_json(&file, &manifest);
}

fn bun_from_artifacts(s: &Scenario, project: &str, packages: &[&str]) {
    let lock = std::fs::read_to_string(s.path(project).join("bun.lock")).unwrap();
    for package in packages {
        let entry = format!(
            "\"@arcships/{package}\": [\"@arcships/{package}@{}",
            npm_tarball(package)
        );
        assert!(
            lock.lines()
                .any(|line| line.trim_start().starts_with(&entry)),
            "@arcships/{package} is not this build's in bun.lock:\n{lock}"
        );
    }
}

/// `rutis-host check` on the template as `new` wrote it: it knows the
/// template's `greeter`.
fn check_template(s: &Scenario, project: &str, launcher: &Path, env: &[(&str, &str)]) {
    let mut check = run(s, project, launcher, &["check"], env);
    check.expect("\"greeter\":{\"hello\":\"async\"}");
}

/// risk: B1 (the TypeScript plugin author's path, #663)
#[test]
#[ignore = "needs built packages: install.yml runs it with RUTIS_E2E_ARTIFACTS"]
fn a_typescript_plugin_is_written_tested_reloaded_and_packed() {
    let s = Scenario::installed("devx-typescript");
    let launcher = tools(&s);
    let project = new(&s, &launcher, "ts", "node");
    let platform = format!("rutis-host-{}", npm_platform());
    let ours = ["rutis", "rutis-host", platform.as_str(), "rutis-runtime"];
    from_artifacts(&s, &project, &ours[1..]);
    run(&s, &project, npm(), &["install", "--omit=optional"], &[]);
    npm_from_artifacts(&s, &project, &ours);
    // The template as it comes: its example and its tests.
    run(&s, &project, npm(), &["test"], &[]);
    check_template(&s, &project, &launcher, &[]);
    // The weather plugin.
    s.replace(format!("{project}/src/index.ts"), WEATHER_TS);
    s.write(format!("{project}/dev/fake-llm.ts"), FAKE_LLM_TS);
    s.replace(
        format!("{project}/test/index.test.ts"),
        weather_test_ts(
            "import { test } from 'node:test'\nimport assert from 'node:assert/strict'",
            "assert.deepEqual(ACTUAL, EXPECTED)",
        ),
    );
    s.write_json(
        format!("{project}/rutis.dev.json"),
        &json!({
            "format": 1,
            "rows": [
                { "id": "weather", "config": { "city": "Oslo" } },
                { "id": "llm", "name": "./dev/fake-llm.ts" },
            ],
        }),
    );
    run(&s, &project, npm(), &["test"], &[]);
    check(&s, &project, &launcher, &[]);
    dev(
        &s,
        &project,
        &launcher,
        "src/index.ts",
        "weather: reloaded (",
        &[],
    );
    run(&s, &project, npm(), &["pack"], &[]);
    packed(&s, &project, "weather-0.1.0.tgz");
    s.finish();
}

/// risk: B1 (the Bun plugin author's path, #663)
#[cfg(unix)]
#[test]
#[ignore = "needs built packages: install.yml runs it with RUTIS_E2E_ARTIFACTS"]
fn a_bun_plugin_is_written_tested_reloaded_and_packed() {
    let s = Scenario::installed("devx-bun");
    let launcher = tools(&s);
    let project = new(&s, &launcher, "bun", "bun");
    let platform = format!("rutis-host-{}", npm_platform());
    // rutis-host depends on rutis-runtime: this build's too.
    let ours = [
        "rutis",
        "rutis-host",
        platform.as_str(),
        "rutis-runtime",
        "rutis-bun",
    ];
    from_artifacts(&s, &project, &ours[1..]);
    bun_overrides(&s, &project, &ours);
    // Bun's own temporary files (the tarballs it unpacks, which it may
    // remove only later) in a directory of their own: the residue checks
    // look for this scenario's processes' leftovers in its tmp/.
    let bun_tmp = s.path("bun-tmp");
    std::fs::create_dir_all(&bun_tmp).unwrap();
    let bun_tmp = bun_tmp.display().to_string();
    let bun_env = [("TMPDIR", bun_tmp.as_str())];
    run(&s, &project, "bun", &["install"], &bun_env);
    bun_from_artifacts(&s, &project, &ours);
    // The template as it comes: its example and its tests.
    run(&s, &project, "bun", &["test"], &[]);
    check_template(&s, &project, &launcher, &[]);
    // The weather plugin.
    s.replace(format!("{project}/src/index.ts"), WEATHER_TS);
    s.write(format!("{project}/dev/fake-llm.ts"), FAKE_LLM_TS);
    s.replace(
        format!("{project}/test/index.test.ts"),
        weather_test_ts(
            "import { expect, test } from 'bun:test'",
            "expect(ACTUAL).toEqual(EXPECTED)",
        ),
    );
    s.write_json(
        format!("{project}/rutis.dev.json"),
        &json!({
            "format": 1,
            "rows": [
                { "id": "weather", "config": { "city": "Oslo" } },
                { "id": "llm", "name": "bun:./dev/fake-llm.ts" },
            ],
        }),
    );
    run(&s, &project, "bun", &["test"], &[]);
    check(&s, &project, &launcher, &[]);
    dev(
        &s,
        &project,
        &launcher,
        "src/index.ts",
        "weather: reloaded (",
        &[],
    );
    run(&s, &project, "bun", &["pm", "pack"], &bun_env);
    packed(&s, &project, "weather-0.1.0.tgz");
    s.finish();
}

// ── Python ──────────────────────────────────────────────────────

const WEATHER_PY: &str = r#""""weather: a rutis plugin."""

from rutis import define_plugin


class Weather:
    def __init__(self, ctx, llm, home: str) -> None:
        self._ctx, self._llm, self._home = ctx, llm, home

    async def today(self, city: str | None = None) -> str:
        where = city or self._home
        answer = await self._llm.ask(f"weather in {where}?")
        self._ctx.emit("weather/asked", where)
        return f"{where}: {answer}"


def apply(ctx, config):
    ctx.provide("weather", Weather(ctx, ctx.use("llm"), config.get("city", "Oslo")))
    print("weather v1 applied", flush=True)


plugin = define_plugin(
    apply,
    inject=["llm"],
    provides={"weather": Weather},
    config={"type": "object", "properties": {"city": {"type": "string", "default": "Oslo"}}},
)
"#;

const FAKE_LLM_PY: &str = r#""""A fake llm for `dev`."""

from rutis import define_plugin


class Llm:
    async def ask(self, question: str) -> str:
        return "sunny" if "Oslo" in question else "rain"


def apply(ctx, config):
    ctx.provide("llm", Llm())


plugin = define_plugin(apply, provides={"llm": Llm})
"#;

const WEATHER_TEST_PY: &str = r#"import unittest

from rutis import define_plugin
from rutis.testing import Bus, load

from weather import plugin


class FakeLlm:
    async def ask(self, question: str) -> str:
        return "sunny" if "Oslo" in question else "rain"


class Plugin(unittest.IsolatedAsyncioTestCase):
    async def test_today_asks_the_llm_for_the_configured_city_by_default(self):
        async with load(plugin, config={}, services={"llm": FakeLlm()}) as t:
            self.assertEqual(await t.service("weather").today(), "Oslo: sunny")
            self.assertEqual(await t.service("weather").today("Bergen"), "Bergen: rain")

    async def test_each_call_emits_weather_asked(self):
        asked = []
        listener = define_plugin(lambda ctx, config: ctx.on("weather/asked", asked.append))
        bus = Bus()
        async with load(listener, bus=bus), load(
            plugin, config={"city": "Bergen"}, services={"llm": FakeLlm()}, bus=bus
        ) as t:
            await t.service("weather").today()
        self.assertEqual(asked, ["Bergen"])


if __name__ == "__main__":
    unittest.main()
"#;

/// risk: B1 (the Python plugin author's path, with uv as the guide does)
#[test]
#[ignore = "needs built packages: install.yml runs it with RUTIS_E2E_ARTIFACTS"]
fn a_python_plugin_is_written_tested_reloaded_and_packed() {
    let s = Scenario::installed("devx-python");
    let launcher = tools(&s);
    let project = new(&s, &launcher, "py", "python");
    // The project's environment, with rutis and rutis-host from the
    // wheels, and the plugin itself (editable, as `uv sync` does).
    run(&s, &project, "uv", &["venv", ".venv"], &[]);
    let venv = s.path(&project).join(".venv");
    let python = venv_program(&venv, "python").display().to_string();
    let (rutis, host) = (wheel("rutis-"), wheel("rutis_host-"));
    run(
        &s,
        &project,
        "uv",
        &[
            "pip", "install", "--python", &python, &rutis, &host, "-e", ".",
        ],
        &[],
    );
    // The venv's rutis-host, the one the PyPI package installs.
    let venv_host = venv_program(&venv, "rutis-host");
    // The template as it comes: its example and its tests.
    let unittest = ["-m", "unittest", "discover", "-s", "tests"];
    run(&s, &project, &python, &unittest, &[]);
    check_template(&s, &project, &venv_host, &[]);
    // The weather plugin.
    s.replace(format!("{project}/src/weather/__init__.py"), WEATHER_PY);
    s.write(format!("{project}/src/fake_llm.py"), FAKE_LLM_PY);
    s.replace(format!("{project}/tests/test_plugin.py"), WEATHER_TEST_PY);
    s.write_json(
        format!("{project}/rutis.dev.json"),
        &json!({
            "format": 1,
            "rows": [
                { "id": "weather", "config": { "city": "Oslo" } },
                { "id": "llm", "name": "py:fake_llm" },
            ],
        }),
    );
    run(&s, &project, &python, &unittest, &[]);
    check(&s, &project, &venv_host, &[]);
    dev(
        &s,
        &project,
        &venv_host,
        "src/weather/__init__.py",
        "weather: reloaded (",
        &[],
    );
    run(&s, &project, "uv", &["build", "--out-dir", "dist"], &[]);
    packed(
        &s,
        &format!("{project}/dist"),
        "weather-0.1.0-py3-none-any.whl",
    );
    s.finish();
}

// ── Go ──────────────────────────────────────────────────────────

const WEATHER_GO: &str = r#"// Package weather is a rutis plugin.
package weather

import (
	"context"
	"fmt"
	"strings"

	"github.com/arcships/rutis/go/rutis"
)

// Config is the plugin's configuration.
type Config struct {
	City string `json:"city,omitempty" doc:"the city today asks about (Oslo)"`
}

// LLM is the service it uses: function fields named after its methods.
type LLM struct {
	Ask func(ctx context.Context, question string) (string, error)
}

// Weather is the service the plugin provides.
type Weather struct {
	ctx  *rutis.Ctx
	llm  LLM
	home string
}

// Today asks the llm about city (the configured one when empty).
func (w *Weather) Today(ctx context.Context, city string) (string, error) {
	if city == "" {
		city = w.home
	}
	answer, err := w.llm.Ask(ctx, "weather in "+city+"?")
	if err != nil {
		return "", err
	}
	if err := w.ctx.Emit(ctx, "weather/asked", city); err != nil {
		return "", err
	}
	return fmt.Sprintf("%s: %s", city, answer), nil
}

var Plugin = rutis.Define(rutis.Plugin[Config]{
	Name:     "weather",
	Inject:   []string{"llm"},
	Provides: rutis.Provides{"weather": rutis.MethodsOf[*Weather]()},
	Apply: func(ctx *rutis.Ctx, config Config) error {
		home := config.City
		if home == "" {
			home = "Oslo"
		}
		var llm LLM
		if err := ctx.Use("llm", &llm); err != nil {
			return err
		}
		ctx.Provide("weather", &Weather{ctx: ctx, llm: llm, home: home})
		fmt.Println("weather v1 applied")
		return nil
	},
})

// FakeLLM is a fake llm for `dev` and the tests.
type FakeLLM struct{}

// Ask answers sunny for Oslo, rain elsewhere.
func (FakeLLM) Ask(question string) string {
	if strings.Contains(question, "Oslo") {
		return "sunny"
	}
	return "rain"
}

// FakeLlm provides it.
var FakeLlm = rutis.Define(rutis.Plugin[struct{}]{
	Name:     "fakellm",
	Provides: rutis.Provides{"llm": rutis.MethodsOf[FakeLLM]()},
	Apply: func(ctx *rutis.Ctx, _ struct{}) error {
		ctx.Provide("llm", FakeLLM{})
		return nil
	},
})
"#;

const WEATHER_TEST_GO: &str = r#"package weather

import (
	"context"
	"testing"
	"time"

	"github.com/arcships/rutis/go/rutis/rutistest"
)

func today(t *testing.T, loaded *rutistest.Loaded, city string) string {
	var answer string
	if err := loaded.Service("weather").Call(context.Background(), "today", &answer, city); err != nil {
		t.Fatal(err)
	}
	return answer
}

func TestTodayAsksTheLlmForTheConfiguredCityByDefault(t *testing.T) {
	loaded := rutistest.Load(t, Plugin, Config{}, map[string]any{"llm": FakeLLM{}})
	if got := today(t, loaded, ""); got != "Oslo: sunny" {
		t.Fatalf("got %q", got)
	}
	if got := today(t, loaded, "Bergen"); got != "Bergen: rain" {
		t.Fatalf("got %q", got)
	}
}

func TestEachCallEmitsWeatherAsked(t *testing.T) {
	loaded := rutistest.Load(t, Plugin, Config{City: "Bergen"}, map[string]any{"llm": FakeLLM{}})
	asked := make(chan any, 1)
	loaded.On("weather/asked", func(_ context.Context, args ...any) (any, error) {
		asked <- args[0]
		return nil, nil
	})
	today(t, loaded, "")
	// An emit returns at once; its listeners run on.
	select {
	case city := <-asked:
		if city != "Bergen" {
			t.Fatalf("asked %v", city)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("weather/asked was not emitted")
	}
}
"#;

/// Go builds use this checkout's SDK and the module cache, never the
/// network (as dev_loop.rs, install.rs).
fn go_env(cache: &str) -> Vec<(&'static str, String)> {
    vec![
        ("GOPROXY", "off".into()),
        ("GOTOOLCHAIN", "local".into()),
        ("GOFLAGS", "-mod=mod".into()),
        ("GOCACHE", cache.into()),
    ]
}

/// risk: B1 (the Go plugin author's path, #663)
#[test]
#[ignore = "needs built packages: install.yml runs it with RUTIS_E2E_ARTIFACTS"]
fn a_go_plugin_is_written_tested_reloaded_and_packed() {
    let s = Scenario::installed("devx-go");
    let launcher = tools(&s);
    let project = new(&s, &launcher, "go", "go");
    // An unreleased SDK is on no proxy: this checkout's.
    let sdk = repo().join("go/rutis").display().to_string();
    let sdk = sdk.strip_prefix(r"\\?\").unwrap_or(&sdk).replace('\\', "/");
    s.edit(format!("{project}/go.mod"), |module| {
        format!("{module}\nreplace github.com/arcships/rutis/go/rutis => {sdk}\n")
    });
    let cache = std::process::Command::new("go")
        .args(["env", "GOCACHE"])
        .output()
        .expect("go env GOCACHE");
    let cache = String::from_utf8_lossy(&cache.stdout).trim().to_owned();
    let env = go_env(&cache);
    let env: Vec<(&str, &str)> = env
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    // The template as it comes: its example and its tests.
    run(&s, &project, "go", &["test", "./..."], &env);
    check_template(&s, &project, &launcher, &env);
    // The weather plugin.
    s.replace(format!("{project}/plugin.go"), WEATHER_GO);
    s.replace(format!("{project}/plugin_test.go"), WEATHER_TEST_GO);
    s.replace(
        format!("{project}/cmd/weather/main.go"),
        "// The weather binary: what a host runs.\npackage main\n\nimport (\n\t\"github.com/arcships/rutis/go/rutis\"\n\n\tweather \"example.com/weather\"\n)\n\nfunc main() { rutis.Serve(weather.Plugin, weather.FakeLlm) }\n",
    );
    s.write_json(
        format!("{project}/rutis.dev.json"),
        &json!({
            "format": 1,
            "rows": [
                // The binary's other plugins are rows of their own:
                // `fakellm` provides llm.
                { "id": "weather", "config": { "city": "Oslo" } },
            ],
        }),
    );
    run(&s, &project, "go", &["test", "./..."], &env);
    check(&s, &project, &launcher, &env);
    dev(
        &s,
        &project,
        &launcher,
        "plugin.go",
        "go-weather: rebuilt and restarted",
        &env,
    );
    let binary = if cfg!(windows) {
        "weather.exe"
    } else {
        "weather"
    };
    run(
        &s,
        &project,
        "go",
        &["build", "-o", &format!("dist/{binary}"), "./cmd/weather"],
        &env,
    );
    packed(&s, &format!("{project}/dist"), binary);
    s.finish();
}

// ── Rust ────────────────────────────────────────────────────────

const WEATHER_RS: &str = r#"//! weather: a rutis plugin, compiled into a Rust host.

use std::sync::Arc;

use rutis::{Ctx, Event, Plugin};
use serde::{Deserialize, Serialize};

/// What the plugin asks.
pub trait Llm: Send + Sync {
    fn ask(&self, question: &str) -> String;
}

/// Its config: `{ city }`, Oslo by default.
#[derive(Deserialize)]
pub struct Config {
    #[serde(default = "oslo")]
    pub city: String,
}

fn oslo() -> String {
    "Oslo".into()
}

/// Emitted on each call.
#[derive(Serialize, Deserialize)]
pub struct Asked(pub String);

impl Event for Asked {
    const NAME: &'static str = "weather/asked";
    type Value = ();
}

/// The service it provides.
pub struct Weather {
    ctx: Ctx,
    llm: Arc<dyn Llm>,
    home: String,
}

impl Weather {
    /// Asks the llm about `city` (the configured one when `None`).
    pub fn today(&self, city: Option<&str>) -> String {
        let city = city.unwrap_or(&self.home).to_owned();
        let answer = self.llm.ask(&format!("weather in {city}?"));
        self.ctx.emit(Asked(city.clone()));
        format!("{city}: {answer}")
    }
}

/// The plugin, from its config.
pub struct WeatherPlugin {
    pub config: Config,
}

impl Plugin for WeatherPlugin {
    type Deps = Arc<dyn Llm>;

    async fn apply(&self, ctx: &Ctx, llm: Arc<dyn Llm>) -> rutis::Result {
        let weather = Weather { ctx: ctx.clone(), llm, home: self.config.city.clone() };
        ctx.provide(weather)?;
        println!("weather v1 applied");
        Ok(())
    }
}
"#;

const WEATHER_TEST_RS: &str = r#"use std::sync::{Arc, Mutex};

use rutis::host::*;
use rutis::{Ctx, Plugin};
use weather::{Asked, Config, Llm, Weather, WeatherPlugin};

struct FakeLlm;

impl Llm for FakeLlm {
    fn ask(&self, question: &str) -> String {
        if question.contains("Oslo") { "sunny".into() } else { "rain".into() }
    }
}

fn config(json: serde_json::Value) -> Config {
    serde_json::from_value(json).unwrap()
}

#[tokio::test]
async fn today_asks_the_llm_for_the_configured_city_by_default() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;
    root.provide_as::<dyn Llm>(Arc::new(FakeLlm))?;
    let weather = root.plugin(WeatherPlugin { config: config(serde_json::json!({})) });
    (&weather).await?;
    let service = root.get::<Weather>().unwrap();
    assert_eq!(service.today(None), "Oslo: sunny");
    assert_eq!(service.today(Some("Bergen")), "Bergen: rain");
    root.shutdown().await?;
    Ok(())
}

struct Listener(Arc<Mutex<Vec<String>>>);

impl Plugin for Listener {
    type Deps = ();

    async fn apply(&self, ctx: &Ctx, _: ()) -> rutis::Result {
        let asked = self.0.clone();
        ctx.on::<Asked>(move |event, _| {
            let asked = asked.clone();
            async move {
                asked.lock().unwrap().push(event.0.clone());
                Ok(None)
            }
        })?;
        Ok(())
    }
}

#[tokio::test]
async fn each_call_emits_weather_asked() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;
    root.provide_as::<dyn Llm>(Arc::new(FakeLlm))?;
    let asked = Arc::new(Mutex::new(Vec::new()));
    (&root.plugin(Listener(asked.clone()))).await?;
    let weather = root.plugin(WeatherPlugin { config: config(serde_json::json!({ "city": "Bergen" })) });
    (&weather).await?;
    root.get::<Weather>().unwrap().today(None);
    // An emit returns at once; its listeners run on.
    for _ in 0..100 {
        if !asked.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(*asked.lock().unwrap(), ["Bergen"]);
    root.shutdown().await?;
    Ok(())
}
"#;

/// risk: B1 (the Rust plugin author's path, #663): `cargo new`, the plugin
/// with this checkout's `rutis` (an unreleased version is on no
/// registry), its tests, and `cargo package`'s file list (packing would
/// resolve `rutis` from crates.io, which has no unreleased version).
#[test]
#[ignore = "needs built packages: install.yml runs it with RUTIS_E2E_ARTIFACTS"]
fn a_rust_plugin_is_written_tested_and_packed() {
    // Not installed mode: cargo is the developer's own.
    let s = Scenario::new("devx-rust");
    run(
        &s,
        "",
        "cargo",
        &["new", "--lib", "--vcs", "none", "weather"],
        &[],
    );
    let rutis = repo().join("crates/rutis").display().to_string();
    let rutis = rutis
        .strip_prefix(r"\\?\")
        .unwrap_or(&rutis)
        .replace('\\', "/");
    s.edit("weather/Cargo.toml", |manifest| {
        format!(
            "{manifest}rutis = {{ version = \"{}\", path = \"{rutis}\" }}\nserde = {{ version = \"1\", features = [\"derive\"] }}\n\n[dev-dependencies]\nserde_json = \"1\"\ntokio = {{ version = \"1\", features = [\"rt-multi-thread\", \"macros\", \"time\"] }}\n\n[workspace]\n",
            version()
        )
    });
    s.replace("weather/src/lib.rs", WEATHER_RS);
    // This checkout's lockfile: the dependency versions its CI built and
    // checked, not whatever crates.io has newest tonight (cargo adds the
    // plugin and drops what it does not use).
    s.write(
        "weather/Cargo.lock",
        std::fs::read(repo().join("Cargo.lock")).unwrap(),
    );
    s.write("weather/tests/weather.rs", WEATHER_TEST_RS);
    // Its builds next to this checkout's: the same dependencies, built once.
    let target = repo().join("target/devx").display().to_string();
    let env = [("CARGO_TARGET_DIR", target.as_str())];
    run(&s, "weather", "cargo", &["test"], &env);
    let mut list = run(
        &s,
        "weather",
        "cargo",
        &["package", "--list", "--allow-dirty"],
        &env,
    );
    list.expect("src/lib.rs");
    s.finish();
}

// ── Assembling plugins ──────────────────────────────────────────

/// The TypeScript plugin assembled: `weather`, using Python's `llm`.
const ASSEMBLED_WEATHER_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

export default definePlugin<{ city?: string }>({
  inject: ['llm'],
  provides: { weather: { today: 'async' } },
  config: { type: 'object', properties: { city: { type: 'string', default: 'Oslo' } } },
  apply(ctx, config) {
    const llm = ctx.use('llm') as { ask(question: string): Promise<string> }
    ctx.provide('weather', {
      async today(city?: string) {
        const where = city ?? config.city ?? 'Oslo'
        return `${where}: ${await llm.ask(`weather in ${where}?`)}`
      },
    })
  },
})
"#;

/// The Python plugin assembled: `llm`.
const ASSEMBLED_LLM_PY: &str = r#"from rutis import define_plugin


class Llm:
    async def ask(self, question: str) -> str:
        return ("sunny" if "Oslo" in question else "rain") + " (python)"


plugin = define_plugin(lambda ctx, config: ctx.provide("llm", Llm()), provides={"llm": Llm})
"#;

/// risk: B1 (assembling plugins with rutis-host, #663)
#[test]
#[ignore = "needs built packages: install.yml runs it with RUTIS_E2E_ARTIFACTS"]
fn plugins_in_two_languages_are_assembled_run_changed_and_stopped() {
    let s = Scenario::installed("devx-assemble");
    let launcher = tools(&s);
    // The Python runtime's environment: rutis from the wheel.
    run(&s, "tools", "uv", &["venv", ".venv"], &[]);
    let python = venv_program(&s.path("tools/.venv"), "python")
        .display()
        .to_string();
    run(
        &s,
        "tools",
        "uv",
        &["pip", "install", "--python", &python, &wheel("rutis-")],
        &[],
    );
    s.write("tools/weather.ts", ASSEMBLED_WEATHER_TS);
    s.write("tools/llm.py", ASSEMBLED_LLM_PY);
    let probe = s.probe_in("tools", "probe", Lang::Node, &["weather"]);
    let config = |city: &str, llm_disabled: bool| {
        json!({
            "runtimes": { "node": {}, "py": { "project": ".", "python": python } },
            "rows": [
                { "id": "weather", "name": "./weather.ts", "config": { "city": city } },
                { "id": "llm", "name": "py:llm", "disabled": llm_disabled },
                probe.row(),
            ],
        })
    };
    s.write_json("tools/rutis.json", &config("Oslo", false));

    // check: both rows, what each provides, nothing missing.
    let mut check = run(&s, "tools", &launcher, &["check"], &[]);
    check.expect("\"weather\":{\"today\":\"async\"}");
    check.expect("\"llm\":{\"ask\":\"async\"}");

    // run, and a call end to end: TypeScript → Python.
    let mut host = s.program("tools", &launcher, ["run", "--watch-config"], &[]);
    probe.started(&mut host);
    assert_eq!(
        probe.call(&mut host, "weather", "today", json!([])),
        Ok(json!("Oslo: sunny (python)"))
    );

    // The configuration changed while it runs: the row restarts with it.
    host.skip_written();
    s.replace("tools/rutis.json", config("Bergen", false).to_string());
    host.expect("rutis-host: reloaded rutis.json (1 row changed)");
    probe.started(&mut host);
    assert_eq!(
        probe.call(&mut host, "weather", "today", json!([])),
        Ok(json!("Bergen: rain (python)"))
    );

    // A row disabled: the one using its service waits, and says for what.
    host.skip_written();
    s.replace("tools/rutis.json", config("Bergen", true).to_string());
    host.expect("llm: disabled");
    host.expect("weather: waiting for llm");

    // Stopped: exit status 0.
    host.ctrl_c();
    let status = host.wait_exit();
    assert!(status.success(), "rutis-host run after Ctrl-C: {status}");
    s.finish();
}

// ── A host owner ────────────────────────────────────────────────

/// The third party's tool plugin, from the template: it registers the tool
/// `weather` with the host's extension point. A call without a city fails
/// with the plugin's own error code.
const TOOL_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

interface Tools {
  register(name: string, description: string, run: (input: { city?: string }) => Promise<string>): Promise<void>
}

export default definePlugin({
  inject: ['tools'],
  async apply(ctx) {
    const tools = ctx.use('tools') as Tools
    await tools.register('weather', 'the weather in a city', async input => {
      if (!input?.city) throw Object.assign(new Error('which city? pass { "city": … }'), { code: 'no-city' })
      return `${input.city}: sunny`
    })
  },
})
"#;

/// One JSON line the tool host answers.
fn toolhost_answer(host: &mut Host, what: &str) -> Value {
    // As long as a start may take (the host waits up to a minute for its
    // plugins): a busy runner starts Node slowly.
    let line = host.wait_for_within(build_guard(), what, |line| line.starts_with('{'));
    serde_json::from_str(&line).unwrap_or_else(|error| panic!("{line}: {error}"))
}

/// risk: B1 (the host owner's path, #663: an extension point, a third
/// party's plugin, its errors structured)
#[test]
#[ignore = "needs built packages: install.yml runs it with RUTIS_E2E_ARTIFACTS"]
fn a_host_owners_extension_point_takes_a_template_plugin_and_its_errors() {
    let s = Scenario::installed("devx-host-owner");
    // Built first: a build's time is not the host's.
    let _ = rutis_e2e::fixture_toolhost_program();
    let launcher = tools(&s);
    // The third party: a plugin from the template, on the packages a user
    // installs.
    let project = new(&s, &launcher, "third-party", "node");
    let platform = format!("rutis-host-{}", npm_platform());
    let ours = ["rutis", "rutis-host", platform.as_str(), "rutis-runtime"];
    from_artifacts(&s, &project, &ours[1..]);
    s.replace(format!("{project}/src/index.ts"), TOOL_TS);
    // Its tests were the template's greeter's.
    std::fs::remove_file(s.path(&project).join("test/index.test.ts")).unwrap();
    run(&s, &project, npm(), &["install", "--omit=optional"], &[]);
    npm_from_artifacts(&s, &project, &ours);
    let entry = s.path(&project).join("src/index.ts").display().to_string();
    s.write_json(
        format!("{project}/toolhost.json"),
        &json!({ "plugins": [{ "id": "weather-tool", "name": entry }] }),
    );
    // The host, with the Node runtime the plugin installed.
    let runtime = s
        .path(&project)
        .join("node_modules/@arcships/rutis-runtime")
        .display()
        .to_string();
    let mut host = s.fixture_toolhost(&project, &[("RUTIS_NODE_RUNTIME", runtime.as_str())]);
    assert_eq!(
        toolhost_answer(&mut host, "the host to start"),
        json!({ "ready": true })
    );

    // The host lists the tool the plugin registered, and calls it.
    host.send_line("list");
    assert_eq!(
        toolhost_answer(&mut host, "the tools"),
        json!({ "tools": [{ "name": "weather", "description": "the weather in a city" }] })
    );
    host.send_line(r#"call weather {"city":"Oslo"}"#);
    assert_eq!(
        toolhost_answer(&mut host, "the call"),
        json!({ "ok": "Oslo: sunny" })
    );

    // The tool fails: the host gets the plugin's error, structured, with
    // the plugin's own code.
    host.send_line("call weather {}");
    let failed = toolhost_answer(&mut host, "the failed call");
    assert_eq!(
        failed,
        json!({ "error": { "name": "Error", "code": "no-city", "message": "which city? pass { \"city\": … }" } })
    );

    host.send_line("stop");
    let status = host.wait_exit();
    assert!(status.success(), "the tool host after stop: {status}");
    s.finish();
}
