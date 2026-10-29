//! The unified events conformance suite (#464 step 7): the expectation file
//! [`EVENTS_EXPECTED`] (`events.expected.json`, format: the protocol spec
//! `docs/design-events-protocol-2026-10-18.md` §13), read into cases, the
//! placements each case runs in, and the engine that runs one placement
//! against a [`World`] and checks every expectation.
//!
//! A world is where the participants of a case live: the real one is a rutis
//! host with its runtimes and a second node
//! (`crates/rutis-loader/tests/events_conformance.rs`); the SDK testing tools
//! run the same file in one process (`node/rutis/test/events-conformance.test.mjs`,
//! `go/rutis/rutistest/events_conformance_test.go`).
//!
//! # The plugin `conformance-events`
//!
//! Every participant of a case is a row of it, keyed by its label; its
//! config is the participant as [`rows`] resolves it:
//!
//! - `name`, `kind` (`sync` / `async` / `any`), `options` (`prepend`,
//!   `global`), `log` (`"label"`, `"arg"`, field names or `false`) and
//!   `does`: the row registers one listener with its language's own API.
//!   On every call it logs, then does what `does` says (the table of §13.2).
//! - `emitter: true`: the row provides `emitter-<label>` with
//!   `dispatch(spec)` (async) and `dispatch_sync(spec)` (sync, for `bail`),
//!   emitting with its language's own API. Plugins
//!   declare `emitter-e` and `emitter-f`; emitter rows are only ever
//!   labelled so.
//! - `provides`: it also provides an undeclared service of that name;
//!   `fails`: its `apply` throws that error, registering nothing;
//!   `inject`: the row's own `inject` list (service names), as a
//!   configuration gives it, for the loader to pass on;
//!   `observesDispatch` (Cordis): it logs `<label>:dispatch:<mode>` from
//!   Cordis's `internal/dispatch` for each dispatch of `name`.
//!
//! It injects `events-log` ([`EventsLog`]): `add(entry)` (sync),
//! `filter(description)` (sync: logs `filter:<row>` and keeps the
//! description), `wait(entry)` (async: resolves once `entry` is logged)
//! and `until(entry)` (sync: logs `until:<entry>`, returns once `entry` is
//! logged).
//! A spec given to `dispatch` has `mode`, `name`, `args`, and optionally
//! `filter` (`{keep}` or `{throws}`), `via`, `terminal` (a text it returns,
//! or `{echo: true}`: it returns its first argument), `cancel` and `then`
//! (`{until, args}`: after the dispatch, `events-log.until(until)`, then
//! the same emit again with `args`).
//! Logged values: strings as they are, numbers and booleans as JSON.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rutis::host::BoxFuture;
use serde::Deserialize;
use serde_json::{json, Value as Json};

use crate::session::{settle, Error, HostDispatch, Reply, Value};

/// The cases every implementation of the unified events is checked
/// against; its `about` says how to read it.
pub const EVENTS_EXPECTED: &str = include_str!("events.expected.json");

/// How long a log expectation, or a step that `settles`, may take, on a
/// quiet machine; [`scaled`] scales it.
pub const SETTLE: Duration = Duration::from_secs(5);

/// How long a `logLacks` waits after the rest of the step held, for an
/// entry that would come late; [`scaled`] scales it too.
pub const LACKS: Duration = Duration::from_millis(300);

/// [`SETTLE`] (or `base`) times `RUTIS_EVENTS_SETTLE_SCALE` (a number, 1
/// when unset): CI machines run many processes at once and set it.
pub fn scaled(base: Duration) -> Duration {
    let scale = std::env::var("RUTIS_EVENTS_SETTLE_SCALE")
        .ok()
        .and_then(|scale| scale.parse::<f64>().ok())
        .filter(|scale| *scale >= 1.0)
        .unwrap_or(1.0);
    base.mul_f64(scale)
}

/// How long any other dispatch may take before the case fails.
pub const PATIENCE: Duration = Duration::from_secs(30);

// ── The file ────────────────────────────────────────────────────

/// `events.expected.json`, read.
#[derive(Debug, Clone, Deserialize)]
pub struct Expected {
    pub about: Vec<String>,
    pub plugin: String,
    /// The languages `only` and `except` name.
    pub languages: Vec<String>,
    /// Scope path (`t1/s1`) → the group its instance is of.
    pub scopes: BTreeMap<String, String>,
    pub locations: Vec<Location>,
    /// How many cases each testing tool runs ([`tool_combo`]); each tool
    /// checks it ran that many.
    #[serde(rename = "toolCases")]
    pub tool_cases: BTreeMap<String, usize>,
    pub cases: Vec<Case>,
}

/// Read [`EVENTS_EXPECTED`]. Panics if it does not parse.
pub fn expected() -> Expected {
    serde_json::from_str(EVENTS_EXPECTED).expect("events.expected.json")
}

/// Where a participant is (§13.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
pub enum Location {
    /// The runtime under test.
    X,
    /// Another runtime process: X's language again, then each other one.
    Y,
    /// A Rust plugin in the host.
    #[serde(rename = "host")]
    Host,
    /// A Rust plugin on a second node, linked to the host.
    #[serde(rename = "node")]
    Node,
}

impl Location {
    pub const ALL: [Location; 4] = [Location::X, Location::Y, Location::Host, Location::Node];

    pub fn as_str(self) -> &'static str {
        match self {
            Location::X => "X",
            Location::Y => "Y",
            Location::Host => "host",
            Location::Node => "node",
        }
    }

    fn parse(text: &str) -> Result<Self, String> {
        match text {
            "X" => Ok(Location::X),
            "Y" => Ok(Location::Y),
            "host" => Ok(Location::Host),
            "node" => Ok(Location::Node),
            other => Err(format!("unknown location {other:?}")),
        }
    }
}

/// One case of the file.
#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    pub case: String,
    pub rule: String,
    /// The default event name.
    pub name: String,
    pub listeners: Vec<Json>,
    pub steps: Vec<Json>,
    #[serde(default)]
    pub vary: Vec<String>,
    #[serde(default)]
    pub only: Vec<Json>,
    #[serde(default)]
    pub except: Vec<Json>,
    /// `false`: the testing tools do not run it.
    #[serde(default = "yes")]
    pub tool: bool,
}

fn yes() -> bool {
    true
}

/// A participant: a listener, or an emitter row a step creates.
#[derive(Debug, Clone, PartialEq)]
pub struct Participant {
    pub label: String,
    pub at: Location,
    /// The scope path; `root` for the root.
    pub scope: String,
    /// The participant as written.
    pub raw: Json,
}

impl Participant {
    fn parse(raw: &Json) -> Result<Self, String> {
        let label = raw["label"]
            .as_str()
            .ok_or_else(|| format!("a participant without a label: {raw}"))?
            .to_owned();
        let at = Location::parse(
            raw["at"]
                .as_str()
                .ok_or_else(|| format!("{label}: no `at`"))?,
        )?;
        let scope = raw["scope"].as_str().unwrap_or("root").to_owned();
        Ok(Self {
            label,
            at,
            scope,
            raw: raw.clone(),
        })
    }

    /// Whether it registers a listener (every participant of `listeners`
    /// and `load`; never an emitter row).
    pub fn listens(&self) -> bool {
        self.raw.get("does").is_some()
    }
}

/// What a dispatch step emits.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub from: Location,
    /// The emitter row's label (`e` unless written).
    pub row: String,
    pub scope: String,
    pub mode: String,
    pub name: Option<String>,
    pub args: Vec<Json>,
    pub filter: Option<Json>,
    pub via: Option<String>,
    /// A text (the terminal returns it) or `{ "echo": true }` (it returns
    /// its first argument, the event's first argument); logs `terminal`.
    pub terminal: Option<Json>,
    pub cancel: Option<String>,
    /// `{ "until": entry, "args": [..] }`: after its dispatch the emitter
    /// calls `events-log.until(entry)` (a synchronous host call that logs
    /// `until:<entry>` and returns once `entry` is logged), then emits the
    /// same name in the same mode again with these arguments. The frames
    /// that reach it during that wait are what the case is about.
    pub then: Option<Box<Then>>,
}

/// A spec's `then`.
#[derive(Debug, Clone, PartialEq)]
pub struct Then {
    pub until: String,
    pub args: Vec<Json>,
}

pub const MODES: [&str; 5] = ["emit", "parallel", "serial", "waterfall", "bail"];

impl Spec {
    fn parse(raw: &Json) -> Result<Self, String> {
        let known = [
            "from", "row", "scope", "mode", "name", "args", "filter", "via", "terminal", "cancel",
            "then",
        ];
        unknown_keys("emit", raw, &known)?;
        let mode = raw["mode"]
            .as_str()
            .ok_or_else(|| format!("an emit without a mode: {raw}"))?
            .to_owned();
        if !MODES.contains(&mode.as_str()) {
            return Err(format!("unknown mode {mode:?}"));
        }
        let filter = raw.get("filter").cloned();
        if let Some(filter) = &filter {
            match (filter.get("keep"), filter.get("throws")) {
                (Some(Json::Array(_)), None) | (None, Some(Json::Object(_))) => {}
                _ => {
                    return Err(format!(
                        "a filter is {{keep: [..]}} or {{throws: {{..}}}}: {filter}"
                    ))
                }
            }
        }
        Ok(Self {
            from: Location::parse(raw["from"].as_str().ok_or("an emit without `from`")?)?,
            row: raw["row"].as_str().unwrap_or("e").to_owned(),
            scope: raw["scope"].as_str().unwrap_or("root").to_owned(),
            mode,
            name: raw["name"].as_str().map(str::to_owned),
            args: raw["args"]
                .as_array()
                .ok_or_else(|| format!("an emit without args: {raw}"))?
                .clone(),
            filter,
            via: raw["via"].as_str().map(str::to_owned),
            terminal: match raw.get("terminal") {
                None | Some(Json::Null) => None,
                Some(text @ Json::String(_)) => Some(text.clone()),
                Some(echo) if *echo == json!({ "echo": true }) => Some(echo.clone()),
                Some(other) => {
                    return Err(format!(
                        "a terminal is a text or {{\"echo\": true}}: {other}"
                    ))
                }
            },
            cancel: raw
                .get("cancel")
                .map(|cancel| {
                    cancel["when"]
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| format!("cancel without `when`: {cancel}"))
                })
                .transpose()?,
            then: raw
                .get("then")
                .map(|then| -> Result<Box<Then>, String> {
                    unknown_keys("then", then, &["until", "args"])?;
                    Ok(Box::new(Then {
                        until: then["until"]
                            .as_str()
                            .ok_or_else(|| format!("then without `until`: {then}"))?
                            .to_owned(),
                        args: then["args"]
                            .as_array()
                            .ok_or_else(|| format!("then without args: {then}"))?
                            .clone(),
                    }))
                })
                .transpose()?,
        })
    }

    /// The spec as the emitter's `dispatch` receives it.
    pub fn wire(&self, case: &Case) -> Value {
        let mut record = BTreeMap::new();
        record.insert("mode".to_owned(), Value::Data(json!(self.mode)));
        record.insert(
            "name".to_owned(),
            Value::Data(json!(self.name.as_deref().unwrap_or(&case.name))),
        );
        record.insert(
            "args".to_owned(),
            Value::List(self.args.iter().map(argument).collect()),
        );
        if let Some(filter) = &self.filter {
            record.insert("filter".to_owned(), Value::Data(filter.clone()));
        }
        if let Some(via) = &self.via {
            record.insert("via".to_owned(), Value::Data(json!(via)));
        }
        if let Some(terminal) = &self.terminal {
            record.insert("terminal".to_owned(), Value::Data(terminal.clone()));
        }
        if let Some(when) = &self.cancel {
            record.insert("cancel".to_owned(), Value::Data(json!({ "when": when })));
        }
        if let Some(then) = &self.then {
            let mut inner = BTreeMap::new();
            inner.insert("until".to_owned(), Value::Data(json!(then.until)));
            inner.insert(
                "args".to_owned(),
                Value::List(then.args.iter().map(argument).collect()),
            );
            record.insert("then".to_owned(), Value::Record(inner));
        }
        Value::Record(record)
    }

    /// Whether the emitter's synchronous method dispatches it.
    pub fn sync(&self) -> bool {
        self.mode == "bail"
    }
}

/// One step of a case.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // a handful per case, read once
pub enum Step {
    Emit {
        spec: Spec,
        expect: Option<Json>,
        background: bool,
    },
    Load {
        participant: Participant,
        expect: Option<Json>,
    },
    Unload {
        label: String,
        when: Option<String>,
        /// An entry the runner logs once the row is gone (and, with
        /// `answered`, once the host answered its dispatches), before it
        /// joins the background dispatch: what an emitter's `then.until`
        /// waits for.
        then: Option<String>,
        expect: Option<Json>,
    },
    Update {
        label: String,
    },
    Kill {
        target: Location,
        when: Option<String>,
        expect: Option<Json>,
    },
    Restart {
        target: Location,
        expect: Option<Json>,
    },
    Concurrent {
        steps: Vec<Step>,
        hold: bool,
        expect: Option<Json>,
    },
}

/// The keys of a step's `expect` (the first [`STEP_EXPECTATIONS`]), then
/// the fields of `throws`.
pub const EXPECTATIONS: [&str; 15] = [
    "returns",
    "throws",
    "cancelled",
    "log",
    "logUnordered",
    "logContains",
    "logLacks",
    "reported",
    "filterSees",
    "viaHost",
    "settles",
    "answered",
    "why",
    "errors",
    "message",
];

/// How many of [`EXPECTATIONS`] are keys of `expect`.
pub const STEP_EXPECTATIONS: usize = 12;

impl Step {
    pub fn parse(raw: &Json) -> Result<Self, String> {
        let expect = raw.get("expect").cloned();
        if let Some(expect) = &expect {
            let Json::Object(fields) = expect else {
                return Err(format!("expect is an object: {expect}"));
            };
            for key in fields.keys() {
                if !EXPECTATIONS[..STEP_EXPECTATIONS].contains(&key.as_str()) {
                    return Err(format!("unknown expectation {key:?}"));
                }
            }
        }
        let when = raw["when"].as_str().map(str::to_owned);
        if let Some(spec) = raw.get("emit") {
            unknown_keys("step", raw, &["emit", "expect", "background"])?;
            return Ok(Step::Emit {
                spec: Spec::parse(spec)?,
                expect,
                background: raw["background"].as_bool().unwrap_or(false),
            });
        }
        if let Some(participant) = raw.get("load") {
            unknown_keys("step", raw, &["load", "expect"])?;
            return Ok(Step::Load {
                participant: Participant::parse(participant)?,
                expect,
            });
        }
        if let Some(label) = raw.get("unload") {
            unknown_keys("step", raw, &["unload", "when", "then", "expect"])?;
            return Ok(Step::Unload {
                label: label.as_str().ok_or("unload names a label")?.to_owned(),
                when,
                then: raw["then"].as_str().map(str::to_owned),
                expect,
            });
        }
        if let Some(label) = raw.get("update") {
            unknown_keys("step", raw, &["update"])?;
            return Ok(Step::Update {
                label: label.as_str().ok_or("update names a label")?.to_owned(),
            });
        }
        if let Some(target) = raw.get("kill") {
            unknown_keys("step", raw, &["kill", "when", "expect"])?;
            return Ok(Step::Kill {
                target: kill_target(target)?,
                when,
                expect,
            });
        }
        if let Some(target) = raw.get("restart") {
            unknown_keys("step", raw, &["restart", "expect"])?;
            return Ok(Step::Restart {
                target: kill_target(target)?,
                expect,
            });
        }
        if let Some(steps) = raw.get("concurrent") {
            unknown_keys("step", raw, &["concurrent", "hold", "expect"])?;
            let steps = steps
                .as_array()
                .ok_or("concurrent lists steps")?
                .iter()
                .map(Step::parse)
                .collect::<Result<Vec<_>, _>>()?;
            if !steps.iter().all(|step| matches!(step, Step::Emit { .. })) {
                return Err("concurrent steps are emits".into());
            }
            return Ok(Step::Concurrent {
                steps,
                hold: raw["hold"].as_bool().unwrap_or(false),
                expect,
            });
        }
        Err(format!("unknown step {raw}"))
    }

    pub fn expect(&self) -> Option<&Json> {
        match self {
            Step::Emit { expect, .. }
            | Step::Load { expect, .. }
            | Step::Unload { expect, .. }
            | Step::Kill { expect, .. }
            | Step::Restart { expect, .. }
            | Step::Concurrent { expect, .. } => expect.as_ref(),
            Step::Update { .. } => None,
        }
    }

    /// The emits of this step (itself, or the concurrent ones).
    pub fn emits(&self) -> Vec<&Spec> {
        match self {
            Step::Emit { spec, .. } => vec![spec],
            Step::Concurrent { steps, .. } => steps.iter().flat_map(Step::emits).collect(),
            _ => Vec::new(),
        }
    }
}

fn kill_target(raw: &Json) -> Result<Location, String> {
    match raw.as_str().map(Location::parse) {
        Some(Ok(target @ (Location::Y | Location::Node))) => Ok(target),
        _ => Err(format!("kill and restart take \"Y\" or \"node\": {raw}")),
    }
}

fn unknown_keys(what: &str, raw: &Json, known: &[&str]) -> Result<(), String> {
    let Json::Object(fields) = raw else {
        return Err(format!("{what} is an object: {raw}"));
    };
    match fields.keys().find(|key| !known.contains(&key.as_str())) {
        Some(key) => Err(format!("{what}: unknown field {key:?}")),
        None => Ok(()),
    }
}

impl Case {
    pub fn participants(&self) -> Result<Vec<Participant>, String> {
        self.listeners.iter().map(Participant::parse).collect()
    }

    pub fn parsed_steps(&self) -> Result<Vec<Step>, String> {
        self.steps.iter().map(Step::parse).collect()
    }

    /// Every label the case knows, with where it first is: listeners,
    /// emitter rows and rows loaded by steps.
    pub fn places(&self) -> Result<BTreeMap<String, (Location, String)>, String> {
        let mut places = BTreeMap::new();
        for participant in self.participants()? {
            if places
                .insert(
                    participant.label.clone(),
                    (participant.at, participant.scope.clone()),
                )
                .is_some()
            {
                return Err(format!("label {} twice", participant.label));
            }
        }
        for step in self.parsed_steps()? {
            if let Step::Load { participant, .. } = &step {
                if places
                    .insert(
                        participant.label.clone(),
                        (participant.at, participant.scope.clone()),
                    )
                    .is_some()
                {
                    return Err(format!("label {} twice", participant.label));
                }
            }
            for spec in step.emits() {
                places
                    .entry(spec.row.clone())
                    .or_insert((spec.from, spec.scope.clone()));
            }
        }
        Ok(places)
    }

    /// Whether every emit of the case is synchronous (listeners then
    /// default to `sync`).
    fn all_sync(&self) -> bool {
        let steps = self.parsed_steps().unwrap_or_default();
        let emits: Vec<&Spec> = steps.iter().flat_map(Step::emits).collect();
        !emits.is_empty() && emits.iter().all(|spec| spec.sync())
    }

    /// The scopes a label is used in: its own, and every emit's from its row.
    fn scopes_of(&self, label: &str) -> BTreeSet<String> {
        let mut scopes = BTreeSet::new();
        for participant in self.participants().unwrap_or_default() {
            if participant.label == label {
                scopes.insert(participant.scope);
            }
        }
        for step in self.parsed_steps().unwrap_or_default() {
            if let Step::Load { participant, .. } = &step {
                if participant.label == label {
                    scopes.insert(participant.scope.clone());
                }
            }
            for spec in step.emits() {
                if spec.row == label {
                    scopes.insert(spec.scope.clone());
                }
            }
        }
        scopes
    }
}

// ── Placements ──────────────────────────────────────────────────

/// One way to run a case: the languages of X and Y, and where each label
/// is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    pub x: String,
    pub y: String,
    pub at: BTreeMap<String, Location>,
}

impl Combo {
    /// The language of a location.
    pub fn language(&self, at: Location) -> &str {
        match at {
            Location::X => &self.x,
            Location::Y => &self.y,
            Location::Host | Location::Node => "rust",
        }
    }

    /// The language of a label (or of `X` / `Y`).
    pub fn language_of(&self, label: &str) -> Option<&str> {
        match label {
            "X" => Some(&self.x),
            "Y" => Some(&self.y),
            _ => self.at.get(label).map(|at| self.language(*at)),
        }
    }

    pub fn uses(&self, at: Location) -> bool {
        self.at.values().any(|placed| *placed == at)
    }

    /// `X=python Y=go a@X b@host e@Y`.
    pub fn describe(&self) -> String {
        let at: Vec<String> = self
            .at
            .iter()
            .map(|(label, at)| format!("{label}@{}", at.as_str()))
            .collect();
        format!("X={} Y={} {}", self.x, self.y, at.join(" "))
    }
}

/// Whether a limit (`only` / `except` entry) holds in `combo`: every label
/// it names is of one of its languages.
fn limit_holds(limit: &Json, combo: &Combo) -> bool {
    let Json::Object(fields) = limit else {
        return false;
    };
    fields
        .iter()
        .filter(|(key, _)| *key != "why")
        .all(|(label, languages)| {
            let Some(language) = combo.language_of(label) else {
                return false;
            };
            languages
                .as_array()
                .is_some_and(|all| all.iter().any(|l| l.as_str() == Some(language)))
        })
}

/// The placements of `case` with X in `x`, for each language Y may be in
/// (`ys`, X's own first): every combination of the `vary` labels over the
/// locations (§13.2), less those `only` / `except` rule out. A placement
/// that puts nothing in Y runs only with Y in X's language.
pub fn combinations(case: &Case, x: &str, ys: &[&str]) -> Result<Vec<Combo>, String> {
    let places = case.places()?;
    let mut base: BTreeMap<String, Location> = places
        .iter()
        .map(|(label, (at, _))| (label.clone(), *at))
        .collect();
    for label in &case.vary {
        if !base.contains_key(label) {
            return Err(format!("vary names {label}, which is not in the case"));
        }
    }
    let mut assignments = vec![base.clone()];
    for label in &case.vary {
        let rooted = case.scopes_of(label).iter().all(|scope| scope == "root");
        let mut next = Vec::new();
        for assignment in &assignments {
            for at in Location::ALL {
                if at == Location::Node && !rooted {
                    continue;
                }
                let mut assignment = assignment.clone();
                assignment.insert(label.clone(), at);
                next.push(assignment);
            }
        }
        assignments = next;
    }
    // The written placement first, as §13.2 says.
    base.clear();
    let mut combos = Vec::new();
    for y in ys {
        for at in &assignments {
            let combo = Combo {
                x: x.to_owned(),
                y: (*y).to_owned(),
                at: at.clone(),
            };
            let y_used = combo.uses(Location::Y)
                || case.parsed_steps()?.iter().any(|step| {
                    matches!(
                        step,
                        Step::Kill {
                            target: Location::Y,
                            ..
                        } | Step::Restart {
                            target: Location::Y,
                            ..
                        }
                    )
                });
            if !y_used && *y != ys[0] {
                continue;
            }
            if !case.only.iter().all(|limit| limit_holds(limit, &combo)) {
                continue;
            }
            if case.except.iter().any(|limit| limit_holds(limit, &combo)) {
                continue;
            }
            if !combos.contains(&combo) {
                combos.push(combo);
            }
        }
    }
    Ok(combos)
}

/// How many of a case's placements a run takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// The placement as written, for each Y language.
    Written,
    /// With Y in X's language: the written placement, each varied label
    /// moved alone to every other location, and every varied label at one
    /// location; with Y in another language, the written placement. Every
    /// participant meets every location, and every language meets X.
    Sampled,
    /// Every combination (§13.2): about 22 000 placements per language.
    All,
}

impl Coverage {
    /// From `RUTIS_EVENTS_PLACEMENTS` (`written`, `sampled`, `all`);
    /// `sampled` when unset.
    pub fn from_env() -> Self {
        match std::env::var("RUTIS_EVENTS_PLACEMENTS").as_deref() {
            Ok("written") => Coverage::Written,
            Ok("all") => Coverage::All,
            _ => Coverage::Sampled,
        }
    }
}

/// The placements of `case` a run with `coverage` takes, out of
/// [`combinations`].
pub fn placements(
    case: &Case,
    x: &str,
    ys: &[&str],
    coverage: Coverage,
) -> Result<Vec<Combo>, String> {
    let all = combinations(case, x, ys)?;
    if coverage == Coverage::All {
        return Ok(all);
    }
    let written: BTreeMap<String, Location> = case
        .places()?
        .into_iter()
        .map(|(label, (at, _))| (label, at))
        .collect();
    let moved = |combo: &Combo| {
        case.vary
            .iter()
            .filter(|label| combo.at.get(*label) != written.get(*label))
            .count()
    };
    let together = |combo: &Combo| {
        let places: BTreeSet<Location> = case
            .vary
            .iter()
            .filter_map(|l| combo.at.get(l))
            .copied()
            .collect();
        places.len() == 1
    };
    Ok(all
        .into_iter()
        .filter(|combo| match coverage {
            Coverage::Written => moved(combo) == 0,
            _ if combo.y != x => moved(combo) == 0,
            _ => moved(combo) <= 1 || together(combo),
        })
        .collect())
}

/// A row to load: a participant placed by a combination, its config
/// resolved (§13.2 defaults).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    pub at: Location,
    pub language: String,
    pub scope: String,
    /// The row's config: the participant without `at` and `scope`, with
    /// `name`, `kind` and `log` filled in; or `{label, emitter: true}`.
    pub config: Json,
}

impl Row {
    /// The listener row of `participant` in `combo`.
    pub fn listener(case: &Case, combo: &Combo, participant: &Participant) -> Row {
        let at = combo
            .at
            .get(&participant.label)
            .copied()
            .unwrap_or(participant.at);
        let mut config = participant.raw.clone();
        let fields = config.as_object_mut().expect("a participant is an object");
        fields.remove("at");
        fields.remove("scope");
        fields.entry("name").or_insert_with(|| json!(case.name));
        let kind = if case.all_sync() { "sync" } else { "async" };
        fields.entry("kind").or_insert_with(|| json!(kind));
        fields.entry("log").or_insert_with(|| json!("label"));
        Row {
            label: participant.label.clone(),
            at,
            language: combo.language(at).to_owned(),
            scope: participant.scope.clone(),
            config,
        }
    }

    /// The emitter row `label` at `at` in `scope`.
    pub fn emitter(combo: &Combo, label: &str, at: Location, scope: &str) -> Row {
        Row {
            label: label.to_owned(),
            at,
            language: combo.language(at).to_owned(),
            scope: scope.to_owned(),
            config: json!({ "label": label, "emitter": true }),
        }
    }
}

/// The listener rows of `case` in `combo`, in the order they register.
pub fn rows(case: &Case, combo: &Combo) -> Result<Vec<Row>, String> {
    Ok(case
        .participants()?
        .iter()
        .map(|participant| Row::listener(case, combo, participant))
        .collect())
}

// ── Values as the file writes them ──────────────────────────────

/// An argument as the file writes it: data, a tagged value
/// (`{"$bytes": "aGVsbG8="}`), or `{"$callback": "upper"}`, a function
/// returning its argument upper-cased. Panics on a tagged value it cannot
/// read (an unknown `$` tag, or a text its tag does not parse): a typo in
/// the file must not turn into a record that some case then passes with.
pub fn argument(arg: &Json) -> Value {
    match arg {
        Json::Array(items) => Value::List(items.iter().map(argument).collect()),
        Json::Object(fields) if fields.len() == 1 => {
            let (key, inner) = fields.iter().next().unwrap();
            match tagged(key, inner) {
                Some(value) => value,
                None if key.starts_with('$') => {
                    panic!("events.expected.json: a bad tagged value {arg}")
                }
                None => Value::Record(
                    fields
                        .iter()
                        .map(|(key, value)| (key.clone(), argument(value)))
                        .collect(),
                ),
            }
        }
        Json::Object(fields) => Value::Record(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), argument(value)))
                .collect(),
        ),
        other => Value::Data(other.clone()),
    }
}

fn tagged(key: &str, inner: &Json) -> Option<Value> {
    let text = inner.as_str();
    Some(match key {
        "$callback" => {
            assert_eq!(inner, "upper", "the only callback is \"upper\"");
            Value::callback(|args| {
                let [text]: [String; 1] = crate::session::decode_value(args)?;
                Ok(json!(text.to_uppercase()).into())
            })
        }
        "$instant" => Value::Instant(text?.parse().ok()?),
        "$datetime" => Value::DateTime(text?.parse().ok()?),
        "$date" => Value::Date(text?.parse().ok()?),
        "$time" => Value::Time(text?.parse().ok()?),
        "$bigint" => Value::BigInt(text?.parse().ok()?),
        "$decimal" => Value::Decimal(text?.parse().ok()?),
        "$uuid" => Value::Uuid(text?.parse().ok()?),
        "$bytes" => Value::Bytes(crate::session::values::base64_decode(text?).ok()?),
        "$map" => Value::Map(
            inner
                .as_array()?
                .iter()
                .map(|entry| Some((argument(entry.get(0)?), argument(entry.get(1)?))))
                .collect::<Option<_>>()?,
        ),
        "$set" => Value::Set(inner.as_array()?.iter().map(argument).collect()),
        _ => return None,
    })
}

/// A value as the file writes it: data as JSON, a tagged value as
/// `{"$<tag>": <its wire text>}`.
pub fn written(value: Value) -> Result<Json, Error> {
    let tagged = |tag: &str, value: Json| json!({ format!("${tag}"): value });
    Ok(match value {
        Value::List(values) => {
            Json::Array(values.into_iter().map(written).collect::<Result<_, _>>()?)
        }
        Value::Record(fields) => Json::Object(
            fields
                .into_iter()
                .map(|(key, value)| Ok((key, written(value)?)))
                .collect::<Result<_, Error>>()?,
        ),
        Value::Bytes(bytes) => tagged(
            "bytes",
            json!(crate::session::values::base64_encode(&bytes)),
        ),
        Value::Map(entries) => tagged(
            "map",
            Json::Array(
                entries
                    .into_iter()
                    .map(|(key, value)| Ok(json!([written(key)?, written(value)?])))
                    .collect::<Result<_, Error>>()?,
            ),
        ),
        Value::Set(elements) => tagged(
            "set",
            Json::Array(
                elements
                    .into_iter()
                    .map(written)
                    .collect::<Result<_, _>>()?,
            ),
        ),
        Value::Instant(value) => tagged("instant", json!(value.to_string())),
        Value::DateTime(value) => tagged("datetime", json!(value.to_string())),
        Value::Date(value) => tagged("date", json!(value.to_string())),
        Value::Time(value) => tagged("time", json!(value.to_string())),
        Value::BigInt(value) => tagged("bigint", json!(value.to_string())),
        Value::Decimal(value) => tagged("decimal", json!(value.to_string())),
        Value::Uuid(value) => tagged("uuid", json!(value.to_string())),
        other => other.json()?,
    })
}

/// An error's name and message, as an emitter's caller sees them: a
/// failure of the transport is a `BindingError` (§11).
pub fn named(error: &Error) -> (String, String) {
    match error {
        Error::Remote { name, message, .. } => (name.clone(), message.clone()),
        Error::SyncWaitCycle(message) => ("SyncWaitCycle".into(), message.clone()),
        other => ("BindingError".into(), other.to_string()),
    }
}

/// The children of an `AggregateError`, from its graph, as (name, message).
pub fn aggregated(error: &Error) -> Option<Vec<(String, String)>> {
    let Error::Remote {
        graph: Some(graph), ..
    } = error
    else {
        return None;
    };
    let nodes = graph.get("nodes")?.as_array()?;
    let node = |reference: &Json| -> Option<&Json> {
        (reference.get("type")?.as_str()? == "reference")
            .then(|| nodes.get(reference.get("value")?.as_u64()? as usize))?
    };
    let root = node(graph.get("root")?)?;
    root.get("errors")?
        .as_array()?
        .iter()
        .map(|child| {
            let child = node(child)?;
            Some((
                child.get("name")?.as_str()?.to_owned(),
                child
                    .get("message")?
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            ))
        })
        .collect()
}

// ── The log ─────────────────────────────────────────────────────

#[derive(Default)]
struct LogState {
    entries: Vec<String>,
    /// Where each entry was sent from, when a session's invocation added
    /// it: that session's tag and the invocation's sequence.
    senders: Vec<Option<(String, u64)>>,
    /// The descriptions the filters saw, in order.
    seen: Vec<Json>,
}

/// The host service `events-log` every `conformance-events` row injects.
#[derive(Default)]
pub struct EventsLog {
    state: Mutex<LogState>,
    changed: tokio::sync::Notify,
    /// For the synchronous `until`, which blocks its caller's thread.
    added: std::sync::Condvar,
}

impl EventsLog {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    pub fn add(&self, entry: impl Into<String>) {
        self.add_from(None, entry.into());
    }

    /// Log `entry`, sent by the invocation `sender`. The host runs one
    /// session's invocations concurrently, so they can arrive here out of
    /// the order the far end sent them in (a reentrant runtime runs the
    /// next listener while one waits in `add`): an entry goes before the
    /// entries the same session sent after it.
    fn add_from(&self, sender: Option<(String, u64)>, entry: String) {
        {
            let mut state = self.state.lock().unwrap();
            let at = sender
                .as_ref()
                .and_then(|(tag, sequence)| {
                    state.senders.iter().position(|logged| {
                        matches!(logged, Some((other, later)) if other == tag && later > sequence)
                    })
                })
                .unwrap_or(state.entries.len());
            state.entries.insert(at, entry);
            state.senders.insert(at, sender);
        }
        self.changed.notify_waiters();
        self.added.notify_all();
    }

    /// What `events-log.filter(description)` does: log `filter:<row>` and
    /// keep the description.
    pub fn filter_description(&self, description: Json) {
        self.filter_from(None, description);
    }

    fn filter_from(&self, sender: Option<(String, u64)>, description: Json) {
        let row = description["row"].as_str().unwrap_or("null").to_owned();
        self.state.lock().unwrap().seen.push(description);
        self.add_from(sender, format!("filter:{row}"));
    }

    pub fn entries(&self) -> Vec<String> {
        self.state.lock().unwrap().entries.clone()
    }

    pub fn seen(&self) -> Vec<Json> {
        self.state.lock().unwrap().seen.clone()
    }

    pub fn clear(&self) {
        *self.state.lock().unwrap() = LogState::default();
    }

    /// Wait until `entry` is logged, at most `within`.
    pub async fn wait(&self, entry: &str, within: Duration) -> Result<(), String> {
        tokio::time::timeout(within, async {
            loop {
                let changed = self.changed.notified();
                if self.entries().iter().any(|logged| logged == entry) {
                    return;
                }
                changed.await;
            }
        })
        .await
        .map_err(|_| {
            format!(
                "{entry:?} was not logged within {within:?}: {:?}",
                self.entries()
            )
        })
    }

    /// What `events-log.until(entry)` does: log `until:<entry>`, then block
    /// the calling thread until `entry` is logged, at most `within`.
    pub fn until(&self, entry: &str, within: Duration) -> Result<(), String> {
        self.add(format!("until:{entry}"));
        let state = self.state.lock().unwrap();
        let (state, timeout) = self
            .added
            .wait_timeout_while(state, within, |state| {
                !state.entries.iter().any(|logged| logged == entry)
            })
            .unwrap();
        if timeout.timed_out() {
            return Err(format!(
                "{entry:?} was not logged within {within:?}: {:?}",
                state.entries
            ));
        }
        Ok(())
    }

    /// The methods as the plugins see them.
    pub fn methods() -> Json {
        json!({ "add": "sync", "filter": "sync", "wait": "async", "until": "sync" })
    }
}

/// `Arc<EventsLog>` as the host service.
pub struct EventsLogService(pub Arc<EventsLog>);

impl HostDispatch for EventsLogService {
    fn invoke(&self, method: &str, args: Value) -> Reply {
        let mut args = args.list()?.into_iter();
        let first = args.next().unwrap_or(Value::Undefined);
        match method {
            "add" => {
                let entry: String = crate::session::decode_value(first)?;
                self.0.add_from(crate::session::rpc::invocation(), entry);
                Ok(Value::Undefined)
            }
            "filter" => {
                self.0
                    .filter_from(crate::session::rpc::invocation(), written(first)?);
                Ok(Value::Undefined)
            }
            "wait" => {
                let entry: String = crate::session::decode_value(first)?;
                let log = self.0.clone();
                Ok(Value::future(async move {
                    log.wait(&entry, PATIENCE)
                        .await
                        .map(|()| Value::Undefined)
                        .map_err(|message| Error::Remote {
                            name: "Timeout".into(),
                            message,
                            graph: None,
                        })
                }))
            }
            "until" => {
                let entry: String = crate::session::decode_value(first)?;
                // A synchronous call: the caller's thread waits, and so
                // does this one (outside the executor's own workers when
                // it can).
                let log = self.0.clone();
                let wait = move || log.until(&entry, PATIENCE);
                let waited = match tokio::runtime::Handle::try_current() {
                    Ok(handle)
                        if handle.runtime_flavor()
                            == tokio::runtime::RuntimeFlavor::MultiThread =>
                    {
                        tokio::task::block_in_place(wait)
                    }
                    _ => wait(),
                };
                waited
                    .map(|()| Value::Undefined)
                    .map_err(|message| Error::Remote {
                        name: "Timeout".into(),
                        message,
                        graph: None,
                    })
            }
            other => Err(Error::Value(format!("events-log has no method {other}"))),
        }
    }

    fn methods(&self) -> Option<Json> {
        Some(EventsLog::methods())
    }
}

// ── The world a case runs in ────────────────────────────────────

/// Where the rows of a case live. Every method may fail with a message
/// that becomes the case's failure.
pub trait World: Send + Sync {
    /// Load `row` and wait until it runs (or, with `fails`, failed): its
    /// listener is on the bus, its emitter service available.
    fn load<'a>(&'a self, row: &'a Row) -> BoxFuture<'a, Result<(), String>>;

    /// Unload the row `label` and wait until it is gone.
    fn unload<'a>(&'a self, label: &'a str) -> BoxFuture<'a, Result<(), String>>;

    /// Change the row's config (so it registers again) and wait until it
    /// runs again.
    fn update<'a>(&'a self, label: &'a str) -> BoxFuture<'a, Result<(), String>>;

    /// The emitter service of the row `label`, once it is loaded.
    fn emitter(&self, label: &str) -> Option<Arc<dyn HostDispatch>>;

    /// End the Y runtime processes or the link to the node.
    fn kill<'a>(&'a self, target: Location) -> BoxFuture<'a, Result<(), String>>;

    /// Bring back what `kill` ended; its rows load again.
    fn restart<'a>(&'a self, target: Location) -> BoxFuture<'a, Result<(), String>>;

    /// Unload every row of the case and bring back what it killed.
    fn reset(&self) -> BoxFuture<'_, Result<(), String>>;

    /// How many `events.dispatch` the host received so far, if the world
    /// can tell (`viaHost`).
    fn dispatches(&self) -> Option<u64> {
        None
    }

    /// Wait until the host has written its reply to every `events.dispatch`
    /// it received (`answered`): the frames the suite sends afterwards
    /// reach each emitter after those replies.
    fn answered(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Err("answered: this world cannot see the host's replies".into()) })
    }

    /// Hold the next `n` dispatches at the host until all arrived (`hold`).
    fn hold(&self, _n: usize) -> Result<(), String> {
        Err("this world cannot hold dispatches".into())
    }

    /// The emit failures reported so far (§5.6 lines), if the world sees
    /// them (`reported`).
    fn reported(&self) -> Option<Vec<String>> {
        None
    }

    /// What the description of the row `label` must say, as far as the
    /// world knows (`filterSees`, every field it gives compared): as the
    /// row's own process sees it, `name`, `prefix`, `plugin`, `row`,
    /// `runtime`, `node`, `scope` (the chain, `[{id, group}]`) and
    /// `global`. Seen from the other node, `node` and `scope` are the
    /// link's.
    fn description(&self, _label: &str) -> Option<Json> {
        None
    }
}

/// A dispatch's result: the emitter's reply, or that it did not settle.
type Outcome = Result<Reply, String>;

/// Run `case` in `combo`: load its listeners in order, run its steps,
/// check each expectation, and leave the world as it was.
pub async fn run(
    world: &dyn World,
    log: &Arc<EventsLog>,
    case: &Case,
    combo: &Combo,
) -> Result<(), String> {
    let result = run_keeping(world, log, case, combo).await;
    let reset = finish(world, log).await;
    result.and(reset.map_err(|error| format!("reset: {error}")))
}

/// [`run`] without the reset: the world stays as the case left it, for a
/// failure's record; [`finish`] resets it.
pub async fn run_keeping(
    world: &dyn World,
    log: &Arc<EventsLog>,
    case: &Case,
    combo: &Combo,
) -> Result<(), String> {
    log.clear();
    run_steps(world, log, case, combo).await
}

/// Reset the world after [`run_keeping`].
pub async fn finish(world: &dyn World, log: &Arc<EventsLog>) -> Result<(), String> {
    let reset = world.reset().await;
    log.clear();
    reset
}

async fn run_steps(
    world: &dyn World,
    log: &Arc<EventsLog>,
    case: &Case,
    combo: &Combo,
) -> Result<(), String> {
    for row in rows(case, combo)? {
        world
            .load(&row)
            .await
            .map_err(|error| format!("loading {}: {error}", row.label))?;
    }
    // Emitter rows: label → (location, scope) loaded now.
    let mut emitters: BTreeMap<String, (Location, String)> = BTreeMap::new();
    let mut pending: Option<tokio::task::JoinHandle<Outcome>> = None;
    for (i, step) in case.parsed_steps()?.iter().enumerate() {
        let what = |error: String| format!("step {i} ({}): {error}", step_name(step));
        run_step(world, log, case, combo, step, &mut emitters, &mut pending)
            .await
            .map_err(what)?;
    }
    if let Some(pending) = pending {
        pending.abort();
    }
    Ok(())
}

fn step_name(step: &Step) -> String {
    match step {
        Step::Emit { spec, .. } => format!("{} from {}", spec.mode, spec.row),
        Step::Load { participant, .. } => format!("load {}", participant.label),
        Step::Unload { label, .. } => format!("unload {label}"),
        Step::Update { label } => format!("update {label}"),
        Step::Kill { target, .. } => format!("kill {}", target.as_str()),
        Step::Restart { target, .. } => format!("restart {}", target.as_str()),
        Step::Concurrent { .. } => "concurrent".into(),
    }
}

async fn run_step(
    world: &dyn World,
    log: &Arc<EventsLog>,
    case: &Case,
    combo: &Combo,
    step: &Step,
    emitters: &mut BTreeMap<String, (Location, String)>,
    pending: &mut Option<tokio::task::JoinHandle<Outcome>>,
) -> Result<(), String> {
    let empty = Json::Object(Default::default());
    let expect = step.expect().unwrap_or(&empty);
    let settles = expect["settles"].as_bool().unwrap_or(false);
    let before = world.dispatches();
    let mut outcomes: Vec<Outcome> = Vec::new();
    // Where the step's emitter is: the descriptions its filter sees are
    // this process's view (an entry of another node is the link's).
    let mut viewer = None;
    match step {
        Step::Emit {
            spec, background, ..
        } => {
            viewer = Some(combo.at.get(&spec.row).copied().unwrap_or(spec.from));
            let emitter = emitter_for(world, combo, spec, emitters).await?;
            let call = dispatch(emitter, spec.clone(), case.clone());
            if *background {
                *pending = Some(tokio::spawn(call));
                return check_log(world, log, expect, viewer, &event_of(case, step)).await;
            }
            outcomes.push(within(call, settles).await);
        }
        Step::Load { participant, .. } => {
            let row = Row::listener(case, combo, participant);
            world.load(&row).await?;
        }
        Step::Unload {
            label, when, then, ..
        } => {
            if let Some(when) = when {
                log.wait(when, scaled(SETTLE)).await?;
            }
            world.unload(label).await?;
            if let Some(then) = then {
                if expect.get("answered").and_then(Json::as_bool) == Some(true) {
                    world.answered().await?;
                }
                log.add(then.clone());
            }
            if let Some(handle) = pending.take() {
                outcomes.push(joined(handle, settles).await);
            }
        }
        Step::Update { label } => world.update(label).await?,
        Step::Kill { target, when, .. } => {
            if let Some(when) = when {
                log.wait(when, scaled(SETTLE)).await?;
            }
            world.kill(*target).await?;
            if let Some(handle) = pending.take() {
                outcomes.push(joined(handle, settles).await);
            }
        }
        Step::Restart { target, .. } => world.restart(*target).await?,
        Step::Concurrent { steps, hold, .. } => {
            if *hold {
                world.hold(steps.len())?;
            }
            let mut calls = Vec::new();
            for step in steps {
                let Step::Emit { spec, .. } = step else {
                    unreachable!("parsed")
                };
                let emitter = emitter_for(world, combo, spec, emitters).await?;
                calls.push(tokio::spawn(dispatch(emitter, spec.clone(), case.clone())));
            }
            for call in calls {
                outcomes.push(joined(call, settles).await);
            }
        }
    }
    // The language whose cancellation a `cancelled` dispatch ends with:
    // its emitter's (the emits of the step, or the background one's).
    let emitter_language = step
        .emits()
        .first()
        .map(|spec| combo.language(combo.at.get(&spec.row).copied().unwrap_or(spec.from)))
        .map(str::to_owned);
    check_outcomes(expect, &outcomes, emitter_language.as_deref())?;
    if let Some(via) = expect.get("viaHost").and_then(Json::as_bool) {
        let (Some(before), Some(after)) = (before, world.dispatches()) else {
            return Err("viaHost: this world cannot see the host's dispatches".into());
        };
        if (after > before) != via {
            return Err(format!(
                "viaHost: expected {via}, {} events.dispatch arrived",
                after - before
            ));
        }
    }
    // The emits through the host are answered before the next step: an
    // emitter takes its replies in frame order (§5.5), so its next emit of
    // the name may take the fast path again.
    if expect.get("answered").and_then(Json::as_bool) == Some(true) {
        world.answered().await?;
    }
    check_log(world, log, expect, viewer, &event_of(case, step)).await
}

/// The event a step's reports are about: its emit's name, else the case's.
fn event_of(case: &Case, step: &Step) -> String {
    step.emits()
        .first()
        .and_then(|spec| spec.name.clone())
        .unwrap_or_else(|| case.name.clone())
}

/// The emitter row `spec` needs, loaded where `combo` puts it: at most one
/// row per label at a time.
async fn emitter_for(
    world: &dyn World,
    combo: &Combo,
    spec: &Spec,
    emitters: &mut BTreeMap<String, (Location, String)>,
) -> Result<Arc<dyn HostDispatch>, String> {
    let at = combo.at.get(&spec.row).copied().unwrap_or(spec.from);
    let place = (at, spec.scope.clone());
    if emitters.get(&spec.row) != Some(&place) {
        if emitters.remove(&spec.row).is_some() {
            world.unload(&spec.row).await?;
        }
        let row = Row::emitter(combo, &spec.row, at, &spec.scope);
        world.load(&row).await?;
        emitters.insert(spec.row.clone(), place);
    }
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        if let Some(emitter) = world.emitter(&spec.row) {
            return Ok(emitter);
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("no emitter-{} after loading it", spec.row));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Call the emitter: `dispatch_sync` on a blocking thread for the
/// synchronous modes, `dispatch` otherwise.
async fn dispatch(emitter: Arc<dyn HostDispatch>, spec: Spec, case: Case) -> Outcome {
    let args = Value::List(vec![spec.wire(&case)]);
    if spec.sync() {
        // Go's emitter names its sync method on the wire `dispatchSync`.
        let method = match emitter.methods() {
            Some(methods) if methods.get("dispatchSync").is_some() => "dispatchSync",
            _ => "dispatch_sync",
        };
        let reply = tokio::task::spawn_blocking(move || emitter.invoke(method, args))
            .await
            .map_err(|error| format!("dispatch_sync panicked: {error}"))?;
        // Go's is announced, so async on the wire: its answer is a future.
        Ok(match reply {
            Ok(value) => settle(value).await,
            Err(error) => Err(error),
        })
    } else {
        Ok(match emitter.invoke("dispatch", args) {
            Ok(value) => settle(value).await,
            Err(error) => Err(error),
        })
    }
}

async fn within(call: impl std::future::Future<Output = Outcome>, settles: bool) -> Outcome {
    let limit = if settles { scaled(SETTLE) } else { PATIENCE };
    tokio::time::timeout(limit, call)
        .await
        .unwrap_or_else(|_| Err(format!("the dispatch did not settle within {limit:?}")))
}

async fn joined(handle: tokio::task::JoinHandle<Outcome>, settles: bool) -> Outcome {
    within(
        async move { handle.await.map_err(|e| e.to_string())? },
        settles,
    )
    .await
}

/// The name a dispatch given up by its emitter ends with, by the
/// emitter's language (§5.7: "the emitter's call ends with its own
/// language's cancellation"): Python's `asyncio.CancelledError`, Go's
/// `context.Canceled` (an `Error` saying `context canceled` on the wire),
/// Kotlin's `CancellationException`, and for Rust (a dropped future ends
/// with nothing) the fixture's own `Cancelled`.
/// JavaScript cannot give up a dispatch.
pub fn cancellation(language: &str) -> Option<Json> {
    match language {
        "python" => Some(json!({ "name": "CancelledError" })),
        // A plain Go error crosses named `Error`.
        "go" => Some(json!({ "name": "Error", "message": "context canceled" })),
        // A cancelled coroutine's CancellationException.
        "kotlin" => Some(json!({ "name": "CancellationException" })),
        // Swift's own: a cancelled Task throws CancellationError.
        "swift" => Some(json!({ "name": "CancellationError" })),
        "rust" => Some(json!({ "name": "Cancelled" })),
        _ => None,
    }
}

/// `returns`, `throws`, `cancelled` and `settles` against the dispatches
/// of a step. Without `throws` or `cancelled` every dispatch that ends in
/// the step must succeed, whatever else the step expects.
fn check_outcomes(
    expect: &Json,
    outcomes: &[Outcome],
    emitter_language: Option<&str>,
) -> Result<(), String> {
    let cancelled = expect["cancelled"].as_bool().unwrap_or(false);
    let throws = match cancelled {
        true => {
            let language = emitter_language.unwrap_or("none");
            Some(cancellation(language).ok_or_else(|| {
                format!("cancelled: an emitter in {language} cannot give up a dispatch")
            })?)
        }
        false => expect.get("throws").cloned(),
    };
    let wants = expect.get("returns").is_some() || throws.is_some();
    if wants && outcomes.is_empty() {
        return Err("expects a result, but no dispatch ends in this step".into());
    }
    let mut replies = Vec::new();
    for outcome in outcomes {
        match outcome {
            Ok(reply) => replies.push(reply),
            Err(unsettled) => return Err(unsettled.clone()),
        }
    }
    if throws.is_none() {
        for reply in &replies {
            if let Err(error) = reply {
                let (name, message) = named(error);
                return Err(format!(
                    "expected the dispatch to succeed, it threw {name}: {message}"
                ));
            }
        }
    }
    if let Some(returns) = expect.get("returns") {
        for reply in &replies {
            let value = match reply {
                Ok(value) => written(value.clone()).map_err(|e| format!("not data: {e}"))?,
                Err(error) => return Err(format!("expected {returns}, got {error}")),
            };
            if &value != returns {
                return Err(format!("expected {returns}, got {value}"));
            }
        }
    }
    if let Some(throws) = &throws {
        // One thrown is enough when several dispatches ran (`concurrent`).
        let mut last = String::new();
        let matched = replies.iter().any(|reply| match reply {
            Ok(value) => {
                last = format!("returned {:?}", value.clone().json());
                false
            }
            Err(error) => match matches_error(throws, error) {
                Ok(()) => true,
                Err(why) => {
                    last = why;
                    false
                }
            },
        });
        if !matched {
            return Err(format!("expected to throw {throws}: {last}"));
        }
    }
    Ok(())
}

fn matches_error(throws: &Json, error: &Error) -> Result<(), String> {
    let (name, message) = named(error);
    if let Some(expected) = throws["name"].as_str() {
        if name != expected {
            return Err(format!("threw {name}: {message}"));
        }
    }
    if let Some(expected) = throws["message"].as_str() {
        if !message.contains(expected) {
            return Err(format!("threw {name}: {message}"));
        }
    }
    if let Some(errors) = throws["errors"].as_array() {
        let children = aggregated(error)
            .ok_or_else(|| format!("threw {name}: {message}, without the aggregated errors"))?;
        if children.len() != errors.len() {
            return Err(format!("aggregated {children:?}, expected {errors:?}"));
        }
        for ((name, message), expected) in children.iter().zip(errors) {
            let fine = expected["name"].as_str().is_none_or(|n| n == name)
                && expected["message"]
                    .as_str()
                    .is_none_or(|m| message.contains(m));
            if !fine {
                return Err(format!("aggregated {children:?}, expected {errors:?}"));
            }
        }
    }
    Ok(())
}

/// The log expectations, retried until they hold or [`SETTLE`] passed.
/// Once they hold, a step with `logLacks` (or `reported`, which counts its
/// lines) waits [`LACKS`] and checks again: what must not happen could
/// still be on its way.
async fn check_log(
    world: &dyn World,
    log: &EventsLog,
    expect: &Json,
    viewer: Option<Location>,
    event: &str,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + scaled(SETTLE);
    loop {
        let checked = log_holds(world, log, expect, viewer, event);
        match checked {
            Ok(()) => break,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await
            }
            Err(why) => return Err(why),
        }
    }
    if expect.get("logLacks").is_some() || expect.get("reported").is_some() {
        tokio::time::sleep(scaled(LACKS)).await;
        return log_holds(world, log, expect, viewer, event)
            .map_err(|why| format!("{why} (after the step had settled)"));
    }
    Ok(())
}

fn strings(value: &Json) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn log_holds(
    world: &dyn World,
    log: &EventsLog,
    expect: &Json,
    viewer: Option<Location>,
    event: &str,
) -> Result<(), String> {
    let entries = log.entries();
    if let Some(exact) = expect.get("log") {
        let exact = strings(exact);
        if entries != exact {
            return Err(format!("log: expected {exact:?}, got {entries:?}"));
        }
    }
    if let Some(unordered) = expect.get("logUnordered") {
        let (mut want, mut got) = (strings(unordered), entries.clone());
        want.sort();
        got.sort();
        if want != got {
            return Err(format!(
                "log (any order): expected {want:?}, got {entries:?}"
            ));
        }
    }
    if let Some(contains) = expect.get("logContains") {
        let mut rest = entries.iter();
        for want in strings(contains) {
            if !rest.any(|entry| *entry == want) {
                return Err(format!(
                    "log: expected to contain {contains} in order, got {entries:?}"
                ));
            }
        }
    }
    if let Some(lacks) = expect.get("logLacks") {
        for unwanted in strings(lacks) {
            if entries.contains(&unwanted) {
                return Err(format!("log: {unwanted:?} must not be logged: {entries:?}"));
            }
        }
    }
    if let Some(sees) = expect.get("filterSees") {
        let seen = log.seen();
        let rows: Vec<String> = seen
            .iter()
            .map(|d| d["row"].as_str().unwrap_or("null").to_owned())
            .collect();
        if rows != strings(sees) {
            return Err(format!(
                "filterSees: expected {sees}, the filter saw {rows:?}"
            ));
        }
        for description in &seen {
            let row = description["row"].as_str().unwrap_or_default();
            let Some(mut want) = world.description(row) else {
                continue;
            };
            // Seen from the other side of the session with the node (§9):
            // the entry is imported, named by the id its kernel greeted
            // with (the host's is `main`), at the session's scope (the
            // root).
            let on_node = want["node"] == json!("node");
            if (viewer == Some(Location::Node)) != on_node {
                want["node"] = match on_node {
                    true => json!("node"),
                    false => json!("main"),
                };
                want["scope"] = json!([]);
            } else if on_node {
                want["node"] = Json::Null;
            }
            for (key, value) in want.as_object().into_iter().flatten() {
                let got = description[key].clone();
                if &got != value {
                    return Err(format!(
                        "filterSees: the description of {row} says {key} {got}, expected {value}: {description}"
                    ));
                }
            }
        }
    }
    if let Some(reported) = expect.get("reported").and_then(Json::as_array) {
        let Some(lines) = world.reported() else {
            return Err("reported: this world cannot see emit failure reports".into());
        };
        for want in reported {
            let label = want["label"].as_str().unwrap_or_default();
            let name = want["name"].as_str().unwrap_or_default();
            let filter = want["filter"].as_bool().unwrap_or(false);
            let places = report_places(world, label, filter);
            let about: Vec<&String> = lines
                .iter()
                .filter(|line| reports(line, event, filter, &places).is_some())
                .collect();
            let [line] = about.as_slice() else {
                return Err(format!(
                    "reported: expected one line for {label} about {event}, got {}: {lines:?}",
                    about.len()
                ));
            };
            if reports(line, event, filter, &places) != Some(name) {
                return Err(format!(
                    "reported: {label} should fail with {name}: {line:?}"
                ));
            }
        }
    }
    Ok(())
}

/// Where a report line of `label` may say its listener is (§5.6): `row
/// <label>` where it runs; `runtime <name>` or `node <peer>` from the
/// dispatcher, when the invocation could not be delivered or answered.
/// A filter's line names its emitter's row (`from row <label>`).
fn report_places(world: &dyn World, label: &str, filter: bool) -> Vec<String> {
    let mut places = vec![format!("row {label}")];
    if filter {
        return places;
    }
    if let Some(description) = world.description(label) {
        if let Some(runtime) = description["runtime"].as_str() {
            places.push(format!("runtime {runtime}"));
        }
        match description["node"].as_str() {
            // Seen from the host, a listener on the node is the node's.
            Some(node) => places.push(format!("node {node}")),
            // Seen from the node, a listener of the host's side is the host's.
            None => places.push("node host".into()),
        }
    }
    places
}

/// The error name of a §5.6 report `line` about `event` at one of
/// `places`, or `None` when the line is about something else:
/// `listener of <event> in <place> failed: <Name>: <message>`, or
/// `filter of <event> from <place> failed: ...`, either after `rutis: `
/// (as runtimes print it); a row's listener also as `<row>: listener of
/// <event> failed: ...` (as a host prints it).
fn reports<'a>(line: &'a str, event: &str, filter: bool, places: &[String]) -> Option<&'a str> {
    let (kind, at) = match filter {
        true => ("filter", "from"),
        false => ("listener", "in"),
    };
    let rows = places.iter().filter_map(|place| place.strip_prefix("row "));
    let rest = rows
        .clone()
        .find_map(|row| line.strip_prefix(&format!("{row}: {kind} of {event} failed: ")))
        .or_else(|| {
            let line = line.strip_prefix("rutis: ").unwrap_or(line);
            let rest = line.strip_prefix(&format!("{kind} of {event} {at} "))?;
            let rest = places
                .iter()
                .find_map(|place| rest.strip_prefix(place.as_str()))?;
            rest.strip_prefix(" failed: ")
        })?;
    let (name, _message) = rest.split_once(": ")?;
    Some(name)
}

// ── The file's shape ────────────────────────────────────────────

/// Why a value as the file writes it is no value: a `$` tag [`argument`]
/// cannot read.
fn tagged_problem(value: &Json) -> Result<(), String> {
    match value {
        Json::Array(items) => items.iter().try_for_each(tagged_problem),
        Json::Object(fields) if fields.len() == 1 => {
            let (key, inner) = fields.iter().next().unwrap();
            if !key.starts_with('$') {
                return tagged_problem(inner);
            }
            if key == "$callback" {
                return match inner.as_str() {
                    Some("upper") => Ok(()),
                    _ => Err(format!("the only callback is \"upper\": {value}")),
                };
            }
            if let Some(entries) = inner.as_array().filter(|_| key == "$map" || key == "$set") {
                entries.iter().try_for_each(tagged_problem)?;
            }
            match tagged(key, inner) {
                Some(_) => Ok(()),
                None => Err(format!("a bad tagged value {value}")),
            }
        }
        Json::Object(fields) => fields.values().try_for_each(tagged_problem),
        _ => Ok(()),
    }
}

const DOES: [&str; 11] = [
    "returns",
    "returnsNothing",
    "echo",
    "throws",
    "after",
    "next",
    "veto",
    "nextTwice",
    "waits",
    "calls",
    "emits",
];

const PARTICIPANT: [&str; 14] = [
    "label",
    "at",
    "scope",
    "name",
    "kind",
    "options",
    "log",
    "does",
    "typed",
    "fails",
    "inject",
    "provides",
    "observesDispatch",
    "why",
];

fn check_does(does: &Json) -> Result<(), String> {
    let Json::Object(fields) = does else {
        return Err(format!("does is an object: {does}"));
    };
    let key = fields
        .keys()
        .find(|key| *key != "then")
        .ok_or("an empty does")?;
    if !DOES.contains(&key.as_str()) {
        return Err(format!("unknown does {key:?}"));
    }
    if key == "after" {
        check_does(&does["then"])?;
    }
    if key == "emits" {
        Spec::parse(&does["emits"])?;
    }
    Ok(())
}

fn check_participant(expected: &Expected, participant: &Participant) -> Result<(), String> {
    unknown_keys(
        &format!("participant {}", participant.label),
        &participant.raw,
        &PARTICIPANT,
    )?;
    if participant.scope != "root" && !expected.scopes.contains_key(&participant.scope) {
        return Err(format!(
            "{}: unknown scope {}",
            participant.label, participant.scope
        ));
    }
    if participant.at == Location::Node && participant.scope != "root" {
        return Err(format!(
            "{}: a node participant is at the root",
            participant.label
        ));
    }
    check_does(&participant.raw["does"])?;
    if let Some(kind) = participant.raw.get("kind") {
        if !matches!(kind.as_str(), Some("sync" | "async" | "any")) {
            return Err(format!("{}: unknown kind {kind}", participant.label));
        }
    }
    if let Some(options) = participant.raw.get("options") {
        unknown_keys("options", options, &["prepend", "global"])?;
    }
    if let Some(typed) = participant.raw.get("typed") {
        if participant.at != Location::Host
            || !matches!(typed.as_str(), Some("struct" | "tuple" | "unit"))
        {
            return Err(format!(
                "{}: typed is struct, tuple or unit, at host",
                participant.label
            ));
        }
    }
    Ok(())
}

/// Everything wrong with the file's shape (empty when it is right): what
/// each implementation can rely on before running a case.
pub fn problems(expected: &Expected) -> Vec<String> {
    let mut problems = Vec::new();
    let mut ids = BTreeSet::new();
    for case in &expected.cases {
        let mut problem = |what: String| problems.push(format!("{}: {what}", case.case));
        if !ids.insert(case.case.clone()) {
            problem("the case id twice".into());
        }
        let checked = (|| -> Result<(), String> {
            let places = case.places()?;
            for participant in case.participants()? {
                check_participant(expected, &participant)?;
            }
            let steps = case.parsed_steps()?;
            for step in &steps {
                if let Step::Load { participant, .. } = step {
                    check_participant(expected, participant)?;
                }
                if let Step::Unload { label, .. } | Step::Update { label } = step {
                    if !places.contains_key(label) {
                        return Err(format!("{label} is not in the case"));
                    }
                }
                for spec in step.emits() {
                    if !["e", "f"].contains(&spec.row.as_str()) {
                        return Err(format!("emitter rows are e or f, not {}", spec.row));
                    }
                    if spec.scope != "root" && !expected.scopes.contains_key(&spec.scope) {
                        return Err(format!("unknown scope {}", spec.scope));
                    }
                    if spec.cancel.is_some() && !places.contains_key(spec.cancel.as_ref().unwrap())
                    {
                        return Err("cancel names no label of the case".into());
                    }
                    spec.args.iter().try_for_each(tagged_problem)?;
                }
                if let Some(returns) = step.expect().and_then(|expect| expect.get("returns")) {
                    tagged_problem(returns)?;
                }
                if step
                    .expect()
                    .is_some_and(|expect| expect.get("cancelled").is_some())
                    && step.emits().is_empty()
                {
                    return Err("cancelled is for a step that dispatches".into());
                }
                for want in step
                    .expect()
                    .and_then(|expect| expect["reported"].as_array())
                    .into_iter()
                    .flatten()
                {
                    unknown_keys("reported", want, &["label", "name", "filter"])?;
                    let label = want["label"].as_str().unwrap_or_default();
                    if !places.contains_key(label) {
                        return Err(format!("reported names {label}, not in the case"));
                    }
                }
            }
            for limit in case.only.iter().chain(&case.except) {
                let Json::Object(fields) = limit else {
                    return Err(format!("a limit is an object: {limit}"));
                };
                if fields.get("why").and_then(Json::as_str).is_none() {
                    return Err(format!("a limit says why: {limit}"));
                }
                for (label, languages) in fields.iter().filter(|(key, _)| *key != "why") {
                    if !places.contains_key(label) && label != "X" && label != "Y" {
                        return Err(format!("a limit names {label}, not in the case"));
                    }
                    for language in languages.as_array().into_iter().flatten() {
                        if !expected
                            .languages
                            .iter()
                            .any(|l| Some(l.as_str()) == language.as_str())
                        {
                            return Err(format!("unknown language {language}"));
                        }
                    }
                }
            }
            if case.tool {
                // The tools run every participant at X in one process: no
                // other process, no host, no rutis/* (§12.2 testing tools).
                let typed = case
                    .participants()?
                    .iter()
                    .any(|p| p.raw.get("typed").is_some());
                let rutis = case.participants()?.iter().any(|p| {
                    p.raw["name"]
                        .as_str()
                        .unwrap_or(&case.name)
                        .starts_with("rutis/")
                });
                let more = steps.iter().any(|step| {
                    matches!(step, Step::Kill { .. } | Step::Restart { .. } | Step::Concurrent { .. } | Step::Unload { then: Some(_), .. })
                        || step.emits().iter().any(|spec| spec.then.is_some())
                        || matches!(step, Step::Load { participant, .. } if participant.raw["name"].as_str().unwrap_or(&case.name).starts_with("rutis/"))
                });
                if typed || rutis || more {
                    return Err("the testing tools cannot run it: mark it tool: false".into());
                }
            }
            for x in expected.languages.iter().filter(|l| *l != "rust") {
                let ys: Vec<&str> = std::iter::once(x.as_str())
                    .chain(
                        expected
                            .languages
                            .iter()
                            .map(String::as_str)
                            .filter(|l| *l != x && *l != "rust"),
                    )
                    .collect();
                combinations(case, x, &ys)?;
            }
            Ok(())
        })();
        if let Err(why) = checked {
            problem(why);
        }
    }
    problems
}

/// The placement the testing tools run: every participant at X, in
/// `language`. `None` when the case is not for the tools, places a row in
/// a scope (the tools load rows at the root only: platform cleanup design
/// §7.5), or `only` / `except` rule that out.
pub fn tool_combo(case: &Case, language: &str) -> Option<Combo> {
    if !case.tool {
        return None;
    }
    let places = case.places().ok()?;
    if places.values().any(|(_, scope)| scope != "root") {
        return None;
    }
    let combo = Combo {
        x: language.to_owned(),
        y: language.to_owned(),
        at: places
            .keys()
            .map(|label| (label.clone(), Location::X))
            .collect(),
    };
    let allowed = case.only.iter().all(|limit| limit_holds(limit, &combo))
        && !case.except.iter().any(|limit| limit_holds(limit, &combo));
    allowed.then_some(combo)
}

/// The languages a runtime under test may be in: all but `rust`.
pub fn runtime_languages(expected: &Expected) -> Vec<&str> {
    expected
        .languages
        .iter()
        .map(String::as_str)
        .filter(|l| *l != "rust")
        .collect()
}

/// Y's languages for X in `x`: `x` first, then every other runtime
/// language.
pub fn y_languages<'a>(expected: &'a Expected, x: &'a str) -> Vec<&'a str> {
    std::iter::once(x)
        .chain(
            runtime_languages(expected)
                .into_iter()
                .filter(move |l| *l != x),
        )
        .collect()
}

/// Results of many runs, summarized: per case, how many placements passed,
/// and the first failure of each case. A case [`Tally::unrun`] names
/// failed: it should have run and did not.
#[derive(Default)]
pub struct Tally {
    rows: BTreeMap<String, (usize, usize, Option<String>)>,
}

impl Tally {
    /// `case` ran in no placement although it should have, for `why`.
    pub fn unrun(&mut self, case: &Case, why: &str) {
        let entry = self.rows.entry(case.case.clone()).or_default();
        entry.1 += 1;
        entry.2.get_or_insert_with(|| why.to_owned());
    }

    pub fn record(&mut self, case: &Case, combo: &Combo, result: &Result<(), String>) {
        let entry = self.rows.entry(case.case.clone()).or_default();
        match result {
            Ok(()) => entry.0 += 1,
            Err(why) => {
                entry.1 += 1;
                entry
                    .2
                    .get_or_insert_with(|| format!("{}: {why}", combo.describe()));
            }
        }
    }

    pub fn failed(&self) -> usize {
        self.rows
            .values()
            .filter(|(_, failed, _)| *failed > 0)
            .count()
    }

    pub fn cases(&self) -> usize {
        self.rows.len()
    }
}

impl std::fmt::Display for Tally {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (case, (passed, failed, first)) in &self.rows {
            match first {
                None => writeln!(f, "pass {case} ({passed} placements)")?,
                Some(first) => writeln!(
                    f,
                    "FAIL {case} ({failed} of {} placements): {first}",
                    passed + failed
                )?,
            }
        }
        write!(f, "{} of {} cases failed", self.failed(), self.cases())
    }
}
