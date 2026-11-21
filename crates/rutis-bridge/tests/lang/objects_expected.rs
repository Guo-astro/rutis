//! The objects expectation file (`objects.expected.json`, #687 §十): each
//! case has a cell for every language, and every case an enforced
//! language runs (`yes`, or `forward` for the host) is run by a test of
//! it, one carrying the marker `objects-case(<language>): <case>`. A
//! language is enforced from the stage that implements it on.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn expected() -> Value {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/runtime/testing/objects.expected.json"),
    )
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Every `objects-case(<language>): <case>` marker in the repository's
/// tracked sources (`git ls-files`: not other checkouts in it), by
/// language.
fn markers() -> BTreeMap<String, BTreeSet<String>> {
    const SOURCES: [&str; 11] = [
        "rs", "mjs", "js", "ts", "py", "go", "swift", "kt", "kts", "cjs", "mts",
    ];
    let listed = std::process::Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .current_dir(repo())
        .output()
        .expect("git lists the repository's files");
    assert!(listed.status.success(), "git ls-files failed");
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for file in String::from_utf8_lossy(&listed.stdout).lines() {
        let path: PathBuf = repo().join(file);
        let source = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| SOURCES.contains(&e));
        if !source {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            let Some(at) = line.find("objects-case(") else {
                continue;
            };
            let rest = &line[at + "objects-case(".len()..];
            let Some((language, case)) = rest.split_once("): ") else {
                continue;
            };
            let case = case.split_whitespace().next().unwrap_or_default();
            // Prose about markers (`objects-case(<language>): …`).
            if !language.chars().all(|c| c.is_ascii_lowercase()) {
                continue;
            }
            found
                .entry(language.to_owned())
                .or_default()
                .insert(case.to_owned());
        }
    }
    found
}

/// guarantee: bridge/object-refs
#[test]
fn every_enforced_case_is_run_by_a_test() {
    let expected = expected();
    let languages: Vec<&str> = expected["languages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    let cases = expected["cases"].as_array().unwrap();
    let ids: BTreeSet<&str> = cases.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), cases.len(), "case ids are unique");
    let mut problems = Vec::new();
    for case in cases {
        for language in &languages {
            let cell = case[language].as_str().unwrap_or("missing");
            if !["yes", "no", "forward"].contains(&cell) {
                problems.push(format!("{}: {language}: {cell}", case["id"]));
            }
        }
    }
    let markers = markers();
    for (language, marked) in &markers {
        assert!(
            languages.contains(&language.as_str()),
            "marker language {language}"
        );
        for case in marked {
            if !ids.contains(case.as_str()) {
                problems.push(format!("objects-case({language}): {case} is no case"));
            }
        }
    }
    for language in expected["enforced"].as_array().unwrap() {
        let language = language.as_str().unwrap();
        let marked = markers.get(language).cloned().unwrap_or_default();
        for case in cases {
            let id = case["id"].as_str().unwrap();
            let runs = matches!(case[language].as_str(), Some("yes" | "forward"));
            if runs && !marked.contains(id) {
                problems.push(format!("{language}: no test runs {id}"));
            }
            if !runs && marked.contains(id) {
                problems.push(format!("{language}: {id} is not this language's"));
            }
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}
