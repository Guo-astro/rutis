//! The runtime conformance suite (feature `testing`): what any language
//! runtime must do with rows, as checks it runs against itself. Each check
//! panics with what was broken.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use crate::session::{settle, Error, Reply, Value};

/// What calls on the conformance plugin return: the one list both the real
/// runtimes ([`runtime`]) and the SDK testing tools (`@arcships/rutis/testing`,
/// `rutis.testing`) are checked against (issue #206, risk A1, Q6.7.2). Its
/// `about` says how to read it.
pub const WEATHER_EXPECTED: &str = include_str!("testing/weather.expected.json");


/// **Runtime** ([`runtime`]): a runtime loads, as a row, a plugin `entry`
/// that injects `clock` (a rutis host service with `now`, sync, returning
/// 0, 1, 2, ...) and provides `weather`:
///
/// | method | does |
/// | --- | --- |
/// | `today()` | `"Oslo at <clock.now()>"`, sync |
/// | `later()` | `"Oslo later"`, async |
/// | `each(fn)` | `[fn("mon"), fn("tue")]`, calling back during the call |
/// | `echo(value)` | `value`, sync |
/// | `crash()` | ends the runtime process with status 17 |
/// | `samples()`, `sample(name)` | optional: values only its language makes |
///
/// The calls made and their results are [`WEATHER_EXPECTED`]. The repository
/// has the plugin for Node (`node/rutis-runtime/test/fixtures/
/// conformance-weather.mjs`), Python (`python/rutis/tests/
/// conformance_weather.py`), Go and Bun.
///
/// guarantee: sdk/testing-matches-runtime (risk A1, Q6.7.2), the runtime
/// side; the testing tools' side is `node/rutis/test/conformance.test.mjs`
/// and `python/rutis/tests/test_conformance.py`.
///
/// `ctx` is the context whose bus the process joined (`Mount::ctx`): the
/// `events` checks emit there.
pub async fn runtime(
    ctx: &rutis::Ctx,
    process: Arc<crate::runtime::Process>,
    entry: &std::path::Path,
    started_here: bool,
) {
    use crate::runtime::row_projection_of;
    use crate::session::{host_key, HostDispatch};
    use serde_json::Value as Json;
    use std::sync::atomic::AtomicUsize;

    struct Clock(AtomicUsize);
    impl HostDispatch for Clock {
        fn invoke(&self, method: &str, _args: Value) -> Reply {
            assert_eq!(method, "now", "the plugin calls only clock.now");
            Ok(json!(self.0.fetch_add(1, Ordering::SeqCst)).into())
        }
        fn methods(&self) -> Option<serde_json::Value> {
            Some(json!({ "now": "sync" }))
        }
    }

    async fn eventually(mut check: impl FnMut() -> bool, what: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !check() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    // An argument as the expectations write it: data, or a callback.
    fn argument(arg: &Json) -> Value {
        match arg.get("$callback") {
            None => Value::Data(arg.clone()),
            Some(Json::String(kind)) if kind == "upper" => Value::callback(|args| {
                let [day]: [String; 1] = crate::session::decode_value(args)?;
                Ok(json!(day.to_uppercase()).into())
            }),
            Some(spec) => {
                let thrown = spec
                    .get("throws")
                    .unwrap_or_else(|| panic!("unknown callback {spec}"))
                    .clone();
                Value::callback(move |_| {
                    Err(Error::Remote {
                        name: thrown["name"].as_str().unwrap().to_owned(),
                        message: thrown["message"].as_str().unwrap().to_owned(),
                        graph: None,
                    })
                })
            }
        }
    }

    // Call `method` with `args`, as its kind says.
    async fn call(weather: &Arc<dyn HostDispatch>, method: &str, kind: &str, args: Value) -> Reply {
        let weather = weather.clone();
        let method = method.to_owned();
        if kind == "sync" {
            tokio::task::spawn_blocking(move || weather.invoke(&method, args))
                .await
                .unwrap()
        } else {
            settle(weather.invoke(&method, args)?).await
        }
    }

    // A value as the expectations write it: data as JSON, a tagged value
    // as `{"$<tag>": <its wire value>}` (`about` in WEATHER_EXPECTED).
    fn written(value: Value) -> Result<Json, Error> {
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

    // `reply` is what `entry` expects.
    fn check(what: &str, entry: &Json, reply: Reply) {
        let why = entry["why"].as_str().unwrap_or_default();
        if let Some(returns) = entry.get("returns") {
            let value = reply
                .unwrap_or_else(|error| panic!("{what} ({why}): expected {returns}, got {error}"));
            let mut value =
                written(value).unwrap_or_else(|error| panic!("{what} ({why}): not data: {error}"));
            if let (Some(ignore), Some(object)) = (entry.get("ignore"), value.as_object_mut()) {
                for key in ignore.as_array().unwrap() {
                    object.remove(key.as_str().unwrap());
                }
            }
            assert_eq!(&value, returns, "{what} ({why})");
        } else if let Some(throws) = entry.get("throws") {
            let error = match reply {
                Ok(value) => panic!("{what} ({why}): expected an error, got {:?}", value.json()),
                Err(error) => error,
            };
            let expected = |field: &str| throws.get(field).and_then(Json::as_str);
            if expected("name").is_none() && expected("message").is_none() {
                return;
            }
            let Error::Remote { name, message, .. } = &error else {
                panic!("{what} ({why}): expected an error from the plugin, got {error:?}")
            };
            if let Some(expected) = expected("name") {
                assert_eq!(name, expected, "{what} ({why}): the error's name");
            }
            if let Some(expected) = expected("message") {
                assert_eq!(message, expected, "{what} ({why}): the error's message");
            }
        } else {
            panic!("{what}: the expectations say neither returns nor throws");
        }
    }

    let expected: Json = serde_json::from_str(WEATHER_EXPECTED).expect("weather.expected.json");
    let service = expected["service"].as_str().unwrap();
    let described = process.describe_row(entry).await.expect("rows.schema");
    assert_eq!(
        described.inject,
        ["clock"],
        "the plugin declares what it injects"
    );
    let declared = described
        .provides
        .get(service)
        .and_then(Json::as_object)
        .unwrap_or_else(|| panic!("the plugin declares {service} in provides"))
        .clone();
    for (method, kind) in expected["methods"].as_object().unwrap() {
        assert_eq!(
            declared.get(method),
            Some(kind),
            "the plugin declares {service}.{method} as {kind}"
        );
    }
    let kind = |method: &str| declared[method].as_str().unwrap().to_owned();
    let provides =
        serde_json::Map::from_iter([(service.to_owned(), Json::Object(declared.clone()))]);
    let lease = process
        .lease_host("clock", Arc::new(Clock(AtomicUsize::new(0))), None)
        .await
        .expect("hosts.provide");
    // The declared service, and the ones the row announces (`undeclared`).
    let projection = row_projection_of("row w", &provides, host_key, |_| false);
    projection.attach(ctx, process.clone()).unwrap();
    let load = || {
        process.load_row_exporting(
            "w",
            entry,
            json!({}),
            &[],
            &[],
            &provides,
            projection.clone(),
            &[],
        )
    };
    load().await.expect("rows.load");
    let key = host_key(service);
    eventually(
        || ctx.get_as::<dyn HostDispatch>(key.clone()).is_some(),
        "the weather service",
    )
    .await;
    let weather = ctx.get_as::<dyn HostDispatch>(key.clone()).unwrap();

    // Every call but the last, which ends the process.
    let (crash, calls) = expected["calls"].as_array().unwrap().split_last().unwrap();
    for (i, entry) in calls.iter().enumerate() {
        let method = entry["call"].as_str().unwrap();
        let args = entry["args"].as_array().unwrap().iter().map(argument);
        let reply = call(&weather, method, &kind(method), Value::List(args.collect())).await;
        check(&format!("calls[{i}] {method}"), entry, reply);
    }

    // Values only the plugin's language makes.
    if declared.contains_key("samples") {
        let names = call(&weather, "samples", &kind("samples"), json!([]).into())
            .await
            .expect("samples")
            .json()
            .unwrap();
        let samples = expected["samples"].as_array().unwrap();
        for name in names.as_array().unwrap() {
            let entry = samples
                .iter()
                .find(|entry| &entry["sample"] == name)
                .unwrap_or_else(|| panic!("the expectations list no sample {name}"));
            let reply = call(&weather, "sample", &kind("sample"), json!([name]).into()).await;
            check(&format!("sample {name}"), entry, reply);
        }
    } else {
        // Q7.8: a plugin may have no samples (#206 follow-ups).
        eprintln!(
            "{}: the plugin has no samples(), so no values only its language makes are checked",
            entry.display()
        );
    }

    // The plugin's listeners are on the host's bus: `parallel` there waits
    // for them.
    for (i, entry) in expected["events"].as_array().unwrap().iter().enumerate() {
        let name = entry["emit"].as_str().unwrap();
        let what = format!("events[{i}] {name}");
        let args = entry["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(argument)
            .collect();
        let emitted = ctx
            .events()
            .parallel(
                ctx,
                &rutis::EventKey::<crate::events::Args>::dynamic(name.to_owned()),
                Arc::new(crate::events::Args(args)),
            )
            .await
            .map_err(|error| crate::events::to_session_error(&error));
        match entry.get("then") {
            Some(then) => {
                emitted.unwrap_or_else(|error| panic!("{what}: {error}"));
                let method = then["call"].as_str().unwrap();
                let args = then["args"].as_array().unwrap().iter().map(argument);
                let reply =
                    call(&weather, method, &kind(method), Value::List(args.collect())).await;
                let mut then = then.clone();
                then["why"] = entry["why"].clone();
                check(&what, &then, reply);
            }
            None => check(&what, entry, emitted.map(|()| Value::Data(Json::Null))),
        }
    }

    // A service the plugin provides without declaring it (#442): announced
    // with the methods its object has, all async.
    let undeclared = &expected["undeclared"];
    let name = undeclared["service"].as_str().unwrap();
    assert!(
        !described.provides.contains_key(name),
        "the plugin does not declare {name}"
    );
    assert!(
        process.supports("rows.announce"),
        "{}: the runtime must report the `rows.announce` feature",
        entry.display()
    );
    let announced = host_key(name);
    eventually(
        || ctx.get_as::<dyn HostDispatch>(announced.clone()).is_some(),
        "the undeclared service",
    )
    .await;
    let forecast = ctx.get_as::<dyn HostDispatch>(announced.clone()).unwrap();
    assert_eq!(
        forecast.methods().as_ref(),
        Some(&undeclared["methods"]),
        "{}: the methods of {name}, inferred from its object",
        entry.display()
    );
    for (i, entry) in undeclared["calls"].as_array().unwrap().iter().enumerate() {
        let method = entry["call"].as_str().unwrap();
        let args = entry["args"].as_array().unwrap().iter().map(argument);
        let reply = call(&forecast, method, "async", Value::List(args.collect())).await;
        check(&format!("undeclared.calls[{i}] {method}"), entry, reply);
    }
    drop(forecast);
    drop(weather);

    process.unload_row("w").await.expect("rows.unload");
    eventually(
        || ctx.get_as::<dyn HostDispatch>(key.clone()).is_none(),
        "the withdrawal",
    )
    .await;

    // A crash ends the session; a process started here says how it ended.
    let status = crash["exits"]
        .as_u64()
        .expect("the last call is the one that ends the process");
    let method = crash["call"].as_str().unwrap();
    load().await.expect("rows.load again");
    eventually(
        || ctx.get_as::<dyn HostDispatch>(key.clone()).is_some(),
        "the service again",
    )
    .await;
    let weather = ctx.get_as::<dyn HostDispatch>(key.clone()).unwrap();
    let crashed = call(&weather, method, &kind(method), json!([]).into()).await;
    // How `ExitStatus` reads: `exit status: 17` on Unix, `exit code: 17` on
    // Windows.
    let ended = if cfg!(windows) {
        format!("exited with exit code: {status}")
    } else {
        format!("exited with exit status: {status}")
    };
    match crashed {
        Err(Error::Transport(message)) if started_here => assert_eq!(
            message,
            format!("Cordis process {ended}"),
            "the session ends with how the process ended"
        ),
        Err(Error::Transport(_)) => {}
        other => panic!("the crash should end the session, got {other:?}"),
    }
    drop(weather);
    process.closed().await;
    let expected = started_here.then_some(ended);
    assert_eq!(process.exit_status(), expected);
    projection.close();
    drop(lease);
    ctx.shutdown().await.unwrap();
}
