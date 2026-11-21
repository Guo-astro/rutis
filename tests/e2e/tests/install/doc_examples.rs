//! The guides' runnable examples (#193, Q2.13): each one marked in
//! docs/guide/*.md (tools/doc-examples.mjs) is followed as written, in a
//! new directory, in installed mode. Swift plugins build only on macOS, so
//! the Swift guides' examples run on macOS and the others elsewhere.
//!
//! Two sources of packages:
//!
//! - after a release, the published ones (the guides install from the
//!   registries): `install.yml` runs it with the registry source;
//! - in a release gate, the packages the gate built, before they are
//!   published: with `RUTIS_E2E_ARTIFACTS` set, the examples that install
//!   only npm and PyPI packages run against a local npm registry over those
//!   tarballs and those wheels (docs/design-function-refs-2026-11-03.md §6).
//!   The others need crates.io or the Go module proxy and stay post-release.
//!
//! `cargo test` skips it:
//!
//!   RUTIS_E2E_SOURCE=registry cargo test -p rutis-e2e --test doc_examples -- --ignored
//!   RUTIS_E2E_ARTIFACTS=<dir> cargo test -p rutis-e2e --test doc_examples -- --ignored

#![cfg(unix)]

use rutis_e2e::{build_guard, repo, Scenario};
use serde_json::Value;

#[test]
#[ignore = "installs packages: a release gate runs it with the ones it built, and after a release with the published ones"]
fn the_guides_examples_run_as_written() {
    let built = std::env::var_os("RUTIS_E2E_ARTIFACTS").map(std::path::PathBuf::from);
    let out = std::env::temp_dir().join(format!("rutis-doc-examples-{}.json", std::process::id()));
    let status = std::process::Command::new("node")
        .arg(repo().join("tools/doc-examples.mjs"))
        .arg("--out")
        .arg(&out)
        .status()
        .expect("node tools/doc-examples.mjs");
    assert!(status.success(), "the examples' markers do not parse");
    let examples: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let _ = std::fs::remove_file(&out);
    for example in examples.as_array().unwrap() {
        let swift = example["guide"].as_str().unwrap().starts_with("swift-");
        if swift != cfg!(target_os = "macos") {
            continue;
        }
        let name = example["name"].as_str().unwrap();
        if built.is_some() && !installs_only_npm_and_pypi(name) {
            continue;
        }
        follow(example, built.as_deref());
    }
}

/// Whether every package an example installs comes from npm or PyPI: those
/// run with the packages a release gate built. The others (`jot`, `rust`,
/// `go`, `swift`) need crates.io or the Go module proxy, and run after a
/// release only. Each guide's English twin adds `-en` to the name.
fn installs_only_npm_and_pypi(name: &str) -> bool {
    const NAMES: [&str; 8] = [
        "jot-command",
        "jot-command-python",
        "plugin-api-ts",
        "plugin-api-py",
        "typescript",
        "bun",
        "python",
        "rutis-host",
    ];
    let name = name.strip_suffix("-en").unwrap_or(name);
    NAMES.contains(&name)
}

fn follow(example: &Value, built: Option<&std::path::Path>) {
    let name = example["name"].as_str().unwrap();
    let s = match built {
        Some(built) => Scenario::installed_with_artifacts(&format!("doc-{name}"), built),
        None => Scenario::installed(&format!("doc-{name}")),
    };
    // The directory the reader is in: changed by `cd` lines.
    let mut cwd = String::new();
    for step in example["steps"].as_array().unwrap() {
        let at = step["where"].as_str().unwrap();
        let body = step["body"].as_str().unwrap();
        if let Some(file) = step["file"].as_str() {
            s.write(file, body);
            continue;
        }
        let lines: Vec<&str> = body
            .lines()
            .map(|line| line.split(" #").next().unwrap().trim())
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect();
        if let Some(until) = step["until"].as_str() {
            let [command] = lines[..] else {
                panic!("{at}: a block with until= has one command");
            };
            // exec: the command is the process group's leader's only child,
            // as in a terminal.
            let mut running = s.program(&cwd, "sh", ["-c", &format!("exec {command}")], &[]);
            running.expect(until);
            running.ctrl_c();
            let status = running.wait_exit();
            running.wait_output_end();
            // rutis-host says so when its cleanups did not finish.
            assert_eq!(
                running.count("did not finish"),
                0,
                "{at}: {command} after Ctrl-C"
            );
            // npm runs the command through sh, and dash (Debian's and
            // Ubuntu's sh) ends itself with the SIGINT once its child has
            // exited, however that exited; npm then ends with it (#270).
            let through_sh = command.starts_with("npx ")
                && std::os::unix::process::ExitStatusExt::signal(&status) == Some(libc::SIGINT);
            assert!(
                status.code() == Some(0) || through_sh,
                "{at}: {command} after Ctrl-C: {status}"
            );
            continue;
        }
        for line in lines {
            if let Some(dir) = line.strip_prefix("cd ") {
                cwd = std::path::Path::new(&cwd)
                    .join(dir.trim())
                    .display()
                    .to_string();
                continue;
            }
            // Setup steps: installs and builds.
            let mut command = s.program(&cwd, "sh", ["-c", line], &[]);
            let status = command.wait_exit_within(build_guard());
            if !status.success() {
                let output: Vec<String> =
                    command.lines().into_iter().map(|line| line.text).collect();
                panic!("{at}: {line}: {status}\n  {}", output.join("\n  "));
            }
        }
    }
    s.finish();
}
