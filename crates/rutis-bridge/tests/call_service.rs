//! `session::call_service` (#667): a host or a test calls a service by
//! name, as a row of another language would, in one line: a
//! `#[rutis::service]` type, a hand-written `HostDispatch` with a sync
//! method, and what fails.

use std::sync::Arc;

use rutis::host::CtxHost;
use rutis::Ctx;
use rutis_bridge::session::{call_service, host_key, HostDispatch, Reply, Value};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Forecast {
    city: String,
    days: u32,
}

#[derive(Debug, Serialize, thiserror::Error)]
pub enum WeatherError {
    #[error("no weather for {0}")]
    Unknown(String),
}

pub struct Weather;

#[rutis::service]
impl Weather {
    pub async fn today(&self, city: String) -> String {
        format!("sunny in {city}")
    }

    pub fn forecast(&self, city: String, days: u32) -> Result<Forecast, WeatherError> {
        match city.as_str() {
            "Atlantis" => Err(WeatherError::Unknown(city)),
            _ => Ok(Forecast { city, days }),
        }
    }

    pub fn ping(&self) {}
}

/// A hand-written service whose method is sync.
struct Clock;

impl HostDispatch for Clock {
    fn invoke(&self, method: &str, _args: Value) -> Reply {
        match method {
            "now" => Ok(json!(42).into()),
            other => Err(rutis_bridge::session::Error::Value(format!(
                "no method {other}"
            ))),
        }
    }

    fn methods(&self) -> Option<serde_json::Value> {
        Some(json!({ "now": "sync" }))
    }
}

/// guarantee: bridge/call-service
#[tokio::test]
async fn a_service_is_called_by_name() {
    let root = Ctx::root().unwrap();
    root.provide(Weather).unwrap();
    let today: String = call_service(&root, "weather", "today", ("Oslo",))
        .await
        .unwrap();
    assert_eq!(today, "sunny in Oslo");
    let forecast: Forecast = call_service(&root, "weather", "forecast", ("Bergen", 3))
        .await
        .unwrap();
    assert_eq!(
        forecast,
        Forecast {
            city: "Bergen".into(),
            days: 3
        }
    );
    // No arguments, no result; any result as JSON.
    call_service::<()>(&root, "weather", "ping", ())
        .await
        .unwrap();
    let any: serde_json::Value = call_service(&root, "weather", "today", vec!["Tromsø"])
        .await
        .unwrap();
    assert_eq!(any, json!("sunny in Tromsø"));

    // A sync method of a hand-written service.
    root.provide_at(host_key("clock"), Arc::new(Clock) as Arc<dyn HostDispatch>)
        .unwrap();
    let now: u64 = call_service(&root, "clock", "now", ()).await.unwrap();
    assert_eq!(now, 42);
    root.shutdown().await.unwrap();
}

/// What fails is an `Err`: the method's own error (with its code), an
/// unknown method, a service not provided, arguments that are not a list.
/// guarantee: bridge/call-service
#[tokio::test]
async fn what_fails_is_an_error() {
    let root = Ctx::root().unwrap();
    root.provide(Weather).unwrap();
    let error = call_service::<Forecast>(&root, "weather", "forecast", ("Atlantis", 1))
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("Unknown"), "{error}");
    assert!(
        error.to_string().contains("no weather for Atlantis"),
        "{error}"
    );

    let error = call_service::<()>(&root, "weather", "tomorrow", ())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no method tomorrow"), "{error}");

    let error = call_service::<()>(&root, "storage", "get", ())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "ServiceNotFound: no service named storage is provided"
    );

    let error = call_service::<String>(&root, "weather", "today", "Oslo")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("a tuple or a list"), "{error}");
    root.shutdown().await.unwrap();
}
