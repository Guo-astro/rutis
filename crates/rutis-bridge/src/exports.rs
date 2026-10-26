//! Rust services used by name from the plugins of other languages.
//!
//! A type marked `#[rutis::service]` is served by name when it is provided
//! (`crate::marked` serves it); its TypeScript declarations are generated
//! from the source ([`build::Exports`](crate::build::Exports), feature
//! `exports`). What is here: [`failure`], for a hand-written
//! [`HostDispatch`](crate::session::HostDispatch) to throw an error as a
//! marked method's `Err(e)` is thrown, and what rutis-loader reads of the
//! marked services.

use serde_json::{json, Value as Json};

use crate::session::Error;

#[doc(hidden)]
pub use crate::marked::{claimed_in, claims_in, install, on_claim, unexported};

/// The error a method's `Err(e)` becomes: thrown in the caller's language
/// (a JavaScript `Error`, a Python `RemoteError`, a Go `*RemoteError`),
/// named `name`, with `code`, the message `e`'s `Display` and `data`, `e`'s
/// serde value when it has one. Read back in Rust with [`Error::code`] and
/// [`Error::data`].
pub fn failure(name: &str, code: &str, message: String, data: Option<Json>) -> Error {
    let mut nodes = Vec::new();
    nodes.push(Json::Null);
    let mut root = serde_json::Map::new();
    root.insert("type".into(), json!("error"));
    root.insert("name".into(), json!(name));
    root.insert("message".into(), json!(message));
    root.insert("code".into(), json!(code));
    if let Some(data) = data {
        let data = graph_value(data, &mut nodes);
        root.insert("data".into(), data);
    }
    nodes[0] = Json::Object(root);
    Error::Remote {
        name: name.to_owned(),
        message,
        graph: Some(json!({ "root": { "type": "reference", "value": 0 }, "nodes": nodes })),
    }
}

/// `error`, a method's `Err` as [`failure`] made it, with the stack
/// `Name: message` and one frame naming `method` and where it is declared
/// (the diagnostics design §4.4). A stack already there is kept.
pub fn located(error: Error, method: &str, file: &str, line: u32) -> Error {
    let Error::Remote {
        name,
        message,
        graph: Some(mut graph),
    } = error
    else {
        return error;
    };
    let index = graph["root"]["value"].as_u64().unwrap_or(0) as usize;
    if let Some(node) = graph["nodes"]
        .get_mut(index)
        .and_then(Json::as_object_mut)
        .filter(|node| !node.contains_key("stack"))
    {
        node.insert(
            "stack".into(),
            json!(format!(
                "{name}: {message}\n    at {method} ({file}:{line})"
            )),
        );
    }
    Error::Remote {
        name,
        message,
        graph: Some(graph),
    }
}

/// `data` as an error graph value: JSON as it is, unless it holds an
/// integer beyond ±(2^53−1), which crosses as a bigint (design V6), its
/// arrays and objects then as nodes.
fn graph_value(data: Json, nodes: &mut Vec<Json>) -> Json {
    if !crate::session::values::holds_beyond_safe(&data) {
        return json!({ "type": "data", "value": data });
    }
    match data {
        Json::Number(number) => json!({ "type": "bigint", "value": number.to_string() }),
        Json::Array(items) => {
            let id = nodes.len();
            nodes.push(Json::Null);
            let values: Vec<Json> = items
                .into_iter()
                .map(|item| graph_value(item, nodes))
                .collect();
            nodes[id] = json!({ "type": "array", "values": values });
            json!({ "type": "reference", "value": id })
        }
        Json::Object(fields) => {
            let id = nodes.len();
            nodes.push(Json::Null);
            let values: Vec<Json> = fields
                .into_iter()
                .map(|(key, value)| json!([key, graph_value(value, nodes)]))
                .collect();
            nodes[id] = json!({ "type": "object", "values": values });
            json!({ "type": "reference", "value": id })
        }
        other => json!({ "type": "data", "value": other }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_their_code_message_and_data() {
        let error = failure(
            "Problem",
            "Missing",
            "problem Missing".into(),
            Some(json!("Missing")),
        );
        assert_eq!(error.code(), Some("Missing"));
        assert_eq!(error.data(), Some(json!("Missing")));
        assert_eq!(error.to_string(), "Problem: problem Missing");

        // Beyond 2^53 it crosses as a bigint, and reads back exactly.
        let error = failure(
            "Problem",
            "Big",
            "problem Big".into(),
            Some(json!({ "Big": u64::MAX })),
        );
        assert_eq!(error.data(), Some(json!({ "Big": u64::MAX })));
        let Error::Remote { graph, .. } = &error else {
            panic!()
        };
        assert!(graph.as_ref().unwrap().to_string().contains("\"bigint\""));

        let error = failure("Error", "Error", "no".into(), None);
        assert_eq!((error.code(), error.data()), (Some("Error"), None));
        assert_eq!(error.to_string(), "Error: no");
    }
}
