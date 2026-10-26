//! An error as a host prints it (the diagnostics design §4.4, §4.6): its
//! own text, then which row it came from and what that row was doing, then
//! the frames of the plugin that threw it.
//!
//! Printing never fails and never becomes a fault of its own: whatever an
//! error holds (a graph with missing nodes, cycles, wrong types, a huge
//! stack, control characters), what is printed is at least the error's own
//! text, bounded and readable.

use rutis::host::ErrorKind;

use crate::session::Site;

/// At most this many bytes of a message or of one line of a stack.
pub const MAX_TEXT: usize = 4096;
/// At most this many frames of a stack.
pub const MAX_FRAMES: usize = 64;

/// `text` made safe to print: control characters other than a newline or a
/// tab written as escapes (a NUL in a log makes `grep` take it for binary),
/// and at most [`MAX_TEXT`] bytes, the cut marked.
pub fn printable(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_TEXT) + 16);
    for c in text.chars() {
        if out.len() >= MAX_TEXT {
            out.push_str(" … (truncated)");
            return out;
        }
        match c {
            '\n' | '\t' => out.push(c),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// The lines that say where `error` came from, to print under its text:
/// `in <row>, <what it was doing>`, then the frames of the stack the
/// plugin's language gave (without its first line, which repeats the
/// error's own text), at most [`MAX_FRAMES`]. None for an error with no
/// site and no stack, such as one rutis or Rust code raised.
pub fn whereabouts(error: &ErrorKind) -> Vec<String> {
    let Some(thrown) = crate::thrown(error) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    if let Some(site) = thrown.site() {
        lines.push(printable(&doing(&site)));
    }
    if let Some(stack) = thrown.stack() {
        let mut frames = stack.lines().skip(1).filter(|line| !line.trim().is_empty());
        for frame in frames.by_ref().take(MAX_FRAMES) {
            lines.push(printable(frame.trim()));
        }
        let more = frames.count();
        if more > 0 {
            lines.push(format!("… ({more} more frames)"));
        }
    }
    lines
}

/// `error` as printed: its own text, then [`whereabouts`], each on a line
/// of its own indented under it.
pub fn report(error: &ErrorKind) -> String {
    let mut out = printable(&error.to_string());
    for line in whereabouts(error) {
        out.push_str("\n    ");
        out.push_str(&line);
    }
    out
}

/// What a site says, as a line: `in weather, calling forecast.tomorrow`.
fn doing(site: &Site) -> String {
    let what = match (
        site.doing.as_str(),
        &site.service,
        &site.method,
        &site.event,
    ) {
        ("call", Some(service), Some(method), _) => format!("serving {service}.{method}"),
        ("listener", _, _, Some(event)) => format!("listening to {event}"),
        ("apply", ..) => "in apply".to_owned(),
        ("cleanup", ..) => "in its cleanup".to_owned(),
        (other, ..) => other.to_owned(),
    };
    format!("in {}, {what}", site.row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Error;
    use serde_json::json;

    fn remote(graph: serde_json::Value) -> ErrorKind {
        ErrorKind::PluginFailed(Box::new(Error::Remote {
            name: "KeyError".into(),
            message: "'region'".into(),
            graph: Some(graph),
        }))
    }

    #[test]
    fn an_error_from_a_plugin_prints_its_row_and_frames() {
        let error = remote(json!({
            "root": { "type": "reference", "value": 0 },
            "nodes": [{
                "type": "error", "name": "KeyError", "message": "'region'",
                "stack": "KeyError: 'region'\n  File \"weather.py\", line 31, in tomorrow\n",
                "site": { "row": "weather", "doing": "call", "service": "forecast", "method": "tomorrow" }
            }]
        }));
        assert_eq!(
            report(&error),
            "KeyError: 'region'\n    in weather, serving forecast.tomorrow\n    File \"weather.py\", line 31, in tomorrow"
        );
    }

    /// Whatever the graph holds, printing gives at least the error's text
    /// and never panics (§4.6).
    #[test]
    fn a_broken_graph_prints_the_error_alone() {
        for graph in [
            json!(null),
            json!({}),
            json!({ "root": { "type": "reference", "value": 7 }, "nodes": [] }),
            json!({ "root": { "type": "reference", "value": 0 }, "nodes": [{ "type": "array", "values": [] }] }),
            json!({ "root": { "type": "reference", "value": 0 }, "nodes": [{ "type": "error", "stack": 3, "site": "weather" }] }),
            json!({ "root": { "type": "reference", "value": 0 }, "nodes": [{ "type": "error", "site": { "row": 1 } }] }),
            json!({ "root": { "type": "reference", "value": 0 }, "nodes": [{ "type": "error", "cause": { "type": "reference", "value": 0 } }] }),
        ] {
            assert_eq!(
                report(&remote(graph.clone())),
                "KeyError: 'region'",
                "{graph}"
            );
        }
    }

    #[test]
    fn printing_is_bounded_and_escapes_control_characters() {
        let stack = format!(
            "E: x\n{}",
            (0..100)
                .map(|i| format!("  at f{i}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let error = remote(json!({
            "root": { "type": "reference", "value": 0 },
            "nodes": [{ "type": "error", "name": "E", "message": "x", "stack": stack }]
        }));
        let lines = whereabouts(&error);
        assert_eq!(lines.len(), MAX_FRAMES + 1);
        assert_eq!(lines.last().unwrap(), "… (36 more frames)");
        assert_eq!(printable("row\0:12"), "row\\u{0}:12");
        let long = printable(&"é".repeat(MAX_TEXT));
        assert!(long.ends_with(" … (truncated)"));
        assert!(long.len() <= MAX_TEXT + " … (truncated)".len() + 4);
    }

    /// An error with no graph (rutis's own, or an older end's) prints as
    /// its text alone.
    #[test]
    fn an_error_without_a_graph_prints_as_its_text() {
        let error = ErrorKind::PluginFailed(Box::new(Error::Remote {
            name: "Error".into(),
            message: "boom".into(),
            graph: None,
        }));
        assert_eq!(report(&error), "Error: boom");
        assert_eq!(report(&ErrorKind::Closed), ErrorKind::Closed.to_string());
    }
}
