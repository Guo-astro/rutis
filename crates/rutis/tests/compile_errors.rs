//! What a plugin author sees when a Rust plugin is written wrongly (#680,
//! docs/design-diagnostics-2026-10-26.md §4.5): the first line and label of
//! each compile error say, in the author's terms, what is wrong and how to
//! fix it, and never name the macro's or the trait's internals.
//!
//! Each program in `tests/compile_errors/` is a bin of one scratch crate
//! that depends on this rutis by path; `cargo check --message-format=json`
//! compiles them all, and the test reads each error's message, primary
//! span and label. Only text rutis writes (its `on_unimplemented`
//! messages) is matched exactly; rustc's own wording is matched by a few
//! stable words, so a new stable rustc does not break the test. The scratch
//! crate is built under `CARGO_TARGET_TMPDIR` and kept, so only the first
//! run compiles the dependencies.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Names that must never be in an error's first line or primary label.
const INTERNAL: &[&str] = &[
    "Param<",
    "ViaFunction",
    "ViaData",
    "FromFunction",
    "Failure<",
    "FailSerde",
    "FailDisplay",
    "FailDebug",
    "Throwable",
    "CtxInternal",
    "require_provided_as",
    "IntoEvent",
    "FiberStatusChanged",
    "Served",
    "Parameter",
    "__rutis",
];

/// Each program, the first line its error starts with, and where the
/// primary span is (line in the program, text of the label).
struct Case {
    bin: &'static str,
    message: Expect,
    line: usize,
    label: Expect,
    /// Text that must be somewhere in the rendered error (its notes).
    rendered: &'static [&'static str],
    /// Text that must not be anywhere in the rendered error.
    absent: &'static [&'static str],
}

enum Expect {
    /// rutis's own text, exactly.
    Exactly(&'static str),
    /// rustc's own text: these words.
    Contains(&'static str),
}

impl Expect {
    fn matches(&self, text: &str) -> bool {
        match self {
            Expect::Exactly(expected) => text == *expected,
            Expect::Contains(words) => text.contains(words),
        }
    }
}

const CASES: &[Case] = &[
    Case {
        bin: "param_not_data",
        message: Expect::Exactly(
            "`Item` cannot be a parameter of a `#[rutis::service]` method: other languages cannot send it",
        ),
        line: 9,
        label: Expect::Exactly("`Item` does not implement serde `Deserialize`"),
        rendered: &["derive `serde::Deserialize` for `Item`"],
        absent: &[],
    },
    Case {
        bin: "error_not_display",
        message: Expect::Exactly(
            "`Oops` cannot be the error of a `#[rutis::service]` method: it does not implement `Display`",
        ),
        line: 7,
        label: Expect::Exactly("`Oops` does not implement `Display`"),
        rendered: &["implement `std::fmt::Display` for `Oops`"],
        absent: &["Failure<", "FailSerde", "FailDebug"],
    },
    Case {
        bin: "service_not_sync",
        message: Expect::Contains("cannot be shared between threads safely"),
        line: 5,
        label: Expect::Contains("cannot be shared between threads safely"),
        rendered: &[],
        absent: &["CtxInternal", "require_provided_as", "downcast"],
    },
    Case {
        bin: "emit_not_event",
        message: Expect::Exactly("`Ping` is not an event: it does not implement `rutis::Event`"),
        line: 10,
        label: Expect::Exactly("not an event"),
        rendered: &["implement `rutis::Event`"],
        absent: &["FiberStatusChanged", "ServiceChanged"],
    },
    Case {
        bin: "listen_not_event",
        message: Expect::Exactly("`Ping` is not an event: it does not implement `rutis::Event`"),
        line: 10,
        label: Expect::Exactly("not an event"),
        rendered: &["implement `rutis::Event` for `Ping`"],
        absent: &["FiberStatusChanged", "ServiceChanged"],
    },
    Case {
        bin: "deps_not_arc",
        message: Expect::Exactly(
            "`Db` cannot be a plugin's dependency: `type Deps` lists services as `Arc<T>`",
        ),
        line: 11,
        label: Expect::Exactly("not `()`, `Arc<T>`, `Option<Arc<T>>` or a tuple of them"),
        rendered: &["write `Arc<Db>`"],
        absent: &[],
    },
    Case {
        bin: "listener_wrong_shape",
        message: Expect::Contains("is not a future"),
        line: 16,
        label: Expect::Contains("is not a future"),
        rendered: &[],
        absent: &[],
    },
];

/// One error as rustc reported it.
#[derive(Debug)]
struct Reported {
    message: String,
    file: String,
    line: usize,
    label: String,
    rendered: String,
}

/// guarantee: sdk/rust-compile-errors
#[test]
fn author_mistakes_are_reported_in_the_authors_terms() {
    let reported = check_all();
    let mut failures = Vec::new();
    for case in CASES {
        let errors = reported.get(case.bin).map(Vec::as_slice).unwrap_or(&[]);
        if errors.is_empty() {
            failures.push(format!("{}: compiled without an error", case.bin));
            continue;
        }
        for error in errors {
            for name in INTERNAL {
                if error.message.contains(name) || error.label.contains(name) {
                    failures.push(format!(
                        "{}: `{name}` in the first line or label: {} / {}",
                        case.bin, error.message, error.label
                    ));
                }
            }
            for name in case.absent {
                if error.rendered.contains(name) {
                    failures.push(format!(
                        "{}: `{name}` in the error:\n{}",
                        case.bin, error.rendered
                    ));
                }
            }
        }
        let first = &errors[0];
        let expected_file = format!("src/bin/{}.rs", case.bin);
        if !case.message.matches(&first.message)
            || !case.label.matches(&first.label)
            || first.file != expected_file
            || first.line != case.line
        {
            failures.push(format!(
                "{}: expected the first error at line {} of the program; got\n{}",
                case.bin, case.line, first.rendered
            ));
        }
        for text in case.rendered {
            if !first.rendered.contains(text) {
                failures.push(format!(
                    "{}: `{text}` missing from\n{}",
                    case.bin, first.rendered
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Writes the scratch crate, checks every bin, and returns the errors by
/// bin, in the order rustc reported them.
fn check_all() -> BTreeMap<String, Vec<Reported>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("compile-errors");
    let bins = root.join("src/bin");
    std::fs::create_dir_all(&bins).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"compile-errors\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n\
             [dependencies]\nrutis = {{ path = {:?} }}\n\n[workspace]\n",
            manifest.display().to_string()
        ),
    )
    .unwrap();
    // The workspace's versions, so nothing is fetched.
    let lock = manifest.join("../../Cargo.lock");
    if !root.join("Cargo.lock").exists() {
        std::fs::copy(&lock, root.join("Cargo.lock")).unwrap();
    }
    for case in CASES {
        let source = manifest
            .join("tests/compile_errors")
            .join(format!("{}.rs", case.bin));
        let target = bins.join(format!("{}.rs", case.bin));
        let text = std::fs::read_to_string(&source).unwrap();
        if std::fs::read_to_string(&target).ok().as_deref() != Some(&text) {
            std::fs::write(&target, text).unwrap();
        }
    }

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .current_dir(&root)
        .args([
            "check",
            "--bins",
            "--keep-going",
            "--offline",
            "--quiet",
            "--message-format=json",
        ])
        .arg("--target-dir")
        .arg(root.join("target"))
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .output()
        .expect("cargo runs");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut reported: BTreeMap<String, Vec<Reported>> = BTreeMap::new();
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value["reason"] != "compiler-message" {
            continue;
        }
        let bin = value["target"]["name"].as_str().unwrap_or_default();
        let message = &value["message"];
        if message["level"] != "error" || message["spans"].as_array().is_none_or(Vec::is_empty) {
            continue;
        }
        let spans = message["spans"].as_array().unwrap();
        let primary = spans
            .iter()
            .find(|span| span["is_primary"] == true)
            .unwrap_or(&spans[0]);
        reported.entry(bin.to_owned()).or_default().push(Reported {
            message: message["message"].as_str().unwrap_or_default().to_owned(),
            file: primary["file_name"]
                .as_str()
                .unwrap_or_default()
                .replace('\\', "/"),
            line: primary["line_start"].as_u64().unwrap_or_default() as usize,
            label: primary["label"].as_str().unwrap_or_default().to_owned(),
            rendered: message["rendered"].as_str().unwrap_or_default().to_owned(),
        });
    }
    assert!(
        !reported.is_empty(),
        "cargo check reported nothing:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    reported
}
