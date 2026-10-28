//! `rutis-host call <service>.<method> [arguments…]`: call a service of a
//! running host from the command line (design try-and-test §4). The host
//! makes the call as Rust embedding does ([`call_service`]): the call comes
//! from no row, so the snapshot names its caller `host`. Arguments are
//! JSON values; the result is printed as JSON, a value JSON does not have
//! as its own text.

use rutis::host::CtxHost;
use rutis::host::FiberState;
use rutis_bridge::session::{self, call_service, host_key, Dynamic, HostDispatch, Value};
use rutis_loader::report::EntryStatus;
use rutis_loader::status::Status;
use rutis_loader::Loader;
use serde_json::Value as Json;

/// One call, as `rutis-host call` sends it to the host.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Request {
    pub service: String,
    pub method: String,
    pub args: Vec<Json>,
}

/// The call the command line `<service>.<method> [arguments…]` names: the
/// name split at its last `.` (a service's name may have dots, a method's
/// does not), each argument a JSON value. Not one is guessed to be a
/// string: `007` or `yes` taken for one would pass a wrong value quietly.
pub fn request(name: &str, args: &[String]) -> Result<Request, String> {
    let (service, method) = match name.rsplit_once('.') {
        Some((service, method)) if !service.is_empty() && !method.is_empty() => (service, method),
        _ => {
            let service = name.trim_end_matches('.');
            return Err(format!("name a method, as {service}.<method>"));
        }
    };
    let args = args
        .iter()
        .enumerate()
        .map(|(index, arg)| {
            serde_json::from_str::<Json>(arg).map_err(|_| {
                format!(
                    "argument {} is not JSON: write a string in quotes, as '{}'",
                    index + 1,
                    Json::String(arg.clone())
                )
            })
        })
        .collect::<Result<_, _>>()?;
    Ok(Request {
        service: service.to_owned(),
        method: method.to_owned(),
        args,
    })
}

/// Make `request` as the host: the result as printed (JSON, two spaces of
/// indent), or the error as printed (its first line, then where it came
/// from).
pub async fn call(root: &rutis::Ctx, loader: &Loader, request: Request) -> Result<String, String> {
    let Request {
        service,
        method,
        args,
    } = request;
    let provided = root
        .get_at::<dyn HostDispatch>(&host_key(&service))
        .is_some();
    if let Some(why) = not_there(&loader.status(), loader, &service, provided) {
        return Err(why);
    }
    match call_service::<Dynamic>(root, &service, &method, args).await {
        Ok(Dynamic(value)) => Ok(serde_json::to_string_pretty(&shown(value))
            .unwrap_or_else(|error| format!("\"<{error}>\""))),
        Err(error) => Err(failed(&service, &method, error)),
    }
}

/// Why `service` cannot be called now, if it cannot: the row that provides
/// or declares it is not running (`forecast is not available (row weather
/// waits for its runtime go-weather)`), or no row provides it (the names
/// that are provided, and one close to it).
fn not_there(status: &Status, loader: &Loader, service: &str, provided: bool) -> Option<String> {
    let row = status
        .services
        .iter()
        .find(|provided| provided.name == service)
        .and_then(|provided| provided.provider.clone())
        .or_else(|| declaring(loader, service));
    let state = row.as_ref().and_then(|row| {
        status
            .rows
            .iter()
            .find(|listed| &listed.id == row && listed.instance.is_none())
    });
    if let (Some(row), Some(state)) = (&row, state) {
        if let Some(why) = not_running(&state.state, state.reason.as_deref()) {
            return Some(format!("{service} is not available (row {row} {why})"));
        }
    }
    if provided {
        return None;
    }
    let mut names: Vec<String> = status
        .services
        .iter()
        .map(|provided| provided.name.clone())
        .collect();
    names.dedup();
    let like = rutis::internal::config::closest(service, &names)
        .map(|like| format!("; did you mean {like}?"))
        .unwrap_or_default();
    let names = match names.is_empty() {
        true => "none".to_owned(),
        false => names.join(", "),
    };
    Some(format!(
        "no row provides {service}{like} (services: {names})"
    ))
}

/// The row that declares it provides `service` (its plugin's manifest),
/// for one that does not provide it now.
fn declaring(loader: &Loader, service: &str) -> Option<String> {
    loader
        .entries()
        .into_iter()
        .filter(|entry| entry.instance.is_none())
        .find(|entry| match entry.meta.get("provides") {
            Some(Json::Array(names)) => names.iter().any(|name| name == service),
            Some(Json::Object(names)) => names.contains_key(service),
            _ => false,
        })
        .map(|entry| entry.id)
}

/// What a row that is not running is doing instead, after `row <id> `:
/// `waits for its runtime go-weather`, `waits for llm (…)`, `failed in
/// apply: …`, `is disabled`. `None` for a row that runs or starts.
fn not_running(state: &EntryStatus, reason: Option<&str>) -> Option<String> {
    let fiber = match state {
        EntryStatus::Running(snapshot) => Some(snapshot.state),
        _ => None,
    };
    Some(match (state, fiber, reason) {
        (_, Some(FiberState::Active | FiberState::Loading), _) => return None,
        (_, Some(FiberState::Pending), Some(reason)) => {
            match reason.strip_prefix("waiting for runtime ") {
                Some(runtime) => {
                    let name = runtime.split(" (").next().unwrap_or(runtime);
                    format!("waits for its runtime {name}")
                }
                None => match reason.strip_prefix("waiting for ") {
                    Some(rest) => format!("waits for {rest}"),
                    None => reason.to_owned(),
                },
            }
        }
        (_, Some(FiberState::Pending), None) => "waits for its services".to_owned(),
        (EntryStatus::Unresolved(_), _, Some(reason))
        | (_, Some(FiberState::Failed), Some(reason))
            if reason.starts_with("failed") =>
        {
            reason.to_owned()
        }
        (EntryStatus::Unresolved(_), _, Some(reason))
        | (_, Some(FiberState::Failed), Some(reason)) => {
            format!("failed: {reason}")
        }
        (EntryStatus::Unresolved(_), _, None) | (_, Some(FiberState::Failed), None) => {
            "failed".to_owned()
        }
        (EntryStatus::Disabled, _, _) => "is disabled".to_owned(),
        (_, _, Some(reason)) => format!("is stopped ({reason})"),
        (_, _, None) => "is stopped".to_owned(),
    })
}

/// The end of the text a `#[rutis::service]` method taking the calling
/// row's `&Ctx` fails with when no row calls it.
const NO_CALLER: &str =
    "needs the calling row's context: only a row of a language runtime can call it";

/// A failed call as printed: its first line (as the diagnostics print it:
/// `call to forecast.today failed: …`, or the text that names the call
/// itself), then where it came from, indented. A call rutis refused (no
/// such method, a wrong argument) is the one line.
fn failed(service: &str, method: &str, error: session::Error) -> String {
    let text = error.to_string();
    if text.ends_with(NO_CALLER) {
        return format!(
            "{service}.{method} needs a calling row; call it from a plugin, not from rutis-host call"
        );
    }
    let refused = matches!(
        error.code(),
        Some(
            session::code::NO_METHOD
                | session::code::BAD_ARGUMENT
                | session::code::BAD_VALUE
                | session::code::UNAVAILABLE
                | session::code::NOT_DECLARED
        )
    );
    let error = rutis::host::ErrorKind::PluginFailed(Box::new(error));
    let headline = rutis_bridge::report::headline(&error).unwrap_or_else(|| {
        let text = rutis_bridge::report::printable(&text);
        match text.starts_with(&format!("{service}.{method}"))
            || text.starts_with(&format!("{service} "))
        {
            true => text,
            false => format!("call to {service}.{method} failed: {text}"),
        }
    });
    if refused {
        return headline;
    }
    let mut out = headline;
    for line in rutis_bridge::report::whereabouts(&error) {
        out.push_str("\n    ");
        out.push_str(&line);
    }
    out
}

/// A result as JSON for people: data as it is; a point in time, a date, a
/// big integer, a decimal, a UUID as its text; bytes, a map, a set, a
/// reference as what it is (`"<a map>"`). Not meant to be passed back.
pub fn shown(value: Value) -> Json {
    match value {
        Value::Undefined => Json::Null,
        Value::Data(data) => data,
        Value::List(values) => Json::Array(values.into_iter().map(shown).collect()),
        Value::Record(fields) => Json::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key, shown(value)))
                .collect(),
        ),
        // One that fits 64 bits is the number again.
        Value::BigInt(value) => match (
            value.to_i128().and_then(|n| i64::try_from(n).ok()),
            value.to_u128().and_then(|n| u64::try_from(n).ok()),
        ) {
            (Some(n), _) => Json::from(n),
            (None, Some(n)) => Json::from(n),
            _ => Json::String(value.to_string()),
        },
        Value::Instant(value) => Json::String(value.to_string()),
        Value::DateTime(value) => Json::String(value.to_string()),
        Value::Date(value) => Json::String(value.to_string()),
        Value::Time(value) => Json::String(value.to_string()),
        Value::Decimal(value) => Json::String(value.to_string()),
        Value::Uuid(value) => Json::String(value.to_string()),
        Value::Bytes(_) => Json::String("<bytes>".into()),
        Value::Map(_) => Json::String("<a map>".into()),
        Value::Set(_) => Json::String("<a set>".into()),
        Value::Reference(_) => Json::String("<a reference>".into()),
        Value::Signal => Json::String("<a signal>".into()),
        _ => Json::String("<a value>".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name splits at its last dot; each argument is JSON, and one
    /// that is not says how to write a string.
    #[test]
    fn the_command_line_names_one_call() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert_eq!(
            request("forecast.today", &args(&["\"Oslo\"", "3", "{\"days\": 3}"])).unwrap(),
            Request {
                service: "forecast".into(),
                method: "today".into(),
                args: vec![
                    Json::from("Oslo"),
                    Json::from(3),
                    serde_json::json!({ "days": 3 })
                ],
            }
        );
        let dotted = request("acme.forecast.today", &[]).unwrap();
        assert_eq!(
            (dotted.service.as_str(), dotted.method.as_str()),
            ("acme.forecast", "today")
        );
        assert_eq!(
            request("forecast.today", &args(&["Oslo"])).unwrap_err(),
            "argument 1 is not JSON: write a string in quotes, as '\"Oslo\"'"
        );
        assert_eq!(
            request("forecast", &[]).unwrap_err(),
            "name a method, as forecast.<method>"
        );
        assert_eq!(
            request("forecast.", &[]).unwrap_err(),
            "name a method, as forecast.<method>"
        );
    }

    /// A row that is not running says what it does instead.
    #[test]
    fn a_row_not_running_says_why() {
        let running = |state| {
            EntryStatus::Running(rutis::host::Snapshot {
                generation: 1,
                state,
                error: None,
            })
        };
        let why = |state: EntryStatus, reason| not_running(&state, reason);
        assert_eq!(
            why(
                running(FiberState::Pending),
                Some("waiting for runtime go-weather (down: exited with status 1)")
            )
            .as_deref(),
            Some("waits for its runtime go-weather")
        );
        assert_eq!(
            why(
                running(FiberState::Pending),
                Some("waiting for llm (no row provides it)")
            )
            .as_deref(),
            Some("waits for llm (no row provides it)")
        );
        assert_eq!(
            why(running(FiberState::Failed), Some("failed in apply: boom")).as_deref(),
            Some("failed in apply: boom")
        );
        assert_eq!(
            why(EntryStatus::Disabled, None).as_deref(),
            Some("is disabled")
        );
        assert_eq!(why(running(FiberState::Active), None), None);
    }

    /// Values JSON has are themselves; the others their text, or what
    /// they are.
    #[test]
    fn a_result_is_shown_as_json() {
        let date = Value::Date(session::Date::new(2026, 10, 27).unwrap());
        assert_eq!(shown(date), Json::from("2026-10-27"));
        assert_eq!(shown(Value::Map(Vec::new())), Json::from("<a map>"));
        assert_eq!(shown(Value::Bytes(vec![1])), Json::from("<bytes>"));
        assert_eq!(shown(Value::Undefined), Json::Null);
        assert_eq!(
            shown(Value::List(vec![
                Value::Data(Json::from(1)),
                Value::Set(Vec::new())
            ])),
            serde_json::json!([1, "<a set>"])
        );
    }

    /// A method taking the calling row's `&Ctx` says to call it from a
    /// plugin.
    #[test]
    fn a_method_that_needs_a_calling_row_says_so() {
        let error = session::Error::Value(format!("register {NO_CALLER}"));
        assert_eq!(
            failed("tools", "register", error),
            "tools.register needs a calling row; call it from a plugin, not from rutis-host call"
        );
    }
}
