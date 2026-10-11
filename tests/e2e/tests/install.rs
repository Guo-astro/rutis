//! S9, installed and run (#193): rutis-host as a user gets it, from npm,
//! from PyPI and as the release binary, in a new directory with a new home
//! and none of this checkout's runtimes ([`Scenario::installed`]). Each
//! channel installs, checks that this build is what runs (its version, and
//! where the binary is), runs a minimal project, stops it as a process
//! manager does, and ends with the residue checks. Risk B1 (P0).
//!
//! The packages come from `RUTIS_E2E_ARTIFACTS` (what package.yml builds,
//! or tools/install/local-artifacts.sh on a development machine), or with
//! `RUTIS_E2E_SOURCE=registry` from the registries, at `RUTIS_E2E_VERSION`
//! (after a release). install.yml runs them; `cargo test` skips them:
//!
//!   RUTIS_E2E_ARTIFACTS=<dir> cargo test -p rutis-e2e --test install -- --ignored --test-threads=1

use std::path::{Path, PathBuf};

use rutis_e2e::{Host, Lang, Scenario};
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
    let status = command.wait_exit();
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

/// Run the project in `dir` with `rutis-host run` (no arguments, as the
/// guides do), call the greeter through the probe, stop with SIGTERM to the
/// main process as a process manager does: exit code 0, the probe stopped.
fn run_project(s: &Scenario, dir: &str, host: &Path, probe: &rutis_e2e::Probe, rows: &[&str]) {
    run_project_until(s, dir, host, probe, rows, stop);
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
    run_project_until(s, dir, host, probe, rows, |host| {
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
    stop: impl FnOnce(&mut Host),
) {
    let mut running = s.program(dir, host, ["run"], &[]);
    for row in rows {
        running.expect(&format!("{row}: running"));
    }
    probe.started(&mut running);
    assert_eq!(
        probe.call(&mut running, "greeter", "hello", json!(["Ada"])),
        Ok(json!("Hello, Ada"))
    );
    stop(&mut running);
    #[cfg(unix)]
    probe.event(&mut running, "stopped");
}

fn stop(host: &mut Host) {
    #[cfg(unix)]
    host.signal(libc_sigterm());
    // Windows: no SIGTERM, and Ctrl-C to a console is #232. A service
    // manager ends the whole tree: the main process is a launcher (npm's
    // .cmd, uv) whose rutis-host and runtimes outlive it alone. Killed, the
    // host cleans nothing up, so the callers check its probe's stop on Unix
    // only.
    #[cfg(windows)]
    {
        let status = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &host.pid().to_string()])
            .status()
            .expect("taskkill");
        assert!(status.success(), "taskkill /T: {status}");
    }
    let status = host.wait_exit();
    host.wait_output_end();
    #[cfg(unix)]
    assert_eq!(status.code(), Some(0), "after SIGTERM: {status}");
    #[cfg(windows)]
    let _ = status;
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
    run_project(&s, "npm", &launcher, &probe, &["greeter"]);
    // The launcher in a terminal (#267).
    #[cfg(unix)]
    run_project_to_ctrl_c(&s, "npm", &launcher, &probe, &["greeter"]);

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
    stop(&mut running);
    #[cfg(unix)]
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
    let probe = s.probe_in("app", "probe", Lang::Node, &["greeter"]);
    s.write_json(
        "app/rutis.json",
        &json!({
            "runtimes": { "node": {}, "py": {} },
            "rows": [{ "id": "greeter", "name": "py:greeter" }, probe.row()],
        }),
    );
    run_project(&s, "app", &host, &probe, &["greeter"]);
    s.finish();
}
