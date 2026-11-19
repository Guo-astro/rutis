use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::session::Error;

/// Who speaks at the far end, for diagnostics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Function,
    Future,
    /// An object with methods and live properties, addressed by reference.
    Object,
    /// A plugin definition: not called, only loaded by the runtime that
    /// defined it (`docs/design-plugin-values-2026-10-25.md` §5).
    Plugin,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub(crate) enum WireValue {
    Undefined,
    Data(Value),
    List(Vec<WireValue>),
    /// A plain object whose fields contain references.
    Record(std::collections::BTreeMap<String, WireValue>),
    /// An AbortSignal for the receiving call: it aborts when the caller
    /// cancels the call.
    Signal,
    Reference {
        id: u64,
        home: bool,
        kind: Kind,
        origin: Vec<String>,
    },
    // The tagged values (#308; `docs/design-protocol-plugins-2026-09-25.md`
    // §6.1), sent only to a far end that declared `value-<tag>`. Their text
    // is checked when decoded (`session::values`).
    /// RFC 3339 in UTC.
    Instant(String),
    #[serde(rename = "datetime")]
    DateTime(String),
    Date(String),
    Time(String),
    /// Base64, padded.
    Bytes(String),
    /// Key-value pairs in the sender's order.
    Map(Vec<(WireValue, WireValue)>),
    Set(Vec<WireValue>),
    #[serde(rename = "bigint")]
    BigInt(String),
    Decimal(String),
    Uuid(String),
}

impl WireValue {
    /// The tag of a tagged value, whose capability is `value-<tag>`.
    pub(crate) fn tag(&self) -> Option<&'static str> {
        Some(match self {
            Self::Instant(_) => "instant",
            Self::DateTime(_) => "datetime",
            Self::Date(_) => "date",
            Self::Time(_) => "time",
            Self::Bytes(_) => "bytes",
            Self::Map(_) => "map",
            Self::Set(_) => "set",
            Self::BigInt(_) => "bigint",
            Self::Decimal(_) => "decimal",
            Self::Uuid(_) => "uuid",
            _ => return None,
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Failure {
    pub name: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph: Option<Value>,
}
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        match error {
            Error::Remote {
                name,
                message,
                graph,
            } => Self {
                name,
                message,
                graph,
            },
            Error::SyncWaitCycle(message) => Self {
                name: "SyncWaitCycle".into(),
                message,
                graph: None,
            },
            // A BindingError with the code a program decides by (the
            // diagnostics design §4.3).
            error => {
                let message = error.to_string();
                match (error.code(), error.data()) {
                    (Some(code), data) => {
                        let coded =
                            crate::exports::failure("BindingError", code, message.clone(), data);
                        let graph = match coded {
                            Error::Remote { graph, .. } => graph,
                            _ => None,
                        };
                        Self {
                            name: "BindingError".into(),
                            message,
                            graph,
                        }
                    }
                    (None, _) => Self {
                        name: "BindingError".into(),
                        message,
                        graph: None,
                    },
                }
            }
        }
    }
}
impl From<Failure> for Error {
    fn from(error: Failure) -> Self {
        if error.name == "SyncWaitCycle" && error.graph.is_none() {
            Self::SyncWaitCycle(error.message)
        } else {
            Self::Remote {
                name: error.name,
                message: error.message,
                graph: error.graph,
            }
        }
    }
}

/// The default of a call frame's `awaited`: a caller that says nothing is
/// taken to wait for the result (the conservative reading, SESS-83).
fn waited() -> bool {
    true
}

/// Whether `awaited` is the default, and so not written (SESS-7).
fn is_waited(awaited: &bool) -> bool {
    *awaited
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum Frame {
    /// Opens a session: the version, the endpoint, its implementation and
    /// its capabilities.
    Hello {
        version: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        implementation: Option<Implementation>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capabilities: Option<Vec<String>>,
    },
    Invoke {
        id: String,
        path: Vec<String>,
        target: String,
        method: String,
        args: WireValue,
        /// The caller waits for the reply synchronously (`sync: true`).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        sync: bool,
        /// The caller will wait for this call's result before the call
        /// that made it finishes (default; `false` says it will not, so the
        /// call is no wait edge; SESS-83). Only `false` is written.
        #[serde(default = "waited", skip_serializing_if = "is_waited")]
        awaited: bool,
    },
    /// Call a function reference, or a method of an object reference.
    Call {
        id: String,
        path: Vec<String>,
        reference: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        method: Option<String>,
        args: WireValue,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        sync: bool,
        /// See `Invoke`: absent means the result is waited for.
        #[serde(default = "waited", skip_serializing_if = "is_waited")]
        awaited: bool,
    },
    /// Read a property of an object reference (a live read).
    Get {
        id: String,
        path: Vec<String>,
        reference: u64,
        property: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        sync: bool,
        /// See `Invoke`: absent means the result is waited for.
        #[serde(default = "waited", skip_serializing_if = "is_waited")]
        awaited: bool,
    },
    Await {
        id: String,
        path: Vec<String>,
        reference: u64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        sync: bool,
        /// See `Invoke`: absent means the result is waited for.
        #[serde(default = "waited", skip_serializing_if = "is_waited")]
        awaited: bool,
    },
    Return {
        id: String,
        value: WireValue,
    },
    Throw {
        id: String,
        error: Failure,
    },
    Release {
        reference: u64,
        count: u64,
    },
    /// The caller gave up on call `id`: abort its signal / stop awaiting.
    /// The callee resolves it with `ack` (and then no reply) or with the
    /// reply it already wrote; a late reply is discarded (#1099).
    Cancel {
        id: String,
    },
    /// The callee resolved a `cancel`: it will send no `return`/`throw` for
    /// this call. A reply already in flight arrived first and cleared the
    /// caller's entry; an `ack` without one is ignored.
    Ack {
        id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A float in a frame reads back as the same double (#290). Written in
    /// their shortest form, these doubles read one or two units in the last
    /// place off under serde_json's default float parsing; its
    /// `float_roundtrip` feature reads them exactly.
    #[test]
    fn floats_in_a_frame_read_back_bit_exact() {
        for x in [
            1.0934904699147477e61_f64,
            5.763334961677081e-103,
            2.3852144308262914e-48,
            1.3677769034820771e302,
            3.278540359048182e-167,
            2.917874861685172e-272,
        ] {
            let frame = format!(
                r#"{{"op":"return","id":"1","value":{{"type":"data","value":[{}]}}}}"#,
                serde_json::to_string(&x).unwrap()
            );
            let Ok(Frame::Return {
                value: WireValue::Data(value),
                ..
            }) = serde_json::from_str::<Frame>(&frame)
            else {
                panic!("{frame} is not a return frame");
            };
            let got = value[0].as_f64().unwrap();
            assert_eq!(got.to_bits(), x.to_bits(), "{x:e} read back as {got:e}");
        }
    }

    /// guarantee: sdk/error-codes
    /// rutis's own errors cross as `BindingError`s carrying the code a
    /// program decides by, and its details as `data` (#677).
    #[test]
    fn rutis_errors_cross_with_their_codes() {
        use crate::session::{code, Error};
        let root = |failure: &Failure| {
            let graph = failure.graph.as_ref().expect("a graph");
            let index = graph["root"]["value"].as_u64().unwrap() as usize;
            graph["nodes"][index].clone()
        };
        let no_method = Failure::from(Error::binding(
            code::NO_METHOD,
            "weather has no method tomorow (methods: today, tomorrow)".into(),
            Some(serde_json::json!({ "service": "weather", "method": "tomorow" })),
        ));
        assert_eq!(no_method.name, "BindingError");
        assert_eq!(
            no_method.message,
            "weather has no method tomorow (methods: today, tomorrow)"
        );
        let node = root(&no_method);
        assert_eq!(node["code"], "no-method");
        assert_eq!(
            node["data"],
            serde_json::json!({ "type": "data", "value": { "service": "weather", "method": "tomorow" } })
        );
        // Read back on the far end, the code and data are there.
        let back = Error::from(no_method);
        assert_eq!(back.code(), Some("no-method"));
        assert_eq!(
            back.data(),
            Some(serde_json::json!({ "service": "weather", "method": "tomorow" }))
        );
        for (error, expected) in [
            (Error::value("a sparse array"), "bad-value"),
            (
                Error::Transport("the runtime py exited".into()),
                "peer-gone",
            ),
        ] {
            let failure = Failure::from(error);
            assert_eq!(failure.name, "BindingError");
            assert_eq!(root(&failure)["code"], expected);
        }
        // A protocol refusal carries no code.
        let refused = Failure::from(Error::refused("link is already registered".into()));
        assert_eq!(refused.name, "BindingError");
        assert!(refused.graph.is_none());
    }

    // ── Any value, any frame, bad input (#631) ───────────────────────
    //
    // guarantee: bridge/value-roundtrip, bridge/large-and-deep-values,
    // bridge/value-tags
    //
    // A value this side can carry within the depth limit, written to the
    // wire and read back, is the same value; a frame this side writes
    // reads back as the same frame, byte for byte. Bad input fails alone:
    // the frame after it still reads, and a tagged value whose text does
    // not decode fails only that value, with `bad-value`. A failure prints
    // the seed, and `RUTIS_PROPTEST_SEED=<seed>` replays it (Q7.2).

    use proptest::prelude::*;
    use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner};
    use serde_json::Value as Json;

    use crate::session::rpc::{decode_text, deeper_than, parse_frame, MAX_FRAME_DEPTH};

    /// A generated wire value. `Debug` prints it as the JSON the case
    /// wrote, so a failure shows its own minimal input (`Frame` and
    /// `WireValue` have no `Debug` of their own).
    struct Generated(WireValue);

    impl std::fmt::Debug for Generated {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match serde_json::to_string(&self.0) {
                Ok(text) => f.write_str(&text),
                Err(_) => f.write_str("<unwritable>"),
            }
        }
    }

    /// A generated frame, printed as its JSON when a case fails.
    struct GeneratedFrame(Frame);

    impl std::fmt::Debug for GeneratedFrame {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match serde_json::to_string(&self.0) {
                Ok(text) => f.write_str(&text),
                Err(_) => f.write_str("<unwritable>"),
            }
        }
    }

    /// Text a JSON document may carry, quotes and control characters
    /// included: written and read back, it must not change.
    fn text() -> impl Strategy<Value = String> {
        prop::collection::vec(any::<char>(), 0..8).prop_map(|chars| chars.into_iter().collect())
    }

    /// JSON data `depth` arrays and objects deep, without NaN or
    /// infinities (not JSON).
    fn json_data(depth: u32) -> BoxedStrategy<Json> {
        let number = prop_oneof![
            any::<i64>().prop_map(|n| Json::Number(n.into())),
            any::<u64>().prop_map(|n| Json::Number(n.into())),
            prop_oneof![
                Just(0.0_f64),
                Just(-0.0_f64),
                any::<f64>().prop_filter("finite", |x| x.is_finite()),
            ]
            .prop_map(|x| Json::Number(serde_json::Number::from_f64(x).unwrap())),
        ];
        let leaf = prop_oneof![
            Just(Json::Null),
            any::<bool>().prop_map(Json::Bool),
            number,
            text().prop_map(Json::String),
        ];
        if depth == 0 {
            return leaf.boxed();
        }
        let array = prop::collection::vec(json_data(depth - 1), 0..4).prop_map(Json::Array);
        let object =
            prop::collection::vec((text(), json_data(depth - 1)), 0..4).prop_map(|fields| {
                Json::Object(
                    fields
                        .into_iter()
                        .collect::<serde_json::Map<String, Json>>(),
                )
            });
        prop_oneof![leaf, array, object].boxed()
    }

    fn kind() -> impl Strategy<Value = Kind> {
        prop_oneof![
            Just(Kind::Function),
            Just(Kind::Future),
            Just(Kind::Object),
            Just(Kind::Plugin),
        ]
    }

    /// A wire value `depth` containers deep, of every kind the protocol
    /// has: data, list, record, map, set, signal, reference and each of
    /// the ten tagged kinds. The depth keeps a frame carrying it well
    /// within [`MAX_FRAME_DEPTH`].
    fn wire_value(depth: u32) -> BoxedStrategy<Generated> {
        let reference = (
            any::<u64>(),
            any::<bool>(),
            kind(),
            prop::collection::vec(text(), 0..3),
        )
            .prop_map(|(id, home, kind, origin)| {
                Generated(WireValue::Reference {
                    id,
                    home,
                    kind,
                    origin,
                })
            });
        let leaf = prop_oneof![
            3 => Just(()).prop_map(|()| Generated(WireValue::Undefined)),
            2 => Just(()).prop_map(|()| Generated(WireValue::Signal)),
            6 => json_data(2).prop_map(|data| Generated(WireValue::Data(data))),
            3 => reference,
            1 => text().prop_map(|text| Generated(WireValue::Instant(text))),
            1 => text().prop_map(|text| Generated(WireValue::DateTime(text))),
            1 => text().prop_map(|text| Generated(WireValue::Date(text))),
            1 => text().prop_map(|text| Generated(WireValue::Time(text))),
            1 => text().prop_map(|text| Generated(WireValue::Bytes(text))),
            1 => text().prop_map(|text| Generated(WireValue::BigInt(text))),
            1 => text().prop_map(|text| Generated(WireValue::Decimal(text))),
            1 => text().prop_map(|text| Generated(WireValue::Uuid(text))),
        ];
        if depth == 0 {
            return leaf.boxed();
        }
        let list = prop::collection::vec(wire_value(depth - 1), 0..4).prop_map(|values| {
            Generated(WireValue::List(
                values.into_iter().map(|value| value.0).collect(),
            ))
        });
        let record =
            prop::collection::vec((text(), wire_value(depth - 1)), 0..3).prop_map(|fields| {
                Generated(WireValue::Record(
                    fields
                        .into_iter()
                        .map(|(key, value)| (key, value.0))
                        .collect(),
                ))
            });
        let map = prop::collection::vec((wire_value(depth - 1), wire_value(depth - 1)), 0..3)
            .prop_map(|entries| {
                Generated(WireValue::Map(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key.0, value.0))
                        .collect(),
                ))
            });
        let set = prop::collection::vec(wire_value(depth - 1), 0..3).prop_map(|values| {
            Generated(WireValue::Set(
                values.into_iter().map(|value| value.0).collect(),
            ))
        });
        prop_oneof![4 => leaf, 3 => list, 2 => record, 2 => map, 1 => set].boxed()
    }

    /// A frame of every kind the protocol has, carrying values within the
    /// depth limit.
    fn frame() -> impl Strategy<Value = GeneratedFrame> {
        let path = || prop::collection::vec(text(), 0..3);
        prop_oneof![
            (
                any::<u32>(),
                prop::option::of(text()),
                prop::option::of(
                    (text(), text()).prop_map(|(name, version)| Implementation { name, version }),
                ),
                prop::option::of(prop::collection::vec(text(), 0..3)),
            )
                .prop_map(|(version, endpoint, implementation, capabilities)| {
                    GeneratedFrame(Frame::Hello {
                        version,
                        endpoint,
                        implementation,
                        capabilities,
                    })
                }),
            (
                text(),
                path(),
                text(),
                text(),
                wire_value(3),
                any::<bool>(),
                any::<bool>()
            )
                .prop_map(|(id, path, target, method, args, sync, awaited)| {
                    GeneratedFrame(Frame::Invoke {
                        id,
                        path,
                        target,
                        method,
                        args: args.0,
                        sync,
                        awaited,
                    })
                }),
            (
                text(),
                path(),
                any::<u64>(),
                prop::option::of(text()),
                wire_value(3),
                any::<bool>(),
                any::<bool>(),
            )
                .prop_map(|(id, path, reference, method, args, sync, awaited)| {
                    GeneratedFrame(Frame::Call {
                        id,
                        path,
                        reference,
                        method,
                        args: args.0,
                        sync,
                        awaited,
                    })
                }),
            (
                text(),
                path(),
                any::<u64>(),
                text(),
                any::<bool>(),
                any::<bool>()
            )
                .prop_map(|(id, path, reference, property, sync, awaited)| {
                    GeneratedFrame(Frame::Get {
                        id,
                        path,
                        reference,
                        property,
                        sync,
                        awaited,
                    })
                }),
            (text(), path(), any::<u64>(), any::<bool>(), any::<bool>()).prop_map(
                |(id, path, reference, sync, awaited)| GeneratedFrame(Frame::Await {
                    id,
                    path,
                    reference,
                    sync,
                    awaited,
                })
            ),
            (text(), wire_value(3))
                .prop_map(|(id, value)| { GeneratedFrame(Frame::Return { id, value: value.0 }) }),
            (
                text(),
                text(),
                text(),
                // A null graph is no graph on the wire (serde reads it as
                // `None`): not a state a frame this side writes keeps.
                prop::option::of(json_data(2))
                    .prop_map(|graph| graph.filter(|value| !value.is_null())),
            )
                .prop_map(|(id, name, message, graph)| GeneratedFrame(Frame::Throw {
                    id,
                    error: Failure {
                        name,
                        message,
                        graph,
                    },
                })),
            (any::<u64>(), any::<u64>()).prop_map(|(reference, count)| {
                GeneratedFrame(Frame::Release { reference, count })
            }),
            text().prop_map(|id| GeneratedFrame(Frame::Cancel { id })),
            text().prop_map(|id| GeneratedFrame(Frame::Ack { id })),
        ]
    }

    /// Run `test` over `strategy` with a seed from `RUTIS_PROPTEST_SEED`,
    /// or a fresh one; on failure, panic with the seed and the shrunk
    /// input, printed as its JSON (Q7.2).
    fn check<S: Strategy>(
        name: &str,
        strategy: S,
        test: impl Fn(S::Value) -> Result<(), TestCaseError>,
    ) {
        let seed: u64 = match std::env::var("RUTIS_PROPTEST_SEED") {
            Ok(seed) => seed.parse().expect("RUTIS_PROPTEST_SEED is a u64"),
            Err(_) => {
                use std::hash::{BuildHasher, Hasher};
                let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
                hasher.write_u128(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                );
                hasher.finish()
            }
        };
        let mut bytes = [0u8; 32];
        for chunk in bytes.chunks_mut(8) {
            chunk.copy_from_slice(&seed.to_le_bytes());
        }
        // `Config::default()` reads PROPTEST_CASES (256 when unset).
        let config = Config {
            failure_persistence: None,
            ..Config::default()
        };
        let cases = config.cases;
        let mut runner =
            TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, &bytes));
        let started = std::time::Instant::now();
        if let Err(error) = runner.run(&strategy, test) {
            panic!("{name}: {error}\nreplay with RUTIS_PROPTEST_SEED={seed}");
        }
        eprintln!(
            "{name}: {cases} cases in {:?} (seed {seed})",
            started.elapsed()
        );
    }

    /// guarantee: bridge/value-roundtrip, bridge/large-and-deep-values
    #[test]
    fn wire_values_write_and_read_back_unchanged() {
        check("wire values", wire_value(3), |Generated(value)| {
            let written = serde_json::to_string(&value)
                .map_err(|error| TestCaseError::fail(format!("a value does not write: {error}")))?;
            let read: WireValue = serde_json::from_str(&written).map_err(|error| {
                TestCaseError::fail(format!("a value does not read back: {written}: {error}"))
            })?;
            let rewritten = serde_json::to_string(&read).expect("a value read writes");
            if rewritten != written {
                return Err(TestCaseError::fail(format!(
                    "a value changed on its way back: wrote {written}, wrote {rewritten} again"
                )));
            }
            // Inside a return frame, through the reader's own parse.
            let frame = format!(r#"{{"op":"return","id":"1","value":{written}}}"#);
            if deeper_than(frame.as_bytes(), MAX_FRAME_DEPTH) {
                return Err(TestCaseError::fail(format!(
                    "a value within {} levels made a frame too deep: {written}",
                    crate::session::MAX_DEPTH
                )));
            }
            let parsed = parse_frame(frame.as_bytes()).map_err(|error| {
                TestCaseError::fail(format!(
                    "a return frame carrying a value does not read: {error}"
                ))
            })?;
            if !parsed.ignored.is_empty() {
                return Err(TestCaseError::fail(format!(
                    "a return frame this test wrote has unknown fields: {:?}",
                    parsed.ignored
                )));
            }
            let Frame::Return {
                value: from_frame, ..
            } = parsed.frame
            else {
                return Err(TestCaseError::fail("a return frame read as another frame"));
            };
            let from_frame = serde_json::to_string(&from_frame).expect("a value read writes");
            if from_frame != written {
                return Err(TestCaseError::fail(format!(
                    "a value changed in a frame: wrote {written}, read {from_frame} back"
                )));
            }
            Ok(())
        });
    }

    /// guarantee: bridge/value-roundtrip, bridge/large-and-deep-values
    #[test]
    fn frames_write_and_read_back_unchanged() {
        check("frames", frame(), |GeneratedFrame(frame)| {
            let written = serde_json::to_vec(&frame).expect("a frame writes");
            let text = String::from_utf8_lossy(&written);
            if deeper_than(&written, MAX_FRAME_DEPTH) {
                return Err(TestCaseError::fail(format!(
                    "a frame this test wrote is too deep: {text}"
                )));
            }
            let parsed = parse_frame(&written).map_err(|error| {
                TestCaseError::fail(format!(
                    "a frame this side wrote does not read back: {text}: {error}"
                ))
            })?;
            if !parsed.ignored.is_empty() {
                return Err(TestCaseError::fail(format!(
                    "a frame this side wrote has unknown fields: {text}: {:?}",
                    parsed.ignored
                )));
            }
            let rewritten = serde_json::to_vec(&parsed.frame).expect("a frame read writes");
            if rewritten != written {
                return Err(TestCaseError::fail(format!(
                    "a frame changed on its way back: wrote {text}, wrote {} again",
                    String::from_utf8_lossy(&rewritten)
                )));
            }
            let again = parse_frame(&rewritten)
                .map_err(|error| {
                    TestCaseError::fail(format!("a frame written twice does not read: {error}"))
                })?
                .frame;
            let rewritten_again = serde_json::to_vec(&again).expect("a frame read writes");
            if rewritten_again != rewritten {
                return Err(TestCaseError::fail(format!(
                    "a frame changed on its second way back: wrote {}, wrote {} again",
                    String::from_utf8_lossy(&rewritten),
                    String::from_utf8_lossy(&rewritten_again)
                )));
            }
            Ok(())
        });
    }

    /// A call's `awaited` says whether its caller waits for the result
    /// (SESS-83): absent or `true` reads as waiting, and only `false` is
    /// written.
    /// guarantee: bridge/sync-wait-cycle-refused
    #[test]
    fn a_call_says_when_its_result_is_not_waited_for() {
        let invoke = |awaited: bool| Frame::Invoke {
            id: "node:1".into(),
            path: vec![],
            target: "t".into(),
            method: "m".into(),
            args: WireValue::Undefined,
            sync: false,
            awaited,
        };
        let written = serde_json::to_string(&invoke(false)).unwrap();
        assert!(written.contains("\"awaited\":false"), "{written}");
        let Frame::Invoke { awaited, .. } = parse_frame(written.as_bytes()).unwrap().frame else {
            panic!("an invoke expected");
        };
        assert!(!awaited);
        // `true` is the default, and not written.
        let written = serde_json::to_string(&invoke(true)).unwrap();
        assert!(!written.contains("awaited"), "{written}");
        // An older frame without the field reads as one whose result is
        // waited for: the conservative reading.
        let without = br#"{"op":"invoke","id":"node:1","path":[],"target":"t","method":"m","args":{"type":"undefined"}}"#;
        let Frame::Invoke { awaited, .. } = parse_frame(without).unwrap().frame else {
            panic!("an invoke expected");
        };
        assert!(awaited);
        // A field of another type is not a frame (§8.1).
        let wrong = br#"{"op":"invoke","id":"node:1","path":[],"target":"t","method":"m","args":{"type":"undefined"},"awaited":"no"}"#;
        assert!(parse_frame(wrong).is_err());
    }

    /// A frame the reader takes, unchanged; the frame after bad input is
    /// this one.
    const GOOD_FRAME: &[u8] = br#"{"op":"release","reference":1,"count":1}"#;

    /// Bytes a far end might send: arbitrary, broken JSON where a value
    /// goes, and a valid frame with one byte changed.
    fn bad_frames() -> impl Strategy<Value = Vec<u8>> {
        let args = prop::collection::vec(any::<u8>(), 0..48).prop_map(|bytes| {
            let mut frame =
                br#"{"op":"invoke","id":"main:1","path":[],"target":"t","method":"m","args":"#
                    .to_vec();
            frame.extend_from_slice(&bytes);
            frame.push(b'}');
            frame
        });
        let value = prop::collection::vec(any::<u8>(), 0..48).prop_map(|bytes| {
            let mut frame = br#"{"op":"return","id":"1","value":"#.to_vec();
            frame.extend_from_slice(&bytes);
            frame.push(b'}');
            frame
        });
        let mutated = (0..GOOD_FRAME.len(), any::<u8>()).prop_map(|(at, byte)| {
            let mut frame = GOOD_FRAME.to_vec();
            frame[at] = byte;
            frame
        });
        prop_oneof![
            prop::collection::vec(any::<u8>(), 0..96),
            args,
            value,
            mutated,
        ]
    }

    /// guarantee: bridge/unknown-fields (the reader fails the frame alone)
    #[test]
    fn a_frame_that_does_not_read_fails_only_itself() {
        check("bad frames", bad_frames(), |bytes| {
            // As the reader does first: a frame deeper than the limit is
            // not parsed at all.
            if !deeper_than(&bytes, MAX_FRAME_DEPTH) {
                let _ = parse_frame(&bytes);
            }
            let _ = serde_json::from_slice::<WireValue>(&bytes);
            // The reader goes on: the frame after it reads.
            let after = parse_frame(GOOD_FRAME).expect("a frame after bad input reads");
            if !after.ignored.is_empty() {
                return Err(TestCaseError::fail(
                    "the frame after bad input has unknown fields",
                ));
            }
            if !matches!(
                after.frame,
                Frame::Release {
                    reference: 1,
                    count: 1
                }
            ) {
                return Err(TestCaseError::fail(
                    "the frame after bad input read as another frame",
                ));
            }
            Ok(())
        });
    }

    /// guarantee: bridge/value-tags
    #[test]
    fn a_tagged_value_that_does_not_decode_fails_only_itself() {
        // The tagged kinds whose text `decode_text` reads; a map or set is
        // decoded by the recursive decoder, not there.
        fn text_tag(value: &WireValue) -> bool {
            matches!(
                value,
                WireValue::Instant(_)
                    | WireValue::DateTime(_)
                    | WireValue::Date(_)
                    | WireValue::Time(_)
                    | WireValue::Bytes(_)
                    | WireValue::BigInt(_)
                    | WireValue::Decimal(_)
                    | WireValue::Uuid(_)
            )
        }
        let tagged =
            wire_value(3).prop_filter("a tagged text value", |generated| text_tag(&generated.0));
        check("tagged values", tagged, |Generated(value)| {
            match decode_text(value) {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    if error.code() != Some(crate::session::code::BAD_VALUE) {
                        return Err(TestCaseError::fail(format!(
                            "a tagged value failed with another code: {error:?}"
                        )));
                    }
                }
                Err(other) => {
                    return Err(TestCaseError::fail(format!(
                        "a {} value came back undecoded",
                        other.tag().unwrap_or("?")
                    )))
                }
            }
            Ok(())
        });
    }

    /// The corpus the `session_frame` fuzz target has collected (#179),
    /// replayed here so the property is checked without cargo-fuzz: every
    /// input that reads writes back and reads again unchanged.
    #[test]
    fn the_session_frame_fuzz_corpus_writes_back_unchanged() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/corpus/session_frame");
        let Ok(entries) = std::fs::read_dir(&directory) else {
            return; // not in the source tree (a packaged crate)
        };
        let mut read = 0;
        for entry in entries {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            if deeper_than(&bytes, MAX_FRAME_DEPTH) {
                continue;
            }
            let Ok(parsed) = parse_frame(&bytes) else {
                continue;
            };
            let written = serde_json::to_vec(&parsed.frame).unwrap();
            let again = parse_frame(&written).unwrap_or_else(|error| {
                panic!(
                    "{}: a frame written does not read back: {error}",
                    path.display()
                )
            });
            let rewritten = serde_json::to_vec(&again.frame).unwrap();
            assert_eq!(
                String::from_utf8_lossy(&written),
                String::from_utf8_lossy(&rewritten),
                "{}: a frame changed on its way back",
                path.display()
            );
            read += 1;
        }
        assert!(read > 0, "no frame in {} read", directory.display());
    }
}
