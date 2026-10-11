//! The guides' runnable examples (#193, Q2.13): each one marked in
//! docs/guide/*.md (tools/doc-examples.mjs) is followed as written, in a
//! new directory, with the published packages (the guides install from the
//! registries), in installed mode. install.yml runs this after a release;
//! `cargo test` skips it:
//!
//!   RUTIS_E2E_SOURCE=registry cargo test -p rutis-e2e --test doc_examples -- --ignored

#![cfg(unix)]

use rutis_e2e::{repo, Scenario};
use serde_json::Value;

#[test]
#[ignore = "installs the published packages: install.yml runs it after a release"]
fn the_guides_examples_run_as_written() {
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
        follow(example);
    }
}

fn follow(example: &Value) {
    let name = example["name"].as_str().unwrap();
    let s = Scenario::installed(&format!("doc-{name}"));
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
            assert_eq!(
                status.code(),
                Some(0),
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
            let mut command = s.program(&cwd, "sh", ["-c", line], &[]);
            let status = command.wait_exit();
            if !status.success() {
                let output: Vec<String> =
                    command.lines().into_iter().map(|line| line.text).collect();
                panic!("{at}: {line}: {status}\n  {}", output.join("\n  "));
            }
        }
    }
    s.finish();
}
