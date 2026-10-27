//! A row's config against its plugin's JSON Schema: defaults filled, then
//! validated (config schema design §3–§6). The Rust copy of the rules every
//! language's SDK has; `specs/config-schema/cases.json` keeps them the
//! same word for word.
//!
//! Keywords checked: `type`, `properties`, `required`,
//! `additionalProperties`, `items`, `enum`, `const`, `minimum`, `maximum`,
//! `default`, `anyOf`, `oneOf`, `allOf`, `$ref` (to `#/$defs/…` and
//! `#/definitions/…`). Others are neither checked nor warned about.

use std::collections::HashSet;

use rutis::internal::config::value_text;
use rutis::{ConfigError, ConfigIssue, PathSegment};
use serde_json::{Map, Value};

/// How a config is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options {
    /// Missing `required` keys are errors. Off for the manifests of Go
    /// binaries built with an SDK older than 0.18, which list every field
    /// without `omitempty` as required (design §7).
    pub required: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { required: true }
    }
}

impl Options {
    /// Missing `required` keys are not errors.
    pub fn without_required(mut self) -> Self {
        self.required = false;
        self
    }
}

/// What checking a config gives.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Checked {
    /// The config with defaults filled, an integral number passing
    /// `integer` written as an integer: what `apply` gets. `Null` for no
    /// value.
    pub config: Value,
    /// Every problem, sorted by path (design §6.4); none when it passes.
    pub errors: Vec<ConfigIssue>,
    /// Extra keys where the schema allows them (design §3.2), sorted.
    pub warnings: Vec<ConfigIssue>,
}

impl Checked {
    /// The errors as one [`ConfigError`], when there are any.
    pub fn error(&self) -> Option<ConfigError> {
        (!self.errors.is_empty()).then(|| ConfigError::new(self.errors.clone()))
    }
}

/// Fill `config` (`None` or `null`: no config) with the defaults of
/// `schema`, then validate it.
pub fn check(schema: &Value, config: Option<&Value>, options: Options) -> Checked {
    let doc = Document { root: schema };
    let config = config.filter(|value| !value.is_null());
    let root = vec![schema];
    let mut value = match config {
        Some(value) => value.clone(),
        None => match doc.default_of(&root) {
            Some(default) => default.clone(),
            None if doc.is_object_root(schema) => Value::Object(Map::new()),
            None => Value::Null,
        },
    };
    doc.fill(&root, &mut value);
    let mut sink = Sink {
        warn: true,
        ..Sink::default()
    };
    doc.validate(
        &root,
        &mut value,
        &mut Vec::new(),
        options,
        &mut sink,
        &mut Vec::new(),
    );
    // No value where the root does not allow it.
    if value.is_null() && config.is_none() && !sink.errors.is_empty() {
        sink.errors = vec![ConfigIssue::at(Vec::new(), "is required")];
    }
    Checked {
        config: value,
        errors: sorted(sink.errors),
        warnings: sorted(sink.warnings),
    }
}

/// Problems sorted by path (a stable sort: at one path, in the order they
/// were found), each once.
fn sorted(mut issues: Vec<ConfigIssue>) -> Vec<ConfigIssue> {
    issues.sort_by(|a, b| a.path.cmp(&b.path));
    let mut seen = HashSet::new();
    issues.retain(|issue| seen.insert((issue.path.clone(), issue.message.clone())));
    issues
}

#[derive(Default)]
struct Sink {
    errors: Vec<ConfigIssue>,
    warnings: Vec<ConfigIssue>,
    /// Extra keys are warned about (not inside `anyOf` / `oneOf` branches).
    warn: bool,
}

/// The schema document `$ref`s resolve in.
struct Document<'a> {
    root: &'a Value,
}

impl<'a> Document<'a> {
    /// `#/$defs/…` or `#/definitions/…` in the document.
    fn resolve(&self, reference: &str) -> Option<&'a Value> {
        let pointer = reference.strip_prefix('#')?;
        if !(pointer.starts_with("/$defs/") || pointer.starts_with("/definitions/")) {
            return None;
        }
        self.root.pointer(pointer)
    }

    /// `schemas` and what they take in through `$ref` and `allOf`, each
    /// once: a schema, then its `$ref`'s, then its `allOf` members' in
    /// order.
    fn parts(&self, schemas: &[&'a Value]) -> Vec<&'a Map<String, Value>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for schema in schemas {
            self.collect(schema, &mut out, &mut seen);
        }
        out
    }

    fn collect(
        &self,
        schema: &'a Value,
        out: &mut Vec<&'a Map<String, Value>>,
        seen: &mut HashSet<*const Map<String, Value>>,
    ) {
        let Some(node) = schema.as_object() else {
            return;
        };
        if !seen.insert(node as *const _) {
            return;
        }
        out.push(node);
        if let Some(target) = node
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| self.resolve(r))
        {
            self.collect(target, out, seen);
        }
        for member in node
            .get("allOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.collect(member, out, seen);
        }
    }

    /// The first `default` of `schemas` (through `$ref` and `allOf`).
    fn default_of(&self, schemas: &[&'a Value]) -> Option<&'a Value> {
        self.parts(schemas)
            .into_iter()
            .find_map(|part| part.get("default"))
    }

    /// The root's type is `object` or includes it.
    fn is_object_root(&self, schema: &'a Value) -> bool {
        self.parts(&[schema])
            .into_iter()
            .find_map(|part| part.get("type"))
            .map(|ty| types(ty).contains(&"object"))
            .unwrap_or(false)
    }

    /// The schemas a child at `key` is checked against: each part's
    /// `properties[key]`.
    fn property(parts: &[&'a Map<String, Value>], key: &str) -> Vec<&'a Value> {
        parts
            .iter()
            .filter_map(|part| part.get("properties")?.as_object()?.get(key))
            .collect()
    }

    /// The first `additionalProperties` of `parts`.
    fn additional(parts: &[&'a Map<String, Value>]) -> Option<&'a Value> {
        parts
            .iter()
            .find_map(|part| part.get("additionalProperties"))
    }

    /// Every `properties` key of `parts`.
    fn known(parts: &[&'a Map<String, Value>]) -> Vec<String> {
        let mut known: Vec<String> = parts
            .iter()
            .filter_map(|part| part.get("properties")?.as_object())
            .flat_map(|properties| properties.keys().cloned())
            .collect();
        known.sort();
        known.dedup();
        known
    }

    /// Fill defaults level by level: in an object that is there, each
    /// missing property with a `default`; then into the child objects and
    /// array items. Not into `anyOf` / `oneOf`.
    fn fill(&self, schemas: &[&'a Value], value: &mut Value) {
        let parts = self.parts(schemas);
        match value {
            Value::Object(map) => {
                for part in &parts {
                    let Some(properties) = part.get("properties").and_then(Value::as_object) else {
                        continue;
                    };
                    for (key, property) in properties {
                        if map.contains_key(key) {
                            continue;
                        }
                        if let Some(default) = self.default_of(&[property]) {
                            map.insert(key.clone(), default.clone());
                        }
                    }
                }
                let known = Self::known(&parts);
                let additional = Self::additional(&parts).filter(|a| a.is_object());
                for (key, child) in map.iter_mut() {
                    let below = match known.binary_search(key) {
                        Ok(_) => Self::property(&parts, key),
                        Err(_) => additional.into_iter().collect(),
                    };
                    if !below.is_empty() {
                        self.fill(&below, child);
                    }
                }
            }
            Value::Array(items) => {
                let below: Vec<&Value> = parts
                    .iter()
                    .filter_map(|part| part.get("items").filter(|i| i.is_object()))
                    .collect();
                if !below.is_empty() {
                    for item in items {
                        self.fill(&below, item);
                    }
                }
            }
            _ => {}
        }
    }

    /// Validate `value` at `path` against every one of `schemas`.
    fn validate(
        &self,
        schemas: &[&'a Value],
        value: &mut Value,
        path: &mut Vec<PathSegment>,
        options: Options,
        sink: &mut Sink,
        entered: &mut Vec<*const Value>,
    ) {
        let parts = self.parts(schemas);
        // The type first: when it is wrong, nothing else at or below it.
        for part in &parts {
            let Some(ty) = part.get("type") else {
                continue;
            };
            let names = types(ty);
            if names.is_empty() {
                continue;
            }
            if !names.iter().any(|name| is_type(name, value)) {
                let expected: Vec<String> = names.iter().map(|name| type_text(name)).collect();
                sink.errors.push(ConfigIssue::at(
                    path.clone(),
                    &format!("should be {}, got {}", joined(&expected), value_text(value)),
                ));
                return;
            }
            if names.contains(&"integer") {
                integral(value);
            }
        }
        for part in &parts {
            if let Some(allowed) = part.get("enum").and_then(Value::as_array) {
                if !allowed.iter().any(|candidate| same(candidate, value)) {
                    let texts: Vec<String> = allowed.iter().map(value_text).collect();
                    sink.errors.push(ConfigIssue::at(
                        path.clone(),
                        &format!("should be {}, got {}", joined(&texts), value_text(value)),
                    ));
                }
            }
            if let Some(constant) = part.get("const") {
                if !same(constant, value) {
                    sink.errors.push(ConfigIssue::at(
                        path.clone(),
                        &format!(
                            "should be {}, got {}",
                            value_text(constant),
                            value_text(value)
                        ),
                    ));
                }
            }
            if let Some(n) = value.as_f64() {
                if let Some(minimum) = part.get("minimum").and_then(Value::as_f64) {
                    if n < minimum {
                        sink.errors.push(ConfigIssue::at(
                            path.clone(),
                            &format!(
                                "should be at least {}, got {}",
                                value_text(&part["minimum"]),
                                value_text(value)
                            ),
                        ));
                    }
                }
                if let Some(maximum) = part.get("maximum").and_then(Value::as_f64) {
                    if n > maximum {
                        sink.errors.push(ConfigIssue::at(
                            path.clone(),
                            &format!(
                                "should be at most {}, got {}",
                                value_text(&part["maximum"]),
                                value_text(value)
                            ),
                        ));
                    }
                }
            }
            for keyword in ["anyOf", "oneOf"] {
                let Some(branches) = part.get(keyword).and_then(Value::as_array) else {
                    continue;
                };
                let mut passed = None;
                for branch in branches {
                    // A branch already being tried for this same value (a
                    // schema that refers back to itself through `anyOf` /
                    // `oneOf`) does not match.
                    let at = branch as *const Value;
                    if entered.contains(&at) {
                        continue;
                    }
                    let mut copy = value.clone();
                    let mut trial = Sink::default();
                    entered.push(at);
                    self.validate(&[branch], &mut copy, path, options, &mut trial, entered);
                    entered.pop();
                    if trial.errors.is_empty() {
                        passed = Some(copy);
                        break;
                    }
                }
                match passed {
                    Some(copy) => *value = copy,
                    None => {
                        let texts = self.branch_texts(branches);
                        sink.errors.push(ConfigIssue::at(
                            path.clone(),
                            &format!("should be {}, got {}", joined(&texts), value_text(value)),
                        ));
                    }
                }
            }
        }
        match value {
            Value::Object(map) => {
                if options.required {
                    for part in &parts {
                        for key in part
                            .get("required")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                        {
                            if !map.contains_key(key) {
                                path.push(PathSegment::Key(key.to_owned()));
                                sink.errors
                                    .push(ConfigIssue::at(path.clone(), "is required"));
                                path.pop();
                            }
                        }
                    }
                }
                let known = Self::known(&parts);
                let additional = Self::additional(&parts);
                let described = parts.iter().any(|part| part.contains_key("properties"));
                for (key, child) in map.iter_mut() {
                    path.push(PathSegment::Key(key.clone()));
                    if known.binary_search(key).is_ok() {
                        let below = Self::property(&parts, key);
                        self.validate(&below, child, path, options, sink, &mut Vec::new());
                    } else {
                        match additional {
                            Some(Value::Bool(false)) => {
                                sink.errors.push(extra(path, key, &known));
                            }
                            Some(schema @ Value::Object(_)) => {
                                self.validate(
                                    &[schema],
                                    child,
                                    path,
                                    options,
                                    sink,
                                    &mut Vec::new(),
                                );
                            }
                            None if described && sink.warn => {
                                sink.warnings.push(extra(path, key, &known));
                            }
                            _ => {}
                        }
                    }
                    path.pop();
                }
            }
            Value::Array(items) => {
                let below: Vec<&Value> = parts
                    .iter()
                    .filter_map(|part| part.get("items").filter(|i| i.is_object()))
                    .collect();
                if !below.is_empty() {
                    for (index, item) in items.iter_mut().enumerate() {
                        path.push(PathSegment::Index(index));
                        self.validate(&below, item, path, options, sink, &mut Vec::new());
                        path.pop();
                    }
                }
            }
            _ => {}
        }
    }

    /// How the branches of a failed `anyOf` / `oneOf` are written (design
    /// §6.5), in order, each once.
    fn branch_texts(&self, branches: &'a [Value]) -> Vec<String> {
        let mut texts: Vec<String> = Vec::new();
        for branch in branches {
            let parts = self.parts(&[branch]);
            let found = parts.iter().find_map(|part| {
                if let Some(constant) = part.get("const") {
                    return Some(vec![value_text(constant)]);
                }
                if let Some(allowed) = part.get("enum").and_then(Value::as_array) {
                    return Some(allowed.iter().map(value_text).collect());
                }
                let names = types(part.get("type")?);
                (!names.is_empty()).then(|| names.iter().map(|name| type_text(name)).collect())
            });
            for text in found.unwrap_or_else(|| vec!["another accepted value".to_owned()]) {
                if !texts.contains(&text) {
                    texts.push(text);
                }
            }
        }
        texts
    }
}

/// `<path> is not a config key`, with a close known key suggested.
fn extra(path: &[PathSegment], key: &str, known: &[String]) -> ConfigIssue {
    let reason = match crate::loader::closest(key, known) {
        Some(like) => format!("is not a config key (did you mean {like}?)"),
        None => "is not a config key".to_owned(),
    };
    ConfigIssue::at(path.to_vec(), &reason)
}

/// The type names of `type` this module knows.
fn types(ty: &Value) -> Vec<&str> {
    const NAMES: [&str; 7] = [
        "string", "number", "integer", "boolean", "object", "array", "null",
    ];
    let names: Vec<&str> = match ty {
        Value::String(name) => vec![name.as_str()],
        Value::Array(names) => names.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    names
        .into_iter()
        .filter(|name| NAMES.contains(name))
        .collect()
}

fn is_type(name: &str, value: &Value) -> bool {
    match name {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => match value {
            Value::Number(n) => {
                n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.fract() == 0.0)
            }
            _ => false,
        },
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "null" => value.is_null(),
        _ => false,
    }
}

/// An integral float written as an integer.
fn integral(value: &mut Value) {
    if let Value::Number(n) = value {
        if let (false, Some(f)) = (n.is_i64() || n.is_u64(), n.as_f64()) {
            if f.fract() == 0.0 && f.abs() <= 9_007_199_254_740_992.0 {
                *value = Value::from(f as i64);
            }
        }
    }
}

fn type_text(name: &str) -> String {
    match name {
        "string" => "a string",
        "number" => "a number",
        "integer" => "an integer",
        "boolean" => "a boolean",
        "object" => "an object",
        "array" => "an array",
        _ => "null",
    }
    .to_owned()
}

/// `a`, `a or b`, `one of a, b, c`.
fn joined(texts: &[String]) -> String {
    match texts {
        [one] => one.clone(),
        [a, b] => format!("{a} or {b}"),
        _ => format!("one of {}", texts.join(", ")),
    }
}

/// JSON equality: numbers by value, objects regardless of key order.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64()) {
            (Some(x), Some(y)) => x == y,
            _ => match (x.as_u64(), y.as_u64()) {
                (Some(x), Some(y)) => x == y,
                _ => x.as_f64() == y.as_f64(),
            },
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, x)| y.get(key).is_some_and(|y| same(x, y)))
        }
        _ => a == b,
    }
}
