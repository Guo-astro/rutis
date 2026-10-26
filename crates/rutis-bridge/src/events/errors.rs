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

/// `<ErrorName>: <message>` of an error, as a far end would read it.
fn named(error: &ErrorKind) -> String {
    match to_session(error) {
        Error::Remote { name, message, .. } => format!("{name}: {message}"),
        Error::SyncWaitCycle(message) => format!("SyncWaitCycle: {message}"),
        other => format!("BindingError: {other}"),
    }
}

/// The line a host prints for an event failure its error sink receives
/// (events protocol §5.6): `rutis: listener of <name> in <where> failed:
/// <ErrorName>: <message>`, and the same for a filter; `None` for any other
/// error.
pub fn report_line(error: &ErrorKind) -> Option<String> {
    match error {
        ErrorKind::ListenerFailed {
            name,
            listener,
            error,
        } => {
            let mut line = format!(
                "rutis: listener of {name} in {listener} failed: {}",
                crate::report::printable(&named(error))
            );
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
            "rutis: filter of {name} from {emitter} failed: {}",
            named(error)
        )),
        ErrorKind::PluginFailed(source) => {
            let line = source.to_string();
            line.starts_with("filter of ")
                .then(|| format!("rutis: {line}"))
        }
        _ => None,
    }
}

/// `AggregateError`: `<n> listeners of <name> failed`, its graph's root an
/// error node whose `errors` are the children's (as JavaScript encodes an
/// `AggregateError`).
fn aggregate(name: &str, errors: &[Arc<ErrorKind>]) -> Error {
    let message = format!("{} listeners of {name} failed", errors.len());
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
}
