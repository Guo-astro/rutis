//! Event errors across sessions (events protocol §11): the kernel's
//! [`ErrorKind`]s as the `throw`s a far end reads, and back.

use std::sync::Arc;

use rutis::host::ErrorKind;
use rutis::Error as RutisError;
use serde_json::{json, Value as Json};

use crate::session::Error;

/// A `BindingError` with exactly `message` (transport-class failures and
/// refusals, events protocol §11).
pub(crate) fn binding(message: impl Into<String>) -> Error {
    Error::Remote {
        name: "BindingError".into(),
        message: message.into(),
        graph: None,
    }
}

/// What a listener elsewhere gets for the promise of a runtime that waits
/// for its waterfall synchronously (`sync: true`, events protocol §5.4):
/// it settles only once that wait ends, which waits for this listener.
pub(crate) fn promise_of_waiting() -> Error {
    Error::SyncWaitCycle(
        "the rest of the waterfall is a promise of the runtime waiting for it synchronously: \
         it settles only after that wait, which waits for this listener"
            .into(),
    )
}

/// A session error as the kernel carries it; [`to_session`] gives it back
/// as it was.
pub(crate) fn to_cordis(error: Error) -> RutisError {
    RutisError::from(error)
}

/// The session error a far end gets for `error`: an error that came over a
/// session as it came, the kernel's event errors under their names
/// (`AggregateError` with its children in registration order).
pub fn to_session(error: &ErrorKind) -> Error {
    if let ErrorKind::PluginFailed(source) = error {
        if let Some(error) = source.downcast_ref::<Error>() {
            return error.clone();
        }
    }
    // A Rust row's config that does not match (a Rust definition's copy
    // loaded from another language, say): the far end gets the same error
    // as from its own SDK, code `bad-config` with the issues (config schema
    // design §5.2).
    if let Some(config) = rutis::ConfigError::of(error) {
        return config_failure(config);
    }
    if let ErrorKind::ListenersFailed { name, errors } = error {
        return aggregate(name, errors);
    }
    if let ErrorKind::ListenerFailed {
        name,
        listener,
        error,
    } = error
    {
        // The kernel labels a row's listener `row <id>`: the error records
        // that row and the event it was listening to (#677).
        let error = to_session(error);
        return match listener.strip_prefix("row ") {
            Some(row) => error.with_site(&crate::session::Site {
                event: Some(name.clone()),
                ..crate::session::Site::new(row, "listener")
            }),
            None => error,
        };
    }
    if let ErrorKind::SyncEventPanicked(inner) = error {
        return to_session(inner);
    }
    match error.wire_name() {
        Some(name) => Error::Remote {
            name: name.into(),
            message: error.to_string(),
            graph: None,
        },
        None => Error::Remote {
            name: "Error".into(),
            message: error.to_string(),
            graph: None,
        },
    }
}

/// A config error as a far end reads it: a `BindingError` with code
/// `bad-config` and `data: { issues: [{ path, message }] }`.
pub(crate) fn config_failure(config: &rutis::ConfigError) -> Error {
    let issues: Vec<serde_json::Value> = config
        .issues()
        .iter()
        .map(|issue| {
            let path: Vec<serde_json::Value> = issue
                .path
                .iter()
                .map(|segment| match segment {
                    rutis::PathSegment::Key(key) => serde_json::json!(key),
                    rutis::PathSegment::Index(index) => serde_json::json!(index),
                })
                .collect();
            serde_json::json!({ "path": path, "message": issue.message })
        })
        .collect();
    crate::exports::failure(
        "BindingError",
        crate::session::code::BAD_CONFIG,
        config.to_string(),
        Some(serde_json::json!({ "issues": issues })),
    )
}

/// `<ErrorName>: <message>` of an error, as a far end would read it.
fn named(error: &ErrorKind) -> String {
    match to_session(error) {
        Error::Remote { name, message, .. } => format!("{name}: {message}"),
        Error::SyncWaitCycle(message) => format!("SyncWaitCycle: {message}"),
        other => format!("BindingError: {other}"),
    }
}

/// The line a host prints for an event failure its error sink receives
/// (events protocol §5.6, in the words of the diagnostics design §8.1):
/// `<row>: listener of <name> failed: <ErrorName>: <message>` for a row's
/// listener, `listener of <name> in <where> failed: …` for another, then
/// where the error came from; `filter of <name> from <where> failed: …`
/// for a filter; `None` for any other error.
pub fn report_line(error: &ErrorKind) -> Option<String> {
    match error {
        ErrorKind::ListenerFailed {
            name,
            listener,
            error,
        } => {
            let text = crate::report::printable(&named(error));
            let mut line = match listener.strip_prefix("row ") {
                Some(row) => format!(
                    "{}: listener of {name} failed: {text}",
                    crate::report::printable(row)
                ),
                None => format!("listener of {name} in {listener} failed: {text}"),
            };
            for whereabout in crate::report::whereabouts(error) {
                line.push_str("\n    ");
                line.push_str(&whereabout);
            }
            Some(line)
        }
        ErrorKind::FilterFailed {
            name,
            emitter,
            error,
        } => Some(format!(
            "filter of {name} from {emitter} failed: {}",
            crate::report::printable(&named(error))
        )),
        _ => None,
    }
}

/// `AggregateError`: `<n> listeners of <name> failed`, its graph's root an
/// error node whose `errors` are the children's (as JavaScript encodes an
/// `AggregateError`).
fn aggregate(name: &str, errors: &[Arc<ErrorKind>]) -> Error {
    let message = match errors.len() {
        1 => format!("1 listener of {name} failed"),
        n => format!("{n} listeners of {name} failed"),
    };
    let mut nodes: Vec<Json> = vec![Json::Null];
    let mut children = Vec::new();
    for error in errors {
        let error = to_session(error);
        children.push(graft(&error, &mut nodes));
    }
    nodes[0] = json!({
        "type": "error",
        "name": "AggregateError",
        "message": message,
        "errors": children,
    });
    Error::Remote {
        name: "AggregateError".into(),
        message,
        graph: Some(json!({ "root": { "type": "reference", "value": 0 }, "nodes": nodes })),
    }
}

/// `error` as a value of a graph whose nodes are `nodes`: its own graph's
/// nodes appended (their references moved), or a node of its own.
fn graft(error: &Error, nodes: &mut Vec<Json>) -> Json {
    let (name, message, graph) = match error {
        Error::Remote {
            name,
            message,
            graph,
        } => (name.clone(), message.clone(), graph.clone()),
        Error::SyncWaitCycle(message) => ("SyncWaitCycle".to_owned(), message.clone(), None),
        other => ("BindingError".to_owned(), other.to_string(), None),
    };
    if let Some(graph) = graph {
        if let (Some(root), Some(Json::Array(theirs))) = (graph.get("root"), graph.get("nodes")) {
            let base = nodes.len() as u64;
            nodes.extend(theirs.iter().map(|node| moved(node, base)));
            return moved(root, base);
        }
    }
    nodes.push(json!({ "type": "error", "name": name, "message": message }));
    json!({ "type": "reference", "value": nodes.len() - 1 })
}

/// A graph value or node with every reference in it moved up by `base`.
fn moved(value: &Json, base: u64) -> Json {
    match value {
        Json::Object(fields) => {
            if fields.get("type").and_then(Json::as_str) == Some("reference") {
                if let Some(index) = fields.get("value").and_then(Json::as_u64) {
                    return json!({ "type": "reference", "value": index + base });
                }
            }
            if fields.get("type").and_then(Json::as_str) == Some("data") {
                return value.clone();
            }
            Json::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), moved(value, base)))
                    .collect(),
            )
        }
        Json::Array(items) => Json::Array(items.iter().map(|item| moved(item, base)).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// A Rust config error crosses as `bad-config` with its issues.
    #[test]
    fn a_config_error_crosses_with_its_code() {
        let config = rutis::config::check(
            &json!({ "properties": { "a": { "type": "integer" } } }),
            Some(&json!({ "a": "x" })),
        )
        .error()
        .unwrap();
        let error = to_session(&ErrorKind::PluginFailed(Box::new(config)));
        assert_eq!(error.code(), Some("bad-config"));
        assert_eq!(
            error.to_string(),
            "config: a should be an integer, got \"x\""
        );
        assert_eq!(
            error.data(),
            Some(
                json!({ "issues": [{ "path": ["a"], "message": "a should be an integer, got \"x\"" }] })
            )
        );
    }

    /// guarantee: events/hub-errors
    /// An aggregate keeps its children in order, each with its own graph.
    #[test]
    fn aggregates_carry_their_children_in_order() {
        let child = crate::exports::failure("Busy", "Busy", "busy".into(), Some(json!([1])));
        let errors = vec![
            Arc::new(to_cordis(binding("listener 7 of t/x is withdrawn")).into_kind()),
            Arc::new(to_cordis(child).into_kind()),
        ];
        let error = to_session(&ErrorKind::ListenersFailed {
            name: "t/x".into(),
            errors,
        });
        let Error::Remote {
            name,
            message,
            graph,
        } = error
        else {
            panic!("a remote error");
        };
        assert_eq!(name, "AggregateError");
        assert_eq!(message, "2 listeners of t/x failed");
        let graph = graph.unwrap();
        let nodes = graph["nodes"].as_array().unwrap();
        let children = nodes[0]["errors"].as_array().unwrap();
        let first = &nodes[children[0]["value"].as_u64().unwrap() as usize];
        assert_eq!(first["message"], "listener 7 of t/x is withdrawn");
        let second = &nodes[children[1]["value"].as_u64().unwrap() as usize];
        assert_eq!(second["code"], "Busy");
        // The child's data moved with it.
        assert_eq!(second["data"], json!({ "type": "data", "value": [1] }));
    }

    /// A row's listener that fails records the row and the event (#677).
    #[test]
    fn a_listener_error_records_its_row_and_event() {
        let thrown = crate::exports::failure("KeyError", "KeyError", "'region'".into(), None);
        let failed = |listener: &str| ErrorKind::ListenerFailed {
            name: "weather/asked".into(),
            listener: listener.into(),
            error: Arc::new(to_cordis(thrown.clone()).into_kind()),
        };
        let site = to_session(&failed("row weather")).site().unwrap();
        assert_eq!(
            site,
            crate::session::Site {
                event: Some("weather/asked".into()),
                ..crate::session::Site::new("weather", "listener")
            }
        );
        // A listener that is not a row's records nothing.
        assert_eq!(to_session(&failed("plugin printer")).site(), None);
    }

    /// A row's listener that fails prints as the row's line; one failed
    /// listener is `1 listener` (#677, §8.1 row 7).
    #[test]
    fn a_listener_failure_prints_as_its_rows_line() {
        let thrown = crate::exports::failure("KeyError", "KeyError", "'region'".into(), None);
        let failed = |listener: &str| ErrorKind::ListenerFailed {
            name: "weather/asked".into(),
            listener: listener.into(),
            error: Arc::new(to_cordis(thrown.clone()).into_kind()),
        };
        assert_eq!(
            report_line(&failed("row weather")).unwrap(),
            "weather: listener of weather/asked failed: KeyError: 'region'"
        );
        assert_eq!(
            report_line(&failed("plugin printer")).unwrap(),
            "listener of weather/asked in plugin printer failed: KeyError: 'region'"
        );
        let one = to_session(&ErrorKind::ListenersFailed {
            name: "weather/asked".into(),
            errors: vec![Arc::new(failed("row weather"))],
        });
        assert_eq!(
            one.to_string(),
            "AggregateError: 1 listener of weather/asked failed"
        );
    }

    /// Arbitrary data as an error graph holds it, including objects that
    /// themselves look like graph values.
    fn data_json() -> impl Strategy<Value = Json> {
        let leaf = prop_oneof![
            Just(Json::Null),
            any::<bool>().prop_map(Json::Bool),
            any::<i64>().prop_map(|number| Json::Number(number.into())),
            "[ -~]{0,12}".prop_map(Json::String),
            Just(json!({ "type": "reference", "value": 0 })),
            Just(json!([{ "type": "reference", "value": 1 }])),
            Just(json!({ "type": "data", "value": { "type": "reference", "value": 2 } })),
        ];
        leaf.prop_recursive(3, 24, 4, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..=3).prop_map(Json::Array),
                prop::collection::vec(("[a-z]{1,4}", inner), 0..=3)
                    .prop_map(|fields| Json::Object(fields.into_iter().collect())),
            ]
        })
        .boxed()
    }

    /// A value of an error graph: a reference to one of `nodes`, or a leaf.
    fn graph_value(nodes: usize) -> impl Strategy<Value = Json> {
        prop_oneof![
            4 => (0..nodes).prop_map(|index| json!({ "type": "reference", "value": index })),
            4 => data_json().prop_map(|data| json!({ "type": "data", "value": data })),
            1 => any::<i64>()
                .prop_map(|number| json!({ "type": "bigint", "value": number.to_string() })),
            1 => "[0-9]{1,3}".prop_map(|number| json!({ "type": "number", "value": number })),
            1 => Just(json!({ "type": "undefined" })),
        ]
    }

    fn error_node(nodes: usize) -> impl Strategy<Value = Json> {
        (
            "[a-z][a-z0-9-]{0,7}",
            "[ -~]{0,12}",
            prop::option::of(graph_value(nodes)),
            prop::collection::vec(graph_value(nodes), 0..=3),
            prop::option::of(graph_value(nodes)),
        )
            .prop_map(|(name, message, cause, errors, data)| {
                let mut node = serde_json::Map::new();
                node.insert("type".into(), json!("error"));
                node.insert("name".into(), json!(name));
                node.insert("message".into(), json!(message));
                if let Some(cause) = cause {
                    node.insert("cause".into(), cause);
                }
                if !errors.is_empty() {
                    node.insert("errors".into(), Json::Array(errors));
                }
                if let Some(data) = data {
                    node.insert("data".into(), data);
                }
                Json::Object(node)
            })
    }

    fn any_node(nodes: usize) -> impl Strategy<Value = Json> {
        prop_oneof![
            3 => error_node(nodes),
            2 => prop::collection::vec(graph_value(nodes), 0..=3)
                .prop_map(|values| json!({ "type": "array", "values": values })),
            2 => prop::collection::vec(("[a-z]{1,4}", graph_value(nodes)), 0..=3)
                .prop_map(|values| {
                    let values: Vec<Json> = values
                        .into_iter()
                        .map(|(key, value)| json!([key, value]))
                        .collect();
                    json!({ "type": "object", "values": values })
                }),
            1 => data_json().prop_map(|data| json!({ "type": "data", "value": data })),
        ]
    }

    /// An error graph as a far end sends it: `root` a reference to an error
    /// node, `nodes` an array, every reference within it.
    fn error_graph() -> impl Strategy<Value = Json> {
        (1usize..=8).prop_flat_map(|count| {
            (
                error_node(count),
                prop::collection::vec(any_node(count), count - 1),
            )
                .prop_map(|(root, rest)| {
                    let mut nodes = vec![root];
                    nodes.extend(rest);
                    json!({ "root": { "type": "reference", "value": 0 }, "nodes": nodes })
                })
        })
    }

    /// The name, message and graph `graft` takes for `error`.
    fn described(error: &Error) -> (String, String, Option<Json>) {
        match error {
            Error::Remote {
                name,
                message,
                graph,
            } => (name.clone(), message.clone(), graph.clone()),
            Error::SyncWaitCycle(message) => ("SyncWaitCycle".into(), message.clone(), None),
            other => ("BindingError".into(), other.to_string(), None),
        }
    }

    fn a_child_error() -> impl Strategy<Value = Error> {
        prop_oneof![
            4 => (error_graph(), "[ -~]{0,12}").prop_map(|(graph, message)| Error::Remote {
                name: "Child".into(),
                message,
                graph: Some(graph),
            }),
            2 => "[ -~]{0,12}".prop_map(|message| Error::Remote {
                name: "Plain".into(),
                message,
                graph: None,
            }),
            1 => "[ -~]{0,12}".prop_map(Error::SyncWaitCycle),
            1 => "[ -~]{0,12}".prop_map(|message| Error::Binding {
                code: None,
                message,
                data: None,
            }),
            1 => "[ -~]{0,12}".prop_map(|message| crate::exports::failure(
                "Busy",
                "Busy",
                message,
                Some(json!({ "at": [1, 2] })),
            )),
        ]
    }

    /// `input` moved by `base` should give `output`: a reference points at
    /// the same node of the moved graph, and a data node is untouched.
    fn check_moved(input: &Json, output: &Json, base: u64) -> Result<(), String> {
        match input {
            Json::Object(fields) if fields.get("type").and_then(Json::as_str) == Some("data") => {
                (output == input)
                    .then_some(())
                    .ok_or_else(|| format!("a data node changed: {input} became {output}"))
            }
            Json::Object(fields)
                if fields.get("type").and_then(Json::as_str) == Some("reference") =>
            {
                let index = fields
                    .get("value")
                    .and_then(Json::as_u64)
                    .ok_or_else(|| format!("a reference without an index: {input}"))?;
                let moved = json!({ "type": "reference", "value": index + base });
                (output == &moved)
                    .then_some(())
                    .ok_or_else(|| format!("reference {index} moved to {output}, not to {moved}"))
            }
            Json::Object(fields) => {
                let moved = output
                    .as_object()
                    .ok_or_else(|| format!("an object moved to {output}"))?;
                if moved.len() != fields.len() {
                    return Err(format!("an object gained or lost fields: {output}"));
                }
                for (key, value) in fields {
                    let moved = moved
                        .get(key)
                        .ok_or_else(|| format!("field {key} gone from {output}"))?;
                    check_moved(value, moved, base)?;
                }
                Ok(())
            }
            Json::Array(items) => {
                let moved = output
                    .as_array()
                    .ok_or_else(|| format!("an array moved to {output}"))?;
                if moved.len() != items.len() {
                    return Err(format!("an array changed length: {output}"));
                }
                for (item, moved) in items.iter().zip(moved) {
                    check_moved(item, moved, base)?;
                }
                Ok(())
            }
            other => (output == other)
                .then_some(())
                .ok_or_else(|| format!("a leaf {other} became {output}")),
        }
    }

    /// Every reference in `value`, outside data nodes (whose content is
    /// data, not graph).
    fn references(value: &Json, found: &mut Vec<u64>) {
        match value {
            Json::Object(fields) if fields.get("type").and_then(Json::as_str) == Some("data") => {}
            Json::Object(fields)
                if fields.get("type").and_then(Json::as_str) == Some("reference") =>
            {
                if let Some(index) = fields.get("value").and_then(Json::as_u64) {
                    found.push(index);
                }
            }
            Json::Object(fields) => fields.values().for_each(|value| references(value, found)),
            Json::Array(items) => items.iter().for_each(|item| references(item, found)),
            _ => {}
        }
    }

    proptest! {
        /// guarantee: events/hub-errors; EVP-11-1
        ///
        /// Moving a graph value moves every reference by the same base and
        /// leaves every data node as it was; moving by zero changes
        /// nothing, and moving twice is moving by the sum.
        #[test]
        fn moving_a_graph_value_keeps_references_on_their_nodes(
            value in any_node(8),
            first in 0u64..4096,
            second in 0u64..4096,
        ) {
            prop_assert_eq!(moved(&value, 0), value.clone());
            let once = moved(&value, first);
            if let Err(why) = check_moved(&value, &once, first) {
                prop_assert!(false, "{}", why);
            }
            prop_assert_eq!(moved(&once, second), moved(&value, first + second));
        }

        /// guarantee: events/hub-errors; EVP-11-1
        ///
        /// An aggregate grafts each child graph whole: its nodes appended
        /// once, every reference moved by the same base so it still points
        /// at its own child's node, every data node untouched, and the
        /// children's roots in registration order. A child without a graph
        /// (or with one not in its shape) is one error node of its own.
        #[test]
        fn an_aggregate_grafts_every_child_graph_whole(
            name in "[a-z][a-z0-9/-]{0,11}",
            children in prop::collection::vec(a_child_error(), 0..=4),
        ) {
            let errors: Vec<_> = children
                .iter()
                .cloned()
                .map(|error| Arc::new(to_cordis(error).into_kind()))
                .collect();
            let error = to_session(&ErrorKind::ListenersFailed {
                name: name.clone(),
                errors,
            });
            let Error::Remote {
                name: aggregate,
                message,
                graph: Some(graph),
            } = error
            else {
                prop_assert!(false, "an aggregate expected: {error:?}");
                unreachable!()
            };
            prop_assert_eq!(aggregate, "AggregateError");
            prop_assert_eq!(
                message,
                match children.len() {
                    1 => format!("1 listener of {name} failed"),
                    count => format!("{count} listeners of {name} failed"),
                }
            );
            let nodes = graph["nodes"].as_array().expect("the graph's nodes");
            prop_assert_eq!(nodes[0]["type"].as_str(), Some("error"));
            prop_assert_eq!(nodes[0]["name"].as_str(), Some("AggregateError"));
            let roots = nodes[0]["errors"].as_array().expect("the children");
            prop_assert_eq!(roots.len(), children.len());
            let mut next = 1usize;
            for (child, root) in children.iter().zip(roots) {
                let (child_name, child_message, child_graph) = described(child);
                let in_its_shape = child_graph.as_ref().is_some_and(|graph| {
                    graph
                        .get("root")
                        .and_then(|root| root.get("value"))
                        .and_then(Json::as_u64)
                        .is_some()
                        && graph.get("nodes").and_then(Json::as_array).is_some()
                });
                if in_its_shape {
                    let graph = child_graph.expect("checked above");
                    let child_nodes = graph["nodes"].as_array().expect("checked above");
                    let child_root = graph["root"]["value"].as_u64().unwrap() as usize;
                    let base = next;
                    prop_assert_eq!(
                        root,
                        &json!({ "type": "reference", "value": (base + child_root) as u64 })
                    );
                    prop_assert!(
                        base + child_nodes.len() <= nodes.len(),
                        "a child's nodes are missing"
                    );
                    for (index, node) in child_nodes.iter().enumerate() {
                        if let Err(why) = check_moved(node, &nodes[base + index], base as u64) {
                            prop_assert!(false, "{}", why);
                        }
                    }
                    next += child_nodes.len();
                } else {
                    prop_assert_eq!(
                        root,
                        &json!({ "type": "reference", "value": next as u64 })
                    );
                    prop_assert_eq!(
                        &nodes[next],
                        &json!({ "type": "error", "name": child_name, "message": child_message })
                    );
                    next += 1;
                }
            }
            prop_assert_eq!(next, nodes.len(), "unclaimed nodes");
            let mut found = Vec::new();
            for node in nodes {
                references(node, &mut found);
            }
            prop_assert!(
                found.iter().all(|index| (*index as usize) < nodes.len()),
                "a reference points out of the graph: {found:?}"
            );
        }
    }
}
