//! A row's config that does not match its plugin's schema: the error and
//! how its text is written (config schema design §5, §6.2, §6.3; the shared
//! case list `specs/config-schema/cases.json` locks the text).

use std::fmt;

use serde_json::Value;

mod decode;
pub use decode::decode;

/// At most this many problems are written; the rest as one last line.
const SHOWN: usize = 20;

/// A row's config does not match what its plugin accepts: every problem,
/// sorted by path (error code `bad-config`).
///
/// Its text is `config: <problem>`, one line per problem (at most 20, then
/// `config: and N more problems`), wherever it is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    issues: Vec<ConfigIssue>,
}

impl ConfigError {
    /// An error with these problems, in the order given. For rutis's own
    /// crates: a plugin's own config check returns errors of its own, so
    /// that they stay apart from this one (config schema design §5.2).
    #[doc(hidden)]
    pub fn new(issues: Vec<ConfigIssue>) -> Self {
        ConfigError { issues }
    }

    /// Every problem, in order.
    pub fn issues(&self) -> &[ConfigIssue] {
        &self.issues
    }

    /// The text of each line [`Display`](fmt::Display) writes.
    pub fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .issues
            .iter()
            .take(SHOWN)
            .map(|issue| format!("config: {}", issue.message))
            .collect();
        if self.issues.len() > SHOWN {
            lines.push(format!(
                "config: and {} more problems",
                self.issues.len() - SHOWN
            ));
        }
        lines
    }

    /// The config error in `error`, if it is one (a row's config step
    /// failing, passed as a plugin's error).
    pub fn find<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a ConfigError> {
        let mut current = Some(error);
        while let Some(error) = current {
            if let Some(config) = error.downcast_ref::<ConfigError>() {
                return Some(config);
            }
            current = error.source();
        }
        None
    }
}

impl ConfigError {
    /// The config error `error` is, if it is one: a row's config step
    /// failing, passed as a plugin's error.
    pub fn of(error: &crate::ErrorKind) -> Option<&ConfigError> {
        match error {
            crate::ErrorKind::PluginFailed(source) => ConfigError::find(source.as_ref()),
            _ => None,
        }
    }

    /// This error as a plugin's error.
    #[doc(hidden)]
    pub fn into_error(self) -> crate::Error {
        crate::Error::from(self)
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.lines().join("\n"))
    }
}

impl std::error::Error for ConfigError {}

/// One problem: where it is, and its sentence (path included:
/// `home.lat should be a number, got "north"`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConfigIssue {
    pub path: Vec<PathSegment>,
    pub message: String,
}

impl ConfigIssue {
    /// The problem at `path`, its sentence `<path> <reason>`.
    #[doc(hidden)]
    pub fn at(path: Vec<PathSegment>, reason: &str) -> Self {
        let message = format!("{} {reason}", path_text(&path));
        ConfigIssue { path, message }
    }
}

/// A key of an object or an index of an array.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PathSegment {
    Key(String),
    Index(usize),
}

/// How a path is written: `home.lat`, `cities[2]`, `headers["x-api"]`;
/// `config` for the root (and before a first segment that is not an
/// identifier key).
pub fn path_text(path: &[PathSegment]) -> String {
    let mut out = String::new();
    for (index, segment) in path.iter().enumerate() {
        match segment {
            PathSegment::Key(key) if identifier(key) => {
                if index > 0 {
                    out.push('.');
                }
                out.push_str(key);
            }
            PathSegment::Key(key) => {
                if index == 0 {
                    out.push_str("config");
                }
                out.push('[');
                out.push_str(&quoted(key, usize::MAX));
                out.push(']');
            }
            PathSegment::Index(at) => {
                if index == 0 {
                    out.push_str("config");
                }
                out.push_str(&format!("[{at}]"));
            }
        }
    }
    if out.is_empty() {
        out.push_str("config");
    }
    out
}

/// `[A-Za-z_][A-Za-z0-9_]*`.
fn identifier(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// How a value is written after `got` (and in `enum` lists): strings,
/// numbers, booleans and `null` as JSON, a string past 40 scalar values cut
/// with `…`; `an object`, `an array of N`.
pub fn value_text(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number_text(n),
        Value::String(s) => quoted(s, 40),
        Value::Array(items) => format!("an array of {}", items.len()),
        Value::Object(_) => "an object".to_owned(),
    }
}

/// A number as JavaScript's `String(number)` writes it; an integral value
/// of magnitude at most 2^53 without a decimal point.
pub fn number_text(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    let f = n.as_f64().unwrap_or(f64::NAN);
    float_text(f)
}

/// `f` as JavaScript's `String(number)` writes it.
pub fn float_text(f: f64) -> String {
    if !f.is_finite() {
        return f.to_string();
    }
    if f == 0.0 {
        return "0".to_owned();
    }
    if f.fract() == 0.0 && f.abs() <= 9_007_199_254_740_992.0 {
        return (f as i64).to_string();
    }
    // Shortest round-trip digits and the exponent, as `d.ddde<n>`.
    let shortest = format!("{:e}", f.abs());
    let (mantissa, exponent) = shortest.split_once('e').expect("an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let exponent: i32 = exponent.parse().expect("an integer exponent");
    let k = digits.len() as i32;
    // JavaScript's n: the position of the decimal point.
    let n = exponent + 1;
    let sign = if f < 0.0 { "-" } else { "" };
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let e = if e >= 0 {
            format!("e+{e}")
        } else {
            format!("e-{}", -e)
        };
        match k {
            1 => format!("{digits}{e}"),
            _ => format!("{}.{}{e}", &digits[..1], &digits[1..]),
        }
    };
    format!("{sign}{body}")
}

/// `s` as a JSON string, its content cut to `limit` scalar values with `…`;
/// `"`, `\` and control characters escaped, everything else as it is.
fn quoted(s: &str, limit: usize) -> String {
    let mut out = String::from("\"");
    let count = s.chars().count();
    for c in s.chars().take(limit) {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    if count > limit {
        out.push('…');
    }
    out.push('"');
    out
}

/// The name of `known` closest to `name`, when it is close enough to be a
/// misspelling of it: at most one edit for a name of 3–6 scalar values, two
/// for a longer one, none for a shorter. The smallest distance wins, then
/// the first in `known` (callers sort it by code point).
pub fn closest(name: &str, known: &[String]) -> Option<String> {
    let limit = match name.chars().count() {
        0..=2 => return None,
        3..=6 => 1,
        _ => 2,
    };
    known
        .iter()
        .filter(|candidate| candidate.as_str() != name)
        .map(|candidate| (distance(name, candidate), candidate))
        .filter(|(distance, _)| *distance <= limit)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate.clone())
}

/// Edits (insert, delete, replace, swap two neighbours) between `a` and
/// `b`, in scalar values (OSA distance).
pub fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut rows = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in rows.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in rows[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (rows[i - 1][j] + 1)
                .min(rows[i][j - 1] + 1)
                .min(rows[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(rows[i - 2][j - 2] + 1);
            }
            rows[i][j] = best;
        }
    }
    rows[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_are_written_as_javascript_writes_them() {
        for (value, text) in [
            (1.0, "1"),
            (1.5, "1.5"),
            (-0.0, "0"),
            (1e21, "1e+21"),
            (1.5e21, "1.5e+21"),
            (1e-7, "1e-7"),
            (0.000001, "0.000001"),
            (123.456, "123.456"),
            (-2.5, "-2.5"),
            (0.1, "0.1"),
        ] {
            assert_eq!(float_text(value), text, "{value}");
        }
    }

    #[test]
    fn paths_and_values_are_written_as_the_design_says() {
        let key = |k: &str| PathSegment::Key(k.to_owned());
        assert_eq!(path_text(&[]), "config");
        assert_eq!(path_text(&[key("home"), key("lat")]), "home.lat");
        assert_eq!(
            path_text(&[key("cities"), PathSegment::Index(2)]),
            "cities[2]"
        );
        assert_eq!(
            path_text(&[key("headers"), key("x-api")]),
            "headers[\"x-api\"]"
        );
        assert_eq!(path_text(&[key("x-api")]), "config[\"x-api\"]");
        assert_eq!(value_text(&json!("a\"\\\n\u{1}")), r#""a\"\\\n\u0001""#);
        assert_eq!(value_text(&json!([1, 2])), "an array of 2");
        assert_eq!(value_text(&json!({})), "an object");
    }

    #[test]
    fn more_than_twenty_problems_end_with_a_count() {
        let issues = (0..23)
            .map(|i| ConfigIssue::at(vec![PathSegment::Index(i)], "is required"))
            .collect();
        let lines = ConfigError::new(issues).lines();
        assert_eq!(lines.len(), 21);
        assert_eq!(lines[20], "config: and 3 more problems");
    }
}
