//! The shared case list `specs/config-schema/cases.json`, run against the
//! Rust implementation the loader and `rutis-host check` use: every case,
//! compared word for word (config schema design §8).

use rutis::ConfigError;
use rutis_loader::config::{check, Options};
use serde_json::Value;

fn cases() -> Vec<Value> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../specs/config-schema/cases.json"
    );
    let text = std::fs::read_to_string(path).expect("the case list");
    let doc: Value = serde_json::from_str(&text).expect("JSON");
    doc["cases"].as_array().expect("cases").clone()
}

/// Change every object and array of `value` in place.
fn scribble(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for child in map.values_mut() {
                scribble(child);
            }
            map.insert("scribbled".into(), Value::Bool(true));
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                scribble(item);
            }
            items.push(Value::Bool(true));
        }
        _ => {}
    }
}

#[test]
fn every_case_of_the_shared_list_passes() {
    let mut failures = Vec::new();
    for case in cases() {
        let name = case["name"].as_str().unwrap();
        let config = case.get("config");
        let mut checked = check(&case["schema"], config, Options::default());
        let warnings: Vec<String> = checked
            .warnings
            .iter()
            .map(|issue| format!("config: {}", issue.message))
            .collect();
        let expected_warnings: Vec<String> =
            serde_json::from_value(case["warnings"].clone()).unwrap();
        if warnings != expected_warnings {
            failures.push(format!(
                "{name}: warnings {warnings:#?}, expected {expected_warnings:#?}"
            ));
        }
        let then = &case["then"];
        if let Some(errors) = then.get("errors") {
            let expected: Vec<String> = serde_json::from_value(errors.clone()).unwrap();
            let lines = ConfigError::new(checked.errors.clone()).lines();
            if lines != expected {
                failures.push(format!("{name}: errors {lines:#?}, expected {expected:#?}"));
            }
            continue;
        }
        if !checked.errors.is_empty() {
            failures.push(format!("{name}: unexpected errors {:#?}", checked.errors));
            continue;
        }
        if checked.config != then["config"] {
            failures.push(format!(
                "{name}: config {}, expected {}",
                checked.config, then["config"]
            ));
        }
        // What is filled is a copy: changing it changes nothing of the
        // next check.
        scribble(&mut checked.config);
        let again = check(&case["schema"], config, Options::default());
        if again.config != then["config"] {
            failures.push(format!("{name}: a second check gave {}", again.config));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Manifests of Go binaries older than 0.18 list every field without
/// `omitempty` as required: `check` ignores `required` there (design §7).
#[test]
fn required_can_be_ignored() {
    let schema = serde_json::json!({
        "type": "object",
        "required": ["city"],
        "properties": { "city": { "type": "string" }, "days": { "type": "integer" } }
    });
    let checked = check(
        &schema,
        Some(&serde_json::json!({ "days": "x" })),
        Options::default().without_required(),
    );
    let lines = ConfigError::new(checked.errors).lines();
    assert_eq!(lines, ["config: days should be an integer, got \"x\""]);
}
