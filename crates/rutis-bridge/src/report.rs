//! An error as a host prints it (the diagnostics design §4.4, §4.6): its
//! own text, then which row it came from and what that row was doing, then
//! the frames of the plugin that threw it.
//!
//! Printing never fails and never becomes a fault of its own: whatever an
//! error holds (a graph with missing nodes, cycles, wrong types, a huge
//! stack, control characters), what is printed is at least the error's own
//! text, bounded and readable.

use rutis::host::ErrorKind;

/// At most this many bytes of a message or of one line of a stack.
pub const MAX_TEXT: usize = 4096;
/// At most this many frames of a stack.
pub const MAX_FRAMES: usize = 64;
/// At most this many lines [`whereabouts`] gives: `in <row>:`, the frames,
/// a `called from:` per caller recorded (at most
/// [`crate::exports::MAX_RETHROWN`]), and the count of frames left out.
pub const MAX_LINES: usize = MAX_FRAMES + crate::exports::MAX_RETHROWN + 2;

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

/// The lines that say where `error` came from, to print under its
/// [`headline`]: `in <row>:`, then the frames of the stack the plugin's
/// language gave (without its first line, which repeats the error's own
/// text), then `called from:` and the frames of each caller that threw it
/// on, at most [`MAX_FRAMES`] frames in all. None for an error with no
/// site and no stack, such as one raised on this side.
pub fn whereabouts(error: &ErrorKind) -> Vec<String> {
    let Some(thrown) = crate::thrown(error) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    if let Some(site) = thrown.site() {
        lines.push(printable(&format!("in {}:", site.row)));
    }
    // The other side's frames, then each caller's that threw it on, at
    // most MAX_FRAMES in all.
    let own = thrown.stack().map(|stack| {
        stack
            .lines()
            .skip(1)
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    });
    let callers = thrown.rethrown();
    let mut budget = MAX_FRAMES;
    let mut more = 0;
    let mut push = |lines: &mut Vec<String>, budget: &mut usize, frames: Vec<String>| {
        for frame in frames {
            if *budget == 0 {
                more += 1;
                continue;
            }
            *budget -= 1;
            lines.push(printable(frame.trim()));
        }
    };
    if let Some(own) = own {
        push(&mut lines, &mut budget, own);
    }
    for caller in callers {
        let caller: Vec<String> = caller
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect();
        if caller.is_empty() {
            continue;
        }
        // Past the budget a caller's frames are only counted: no
        // `called from:` line for frames not printed.
        if budget > 0 {
            lines.push("called from:".to_owned());
        }
        push(&mut lines, &mut budget, caller);
    }
    if more > 0 {
        lines.push(format!("… ({more} more frames)"));
    }
    lines
}

/// The first line printed for `error`, saying what failed when its site
/// tells: `call to forecast.tomorrow failed: KeyError: 'region'`,
/// `failed in apply: …`, `failed in cleanup: …`, `listener of
/// weather/asked failed: …`; a row that cannot load (`LoadError`) as its
/// message: `cannot load src/util.ts: …`. None when it does not say (no
/// site, or one that says too little), and the error's own text is
/// printed alone.
pub fn headline(error: &ErrorKind) -> Option<String> {
    let thrown = crate::thrown(error)?;
    let text = error.to_string();
    // A row that cannot load, or provides a name another has, says so
    // itself: `cannot load src/util.ts: …`, `forecast is already provided
    // by a`.
    if matches!(thrown, crate::session::Error::Remote { name, .. }
        if name == "LoadError" || name == "ProvideError")
    {
        return Some(printable(&text));
    }
    let site = thrown.site()?;
    let line = match (
        site.doing.as_str(),
        &site.service,
        &site.method,
        &site.event,
    ) {
        // A call that ran out of the host's time names it itself (`call
        // to weather.tomorrow did not finish within 30s …`).
        ("call", ..) if thrown.code() == Some(crate::session::code::TIMEOUT) => text,
        // A text that names the call already (`weather has no method
        // tomorow …`, `weather.tomorrow: argument 1 …`) is not wrapped.
        ("call", Some(service), Some(method), _)
            if text.starts_with(&format!("{service}.{method}"))
                || text.starts_with(&format!("{service} ")) =>
        {
            text
        }
        ("call", Some(service), Some(method), _) => {
            format!("call to {service}.{method} failed: {text}")
        }
        // A config that does not match its schema says so itself
        // (`config: units should be …`), not `failed in apply:`.
        ("apply", ..) if thrown.code() == Some(crate::session::code::BAD_CONFIG) => text,
        ("apply", ..) => format!("failed in apply: {text}"),
        ("cleanup", ..) => format!("failed in cleanup: {text}"),
        ("listener", _, _, Some(event)) => format!("listener of {event} failed: {text}"),
        _ => return None,
    };
    Some(printable(&line))
}

/// `error` as printed: its [`headline`] (or its own text when there is
/// none), then [`whereabouts`], each on a line of its own indented under
/// it.
pub fn report(error: &ErrorKind) -> String {
    let mut out = headline(error).unwrap_or_else(|| printable(&error.to_string()));
    for line in whereabouts(error) {
        out.push_str("\n    ");
        out.push_str(&line);
    }
    out
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
            "call to forecast.tomorrow failed: KeyError: 'region'\n    in weather:\n    File \"weather.py\", line 31, in tomorrow"
        );
    }

    /// A config that does not match its schema, raised in `apply` by a
    /// runtime's SDK (code `bad-config`), says so itself: no `failed in
    /// apply:` (config schema design §5.2).
    #[test]
    fn a_bad_config_is_its_own_headline() {
        let error = ErrorKind::PluginFailed(Box::new(Error::Remote {
            name: "BindingError".into(),
            message: "config: city is required".into(),
            graph: Some(json!({
                "root": { "type": "reference", "value": 0 },
                "nodes": [{
                    "type": "error", "name": "BindingError", "message": "config: city is required",
                    "code": "bad-config",
                    "site": { "row": "weather", "doing": "apply" }
                }]
            })),
        }));
        assert_eq!(
            headline(&error).as_deref(),
            Some("config: city is required")
        );
    }

    /// Each kind of site gives its headline; a site that says too little
    /// gives none (#677).
    #[test]
    fn each_site_gives_its_headline() {
        let with = |site: serde_json::Value| {
            remote(json!({
                "root": { "type": "reference", "value": 0 },
                "nodes": [{ "type": "error", "name": "KeyError", "message": "'region'", "site": site }]
            }))
        };
        for (site, expected) in [
            (
                json!({ "row": "weather", "doing": "apply" }),
                "failed in apply: KeyError: 'region'\n    in weather:",
            ),
            (
                json!({ "row": "weather", "doing": "cleanup" }),
                "failed in cleanup: KeyError: 'region'\n    in weather:",
            ),
            (
                json!({ "row": "weather", "doing": "listener", "event": "weather/asked" }),
                "listener of weather/asked failed: KeyError: 'region'\n    in weather:",
            ),
            (
                json!({ "row": "weather", "doing": "call" }),
                "KeyError: 'region'\n    in weather:",
            ),
        ] {
            assert_eq!(report(&with(site.clone())), expected, "{site}");
        }
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

    /// A full stack and many callers stay within [`MAX_LINES`]: callers
    /// past the frame budget are counted, not headed (§4.6).
    #[test]
    fn callers_past_the_budget_add_no_lines() {
        let stack = format!(
            "E: x\n{}",
            (0..MAX_FRAMES)
                .map(|i| format!("  at f{i}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let rethrown: Vec<String> = (0..20)
            .map(|i| format!("    at caller{i} (c.ts:{i})"))
            .collect();
        let error = remote(json!({
            "root": { "type": "reference", "value": 0 },
            "nodes": [{
                "type": "error", "name": "E", "message": "x", "stack": stack,
                "site": { "row": "weather", "doing": "apply" },
                "rethrown": rethrown,
            }]
        }));
        let lines = whereabouts(&error);
        assert!(lines.len() <= MAX_LINES, "{}", lines.len());
        assert_eq!(lines.len(), 1 + MAX_FRAMES + 1, "{lines:#?}");
        assert!(
            !lines.iter().any(|line| line == "called from:"),
            "{lines:#?}"
        );
        // Only the 8 a sender records are read.
        assert_eq!(lines.last().unwrap(), "… (8 more frames)");
        // With room, each caller is headed.
        let error = remote(json!({
            "root": { "type": "reference", "value": 0 },
            "nodes": [{ "type": "error", "name": "E", "message": "x", "rethrown": rethrown }]
        }));
        let lines = whereabouts(&error);
        assert_eq!(
            lines.iter().filter(|line| *line == "called from:").count(),
            8
        );
        assert!(lines.len() <= MAX_LINES);
    }

    /// The callers that threw the error on are printed under its own frames
    /// (§4.4).
    #[test]
    fn the_callers_that_threw_it_on_follow_its_frames() {
        let error = remote(json!({
            "root": { "type": "reference", "value": 0 },
            "nodes": [{
                "type": "error", "name": "KeyError", "message": "'region'",
                "stack": "KeyError: 'region'\n  File \"weather.py\", line 31, in tomorrow",
                "site": { "row": "weather", "doing": "call", "service": "forecast", "method": "tomorrow" },
                "rethrown": ["    at ask (src/chat.ts:14:36)", 3, "    at Chat.reply (chat.rs:9)"]
            }]
        }));
        assert_eq!(
            report(&error),
            "call to forecast.tomorrow failed: KeyError: 'region'\n    in weather:\n    File \"weather.py\", line 31, in tomorrow\n    called from:\n    at ask (src/chat.ts:14:36)\n    called from:\n    at Chat.reply (chat.rs:9)"
        );
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
        // A thrown value that is not an error prints as the value.
        let error = ErrorKind::PluginFailed(Box::new(Error::Remote {
            name: "ThrownValue".into(),
            message: "\"need 5 words\"".into(),
            graph: None,
        }));
        assert_eq!(report(&error), "\"need 5 words\"");
        // A row that cannot load: its message is the headline.
        let error = ErrorKind::PluginFailed(Box::new(Error::Remote {
            name: "LoadError".into(),
            message: "cannot load src/util.ts: SyntaxError: x (src/util.ts:3:7)".into(),
            graph: None,
        }));
        assert_eq!(
            headline(&error).as_deref(),
            Some("cannot load src/util.ts: SyntaxError: x (src/util.ts:3:7)")
        );
        assert_eq!(report(&ErrorKind::Closed), ErrorKind::Closed.to_string());
    }
}
