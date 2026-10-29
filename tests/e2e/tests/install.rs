//! S9, installed and run (#193): rutis-host as a user gets it, from npm,
//! from PyPI and as the release binary, in a new directory with a new home
//! and none of this checkout's runtimes ([`Scenario::installed`]). Each
//! channel installs, checks that this build is what runs (its version, and
//! where the binary is), runs a minimal project, stops it as a process
//! manager does, and ends with the residue checks. Risk B1 (P0). Besides:
//! npm with a Bun row (`@arcships/rutis-bun`; not on Windows, which Bun is
//! not declared for, Bun design §10), PyPI with `rutis[network]` at its
//! lowest websockets (15.0) serving a host as a remote runtime
//! (`python -m rutis listen:`), the release binary with a Go row.
//!
//! The packages come from `RUTIS_E2E_ARTIFACTS` (what package.yml builds,
//! or tools/install/local-artifacts.sh on a development machine), or with
//! `RUTIS_E2E_SOURCE=registry` from the registries, at `RUTIS_E2E_VERSION`
//! (after a release). install.yml runs them; `cargo test` skips them:
//!
//!   RUTIS_E2E_ARTIFACTS=<dir> cargo test -p rutis-e2e --test install -- --ignored --test-threads=1

use std::path::{Path, PathBuf};

use rutis_e2e::{build_guard, repo, Host, Lang, Scenario};
use serde_json::{json, Value};

const IGNORED: &str = "needs built packages: install.yml runs it with RUTIS_E2E_ARTIFACTS";

/// Where the built packages are (npm/, wheels/, binary/).
fn artifacts() -> PathBuf {
    PathBuf::from(std::env::var_os("RUTIS_E2E_ARTIFACTS").expect(
        "RUTIS_E2E_ARTIFACTS: the directory package.yml or tools/install/local-artifacts.sh made",
    ))
}

/// The packages from the registries (after a release), not the artifacts.
fn from_registry() -> bool {
    std::env::var("RUTIS_E2E_SOURCE").is_ok_and(|source| source == "registry")
}

/// The version under test: `RUTIS_E2E_VERSION`, else this checkout's.
fn version() -> String {
    std::env::var("RUTIS_E2E_VERSION").unwrap_or_else(|_| {
        let manifest: Value =
            serde_json::from_str(include_str!("../../../node/rutis-host/package.json")).unwrap();
        manifest["version"].as_str().unwrap().to_owned()
    })
}

/// The one file in `dir` whose name starts with `prefix` and ends with
/// `suffix`.
fn artifact(dir: &str, prefix: &str, suffix: &str) -> PathBuf {
    let dir = artifacts().join(dir);
    let found: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().unwrap().to_string_lossy();
            name.starts_with(prefix) && name.ends_with(suffix)
        })
        .collect();
    assert_eq!(
        found.len(),
        1,
        "{prefix}*{suffix} in {}: {found:?}",
        dir.display()
    );
    found.into_iter().next().unwrap()
}

/// The npm tarball of `@arcships/<package>` at this version.
fn npm_tarball(package: &str) -> PathBuf {
    let file = artifacts()
        .join("npm")
        .join(format!("arcships-{package}-{}.tgz", version()));
    assert!(
        file.exists(),
        "{} is not among the artifacts",
        file.display()
    );
    file
}

/// The npm platform of this machine (`darwin-arm64`, `linux-x64`, ...).
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

/// The Python on PATH: `python3`, `python` on Windows.
fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

fn npm() -> &'static str {
    if cfg!(windows) {
        "npm.cmd"
    } else {
        "npm"
    }
}

/// Run a program to its end; it must succeed.
fn run(s: &Scenario, dir: &str, program: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Host {
    let what = format!("{} {}", program.as_ref().to_string_lossy(), args.join(" "));
    let mut command = s.program(dir, program, args, &[]);
    let status = command.wait_exit_within(build_guard());
    if !status.success() {
        let output: Vec<String> = command.lines().into_iter().map(|line| line.text).collect();
        panic!("{what}: {status}\n  {}", output.join("\n  "));
    }
    command
}

const GREETER_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

export default definePlugin({
  provides: { greeter: { hello: 'sync' } },
  apply(ctx) {
    ctx.provide('greeter', { hello: (name: string) => `Hello, ${name}` })
  },
})
"#;

/// The greeter in Bun: it says which runtime runs it.
#[cfg(unix)]
const GREETER_BUN_TS: &str = r#"import { definePlugin } from '@arcships/rutis'

const runtime = (globalThis as { Bun?: unknown }).Bun === undefined ? 'not bun' : 'bun'

export default definePlugin({
  provides: { greeter: { hello: 'sync' } },
  apply(ctx) {
    ctx.provide('greeter', { hello: (name: string) => `Hello, ${name} (${runtime})` })
  },
})
"#;

/// A Go binary with the plugin `gogreeter`.
const GREETER_GO: &str = r#"package main

import "github.com/arcships/rutis/go/rutis"

type Greeter struct{}

func (*Greeter) Hello(name string) string { return "Hello, " + name + " (go)" }

var plugin = rutis.Define(rutis.Plugin[struct{}]{
	Name:     "gogreeter",
	Inject:   []string{},
	Provides: rutis.Provides{"gogreeter": rutis.MethodsOf[*Greeter](rutis.Sync("Hello"))},
	Apply: func(ctx *rutis.Ctx, _ struct{}) error {
		ctx.Provide("gogreeter", &Greeter{})
		return nil
	},
})

func main() { rutis.Serve(plugin) }
"#;

const GREETER_PY: &str = r#"from rutis import define_plugin


class Greeter:
    def hello(self, name):
        return f"Hello, {name}"


def apply(ctx, config):
    ctx.provide("greeter", Greeter())


plugin = define_plugin(apply, provides={"greeter": Greeter})
"#;

/// `--version` names this build's version.
fn identity(s: &Scenario, dir: &str, host: &Path) {
    let mut version = run(s, dir, host, &["--version"]);
    version.expect(&format!("rutis-host {}", self::version()));
}

/// What the probe calls in a minimal project, and the answer: the greeter.
const GREET: &[(&str, &str)] = &[("greeter", "Hello, Ada")];

/// Run the project in `dir` with `rutis-host run` (no arguments, as the
/// guides do), make the probe's `calls` (`<service>.hello("Ada")` and the
/// answer), stop as a process manager does ([`stop`]): exit code 0, the
/// probe stopped.
fn run_project(
    s: &Scenario,
    dir: &str,
    host: &Path,
    probe: &rutis_e2e::Probe,
    rows: &[&str],
    calls: &[(&str, &str)],
) {
    run_project_until(s, dir, host, probe, rows, calls, stop);
}

/// [`run_project`], stopped with Ctrl-C as a terminal sends it: SIGINT to
/// the whole process group, the launcher and rutis-host both. One Ctrl-C
/// must stay one (#267): the cleanups run, exit code 0.
#[cfg(unix)]
fn run_project_to_ctrl_c(
    s: &Scenario,
    dir: &str,
    host: &Path,
    probe: &rutis_e2e::Probe,
    rows: &[&str],
) {
    run_project_until(s, dir, host, probe, rows, GREET, |host| {
        host.ctrl_c();
        let status = host.wait_exit();
        host.wait_output_end();
        assert_eq!(status.code(), Some(0), "after Ctrl-C: {status}");
    });
}

fn run_project_until(
    s: &Scenario,
    dir: &str,
    host: &Path,
    probe: &rutis_e2e::Probe,
    rows: &[&str],
    calls: &[(&str, &str)],
    stop: impl FnOnce(&mut Host),
) {
    let mut running = s.program(dir, host, ["run"], &[]);
    // The whole row name: `greeter: running` is inside `gogreeter: running`,
    // and whichever row starts first would take the other's line.
    for row in rows {
        let line = format!("{row}: running");
        running.wait_for(&format!("the line {line:?}"), |text| {
            text.starts_with(&line)
        });
    }
    probe.started(&mut running);
    for (service, answer) in calls {
        assert_eq!(
            probe.call(&mut running, service, "hello", json!(["Ada"])),
            Ok(json!(answer)),
            "{service}.hello"
        );
    }
    stop(&mut running);
    probe.event(&mut running, "stopped");
}

/// Stop as a process manager does, and wait for the output to end: exit
/// code 0. Unix: SIGTERM to the main process alone (npm's launcher, uv,
/// rutis-host), which passes it on. Windows has no SIGTERM: Ctrl-Break to
/// the main process's console process group ([`Host::ctrl_c`]), as a
/// console's stop reaches every process on it; npm's launchers (its .cmd
/// and the Node launcher) wait for rutis-host, which stops on it. For
/// `uv run`, see [`stop_uv`].
fn stop(host: &mut Host) {
    #[cfg(unix)]
    host.signal(libc_sigterm());
    #[cfg(windows)]
    host.ctrl_c();
    let status = host.wait_exit();
    host.wait_output_end();
    assert_eq!(status.code(), Some(0), "after stopping: {status}");
}

/// [`stop`] with `uv run` as the main process. On Windows uv ignores Ctrl-C
/// while its command runs, but not Ctrl-Break (uv-command-support's
/// `run_to_completion` waits on `tokio::signal::ctrl_c` only): uv ends at
/// once with STATUS_CONTROL_C_EXIT, and its exit code is not rutis-host's.
/// That one step is checked differently there: rutis-host stopped on the
/// Ctrl-Break (its line; the caller checks the probe's stop) and has exited
/// (the output ended: nothing holds it any more).
fn stop_uv(host: &mut Host) {
    #[cfg(unix)]
    stop(host);
    #[cfg(windows)]
    {
        /// STATUS_CONTROL_C_EXIT: ended by the console's default handler.
        const CONTROL_C_EXIT: i32 = 0xC000_013Au32 as i32;
        host.ctrl_c();
        let status = host.wait_exit();
        host.wait_output_end();
        assert!(
            matches!(status.code(), Some(0 | CONTROL_C_EXIT)),
            "uv after stopping: {status}"
        );
        host.expect("rutis-host: Ctrl-Break: stopping");
    }
}

#[cfg(unix)]
fn libc_sigterm() -> i32 {
    15
}

/// risk: B1, B6 (npm)
#[test]
#[ignore = "needs built packages: install.yml runs it with RUTIS_E2E_ARTIFACTS"]
fn npm_installs_and_runs_this_build() {
    let _ = IGNORED;
    let s = Scenario::installed("install-npm");
    let version = version();
    let platform = npm_platform();
    // Its own directory: the check without a platform package below must
    // not find this one's up the tree.
    s.write_json(
        "npm/package.json",
        &json!({ "name": "install-npm", "private": true, "type": "module" }),
    );
    if from_registry() {
        run(
            &s,
            "npm",
            npm(),
            &[
                "install",
                &format!("@arcships/rutis-host@{version}"),
                &format!("@arcships/rutis@{version}"),
            ],
        );
    } else {
        // From the files: this platform's package by name, the others left
        // out (they are on no registry yet for an unreleased build).
        let files = [
            npm_tarball("rutis-host"),
            npm_tarball(&format!("rutis-host-{platform}")),
            npm_tarball("rutis-runtime"),
            npm_tarball("rutis"),
        ];
        let files: Vec<String> = files
            .iter()
            .map(|file| file.display().to_string())
            .collect();
        let mut args = vec!["install", "--omit=optional"];
        args.extend(files.iter().map(String::as_str));
        run(&s, "npm", npm(), &args);
        // This build's packages, not a registry's.
        let lock: Value = serde_json::from_str(
            &std::fs::read_to_string(s.path("npm/package-lock.json")).unwrap(),
        )
        .unwrap();
        for package in [
            "rutis-host",
            &format!("rutis-host-{platform}"),
            "rutis-runtime",
            "rutis",
        ] {
            let resolved = lock["packages"][format!("node_modules/@arcships/{package}")]
                ["resolved"]
                .as_str()
                .unwrap_or_default();
            assert!(
                resolved.starts_with("file:"),
                "@arcships/{package} came from {resolved:?}"
            );
        }
    }
    // Only this platform's binary.
    let platforms: Vec<String> = std::fs::read_dir(s.path("npm/node_modules/@arcships"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("rutis-host-"))
        .collect();
    assert_eq!(platforms, [format!("rutis-host-{platform}")]);
    let launcher = s.path(if cfg!(windows) {
        "npm/node_modules/.bin/rutis-host.cmd"
    } else {
        "npm/node_modules/.bin/rutis-host"
    });
    identity(&s, "npm", &launcher);

    // A minimal project; the Node runtime is the one @arcships/rutis-host
    // brings (the project installs none of its own).
    s.write("npm/greeter.ts", GREETER_TS);
    let probe = s.probe_in("npm", "probe", Lang::Node, &["greeter"]);
    s.write_json(
        "npm/rutis.json",
        &json!({
            "runtimes": { "node": {} },
            "rows": [{ "id": "greeter", "name": "./greeter.ts" }, probe.row()],
        }),
    );
    run_project(&s, "npm", &launcher, &probe, &["greeter"], GREET);
    // The launcher in a terminal (#267).
    #[cfg(unix)]
    run_project_to_ctrl_c(&s, "npm", &launcher, &probe, &["greeter"]);

    // The greeter in Bun, its runtime this build's @arcships/rutis-bun and
    // the `bun` on PATH.
    #[cfg(unix)]
    {
        // From a registry this platform's package is an optional
        // dependency of @arcships/rutis-host: `--omit=optional` would remove
        // it (#363). From the files it was installed by name.
        match from_registry() {
            true => {
                let bun = format!("@arcships/rutis-bun@{version}");
                run(&s, "npm", npm(), &["install", &bun]);
            }
            false => {
                let bun = npm_tarball("rutis-bun").display().to_string();
                run(&s, "npm", npm(), &["install", "--omit=optional", &bun]);
            }
        }
        run(&s, "npm", "bun", &["--version"]);
        s.write("npm/greeter-bun.ts", GREETER_BUN_TS);
        s.write_json(
            "npm/rutis.json",
            &json!({
                "runtimes": { "node": {}, "bun": {} },
                "rows": [{ "id": "greeter", "name": "bun:./greeter-bun.ts" }, probe.row()],
            }),
        );
        run_project(
            &s,
            "npm",
            &launcher,
            &probe,
            &["greeter"],
            &[("greeter", "Hello, Ada (bun)")],
        );
    }
    #[cfg(windows)]
    eprintln!("install-npm: no Bun row: Bun is not declared for Windows (Bun design §10)");

    // B6: without this platform's package, the launcher says so.
    if !from_registry() {
        s.write_json(
            "bare/package.json",
            &json!({ "name": "bare", "private": true }),
        );
        let host = npm_tarball("rutis-host").display().to_string();
        let runtime = npm_tarball("rutis-runtime").display().to_string();
        run(
            &s,
            "bare",
            npm(),
            &["install", "--omit=optional", &host, &runtime],
        );
        let bare = s.path(if cfg!(windows) {
            "bare/node_modules/.bin/rutis-host.cmd"
        } else {
            "bare/node_modules/.bin/rutis-host"
        });
        let mut missing = s.program("bare", &bare, ["--version"], &[]);
        assert_eq!(missing.wait_exit().code(), Some(1));
        missing.expect(&format!("no binary for {platform}"));
    }
    s.finish();
}

/// The Python of a venv.
fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts/python.exe")
    } else {
        venv.join("bin/python")
    }
}

fn venv_program(venv: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join(format!("{name}.exe"))
    } else {
        venv.join("bin").join(name)
    }
}

/// risk: B1, B6 (PyPI, with uv as the guides do, and with pip)
/// guarantee: host/exit-codes
#[test]
#[ignore = "needs built packages: install.yml runs it with RUTIS_E2E_ARTIFACTS"]
fn pypi_installs_and_runs_this_build() {
    let s = Scenario::installed("install-pypi");
    let version = version();
    let wheels = artifacts().join("wheels").display().to_string();
    let source: Vec<String> = match from_registry() {
        true => vec![],
        // Only the artifacts: this build, never PyPI's.
        false => vec!["--no-index".into(), "--find-links".into(), wheels.clone()],
    };
    let source: Vec<&str> = source.iter().map(String::as_str).collect();
    let requirement = format!("rutis-host=={version}");

    // uv, as the Python guide does: a project with rutis-host as a dev
    // dependency, run with `uv run`.
    std::fs::create_dir_all(s.path("uv")).unwrap();
    run(
        &s,
        "uv",
        "uv",
        &["init", "--bare", "--no-workspace", "--name", "install-uv"],
    );
    let mut add = vec!["add", "--dev"];
    add.extend(&source);
    add.push(&requirement);
    run(&s, "uv", "uv", &add);
    let venv = s.path("uv/.venv");
    // This build: the package that runs is the venv's, at this version.
    let mut where_ = run(
        &s,
        "uv",
        venv_python(&venv),
        &["-c", "import importlib.metadata, rutis; print(importlib.metadata.version('rutis'), rutis.__file__)"],
    );
    let line = where_.expect(&version);
    assert!(
        line.contains(&venv.display().to_string()) || line.contains(".venv"),
        "{line}"
    );
    identity(&s, "uv", &venv_program(&venv, "rutis-host"));
    s.write("uv/greeter.py", GREETER_PY);
    let probe = s.probe_in("uv", "probe", Lang::Python, &["greeter"]);
    s.write_json(
        "uv/rutis.json",
        &json!({
            "runtimes": { "py": {} },
            "rows": [{ "id": "greeter", "name": "py:greeter" }, probe.row()],
        }),
    );
    // `uv run rutis-host run`: the uv process is the main process.
    let mut running = s.program("uv", "uv", ["run", "--no-sync", "rutis-host", "run"], &[]);
    running.expect("greeter: running");
    probe.started(&mut running);
    assert_eq!(
        probe.call(&mut running, "greeter", "hello", json!(["Ada"])),
        Ok(json!("Hello, Ada"))
    );
    stop_uv(&mut running);
    probe.event(&mut running, "stopped");

    // pip in a plain venv.
    std::fs::create_dir_all(s.path("pip")).unwrap();
    run(&s, "pip", python(), &["-m", "venv", ".venv"]);
    let venv = s.path("pip/.venv");
    let mut install = vec!["-m", "pip", "install"];
    install.extend(&source);
    install.push(&requirement);
    run(&s, "pip", venv_python(&venv), &install);
    identity(&s, "pip", &venv_program(&venv, "rutis-host"));
    // The wheel holds what the runtime imports.
    run(
        &s,
        "pip",
        venv_python(&venv),
        &["-c", "import rutis.peer, rutis.runner, rutis.channel"],
    );

    // rutis[network] at its lowest websockets (15.0, from PyPI); rutis
    // stays this build's.
    let network = format!("rutis[network]=={version}");
    let mut install = vec!["-m", "pip", "install"];
    if !from_registry() {
        install.extend(["--find-links", &wheels]);
    }
    install.extend([network.as_str(), "websockets==15.0"]);
    run(&s, "pip", venv_python(&venv), &install);
    let mut versions = run(
        &s,
        "pip",
        venv_python(&venv),
        &[
            "-c",
            "import importlib.metadata as m; print('versions:', m.version('websockets'), m.version('rutis'))",
        ],
    );
    versions.expect(&format!("versions: 15.0 {version}"));
    listen(&s, &venv);

    // B6: an interpreter without the rutis package: the host says how to
    // install it.
    s.write("bare/greeter.py", GREETER_PY);
    std::fs::create_dir_all(s.path("bare")).unwrap();
    run(
        &s,
        "bare",
        python(),
        &["-m", "venv", "--without-pip", "empty"],
    );
    s.write_json(
        "bare/rutis.json",
        &json!({
            "runtimes": { "py": { "python": venv_python(&s.path("bare/empty")) } },
            "rows": [{ "id": "greeter", "name": "py:greeter" }],
        }),
    );
    let mut missing = s.program("bare", venv_program(&venv, "rutis-host"), ["run"], &[]);
    assert_eq!(missing.wait_exit().code(), Some(1));
    missing.expect("pip install rutis");
    s.finish();
}

/// risk: B1, B6 (the release binary)
#[test]
#[ignore = "needs built packages: install.yml runs it with RUTIS_E2E_ARTIFACTS"]
fn the_release_binary_runs_this_build() {
    let s = Scenario::installed("install-binary");
    let version = version();
    let archive = artifact(
        "binary",
        &format!("rutis-host-{version}-"),
        if cfg!(windows) { ".zip" } else { ".tar.gz" },
    );
    // The archive matches its checksum.
    let sum =
        std::fs::read_to_string(format!("{}.sha256", archive.display())).expect("the .sha256");
    let expected = sum.split_whitespace().next().unwrap().to_owned();
    std::fs::create_dir_all(s.path("bin")).unwrap();
    let mut actual = s.program(
        "bin",
        if cfg!(windows) { "certutil" } else { "shasum" },
        if cfg!(windows) {
            vec![
                "-hashfile".to_owned(),
                archive.display().to_string(),
                "SHA256".into(),
            ]
        } else {
            vec!["-a".into(), "256".into(), archive.display().to_string()]
        },
        &[],
    );
    assert!(actual.wait_exit().success());
    actual.expect(&expected);
    // Unpacked.
    // Windows' own tar (bsdtar) opens a .zip; the GNU tar on a bash PATH
    // does not, and takes `D:` for a remote host.
    let unpack = match cfg!(windows) {
        true => {
            let tar = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
                .join("System32/tar.exe");
            run(&s, "bin", tar, &["-xf", &archive.display().to_string()])
        }
        false => run(&s, "bin", "tar", &["-xzf", &archive.display().to_string()]),
    };
    drop(unpack);
    let name = archive
        .file_name()
        .unwrap()
        .to_string_lossy()
        .replace(".tar.gz", "")
        .replace(".zip", "");
    let host = s.path("bin").join(&name).join(if cfg!(windows) {
        "rutis-host.exe"
    } else {
        "rutis-host"
    });
    for file in ["README.md", "LICENSE"] {
        assert!(
            s.path("bin").join(&name).join(file).exists(),
            "{file} in the archive"
        );
    }
    identity(&s, "", &host);

    // B6: a Node row with no Node runtime installed: the host says how.
    s.write("bare/greeter.ts", GREETER_TS);
    s.write_json(
        "bare/package.json",
        &json!({ "name": "bare", "private": true, "type": "module" }),
    );
    s.write_json(
        "bare/rutis.json",
        &json!({ "runtimes": { "node": {} }, "rows": [{ "id": "greeter", "name": "./greeter.ts" }] }),
    );
    let mut missing = s.program("bare", &host, ["run"], &[]);
    assert_eq!(missing.wait_exit().code(), Some(1));
    missing.expect("npm install @arcships/rutis-runtime");

    // A project with a TS and a Python row, their runtimes installed into
    // the project as the deployment guide says: the TS probe calls the
    // Python greeter.
    s.write_json(
        "app/package.json",
        &json!({ "name": "app", "private": true, "type": "module" }),
    );
    if from_registry() {
        run(
            &s,
            "app",
            npm(),
            &[
                "install",
                &format!("@arcships/rutis-runtime@{version}"),
                &format!("@arcships/rutis@{version}"),
            ],
        );
    } else {
        let runtime = npm_tarball("rutis-runtime").display().to_string();
        let sdk = npm_tarball("rutis").display().to_string();
        run(&s, "app", npm(), &["install", &runtime, &sdk]);
    }
    run(&s, "app", python(), &["-m", "venv", ".venv"]);
    let rutis = format!("rutis=={version}");
    let wheels = artifacts().join("wheels").display().to_string();
    let mut pip = vec!["-m", "pip", "install"];
    if !from_registry() {
        pip.extend(["--no-index", "--find-links", &wheels]);
    }
    pip.push(&rutis);
    run(&s, "app", venv_python(&s.path("app/.venv")), &pip);
    s.write("app/greeter.py", GREETER_PY);
    go_build(&s, "app/go-src", "app/plugins/go");
    let probe = s.probe_in("app", "probe", Lang::Node, &["greeter", "gogreeter"]);
    s.write_json(
        "app/rutis.json",
        &json!({
            "runtimes": { "node": {}, "py": {}, "go": { "dir": "plugins/go" } },
            "rows": [
                { "id": "greeter", "name": "py:greeter" },
                { "id": "gogreeter", "name": "go:gogreeter" },
                probe.row(),
            ],
        }),
    );
    run_project(
        &s,
        "app",
        &host,
        &probe,
        &["greeter", "gogreeter"],
        &[("greeter", "Hello, Ada"), ("gogreeter", "Hello, Ada (go)")],
    );
    s.finish();
}

/// Build [`GREETER_GO`] in `src` into the directory `out`, as a plugin
/// author does, with the Go on PATH: against this checkout's Go SDK (a
/// `replace`: an unreleased version's module is on no proxy), or with
/// `RUTIS_E2E_SOURCE=registry` the published module at this version. The
/// build cache is the caller's (`go env GOCACHE`): a cache, no package,
/// and a cold one builds the standard library again.
fn go_build(s: &Scenario, src: &str, out: &str) {
    let mut module = format!(
        "module example.com/gogreeter\n\ngo 1.24\n\nrequire github.com/arcships/rutis/go/rutis v{}\n",
        version()
    );
    if !from_registry() {
        // Without the `\\?\` of a canonical Windows path, with `/`.
        let sdk = repo().join("go/rutis").display().to_string();
        let sdk = sdk.strip_prefix(r"\\?\").unwrap_or(&sdk).replace('\\', "/");
        module.push_str(&format!(
            "\nreplace github.com/arcships/rutis/go/rutis => {sdk}\n"
        ));
    }
    s.write(format!("{src}/go.mod"), module);
    s.write(format!("{src}/main.go"), GREETER_GO);
    let cache = std::process::Command::new("go")
        .args(["env", "GOCACHE"])
        .output()
        .expect("go env GOCACHE");
    let cache = String::from_utf8_lossy(&cache.stdout).trim().to_owned();
    let binary = s.path(out).join(if cfg!(windows) {
        "gogreeter.exe"
    } else {
        "gogreeter"
    });
    let binary = binary.display().to_string();
    let mut env = vec![
        ("GOFLAGS", "-mod=mod"),
        ("GOTOOLCHAIN", "local"),
        ("GOCACHE", cache.as_str()),
    ];
    if !from_registry() {
        env.push(("GOPROXY", "off"));
    }
    let mut build = s.program(src, "go", ["build", "-o", &binary, "."], &env);
    let status = build.wait_exit_within(build_guard());
    if !status.success() {
        let output: Vec<String> = build.lines().into_iter().map(|line| line.text).collect();
        panic!("go build: {status}\n  {}", output.join("\n  "));
    }
}

/// Node B of `docs/guide/nodes.md` from what PyPI installed in `venv`
/// (`rutis[network]`): `python -m rutis listen:ws://…` serving the plugin
/// `greeter`, which this venv's rutis-host runs as a remote runtime's row;
/// the probe calls it through the link.
fn listen(s: &Scenario, venv: &Path) {
    let token = s.secret();
    s.write("node/greeter.py", GREETER_PY);
    let mut node = s.program(
        "pip",
        venv_python(venv),
        [
            "-m".to_owned(),
            "rutis".into(),
            "listen:ws://127.0.0.1:0/rutis".into(),
            "--id".into(),
            "gpu".into(),
            "--peer".into(),
            "main".into(),
            s.path("node").display().to_string(),
        ],
        &[("RUTIS_TOKEN", token.as_str())],
    );
    let (url, _) = s.announced(&mut node);
    let probe = s.probe_in("pip", "probe-ws", Lang::Python, &["greeter"]);
    s.write_json(
        "pip/rutis.json",
        &json!({
            "id": "main",
            "runtimes": {
                "py": { "python": venv_python(venv) },
            },
            "rows": [
                {
                    "id": "gpu",
                    "name": "rutis-bridge/peer",
                    "config": { "address": url },
                },
                { "id": "greeter", "name": "gpu:greeter" },
                probe.row(),
            ],
        }),
    );
    let mut running = s.program(
        "pip",
        venv_program(venv, "rutis-host"),
        ["run"],
        &[("RUTIS_TOKEN_GPU", token.as_str())],
    );
    running.expect("greeter: running");
    probe.started(&mut running);
    assert_eq!(
        probe.call(&mut running, "greeter", "hello", json!(["Ada"])),
        Ok(json!("Hello, Ada"))
    );
    stop(&mut running);
    probe.event(&mut running, "stopped");
    node.kill();
    node.wait_exit();
    node.wait_output_end();
}
