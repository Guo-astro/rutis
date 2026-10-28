//! S1, the host writes its own plugin (#185; design "The host writes its
//! own plugins" §八): a Python plugin `maker`, standing in for an agent,
//! writes a plugin `weather`, has the host load it through the service
//! `loader`, breaks it and is told why while the old version keeps
//! serving, fixes it, and removes it.
//!
//! `maker` injects `loader`, provides `maker`, and `step(n)` does step `n`
//! of a fixed script. The test drives it with `rutis-host call maker.step
//! <n>` and uses the new service with `rutis-host call weather.today
//! '"Oslo"'`.
//!
//! | step | `maker.step(n)` | checked |
//! | --- | --- | --- |
//! | 1 | writes `weather.py` (`today` is `"sunny (v1)"`) | the file is there; no `weather` in `status()` |
//! | 2 | `create({"id": "weather", "name": "py:weather"})` | `"weather"`; the row in `rutis.json`; `weather: running` |
//! | 3 | (the test) `weather.today` | `"sunny (v1)"` |
//! | 4 | a syntax error, `reload("weather")` | the error's text; `weather.today` stays v1, every 100 ms |
//! | 5 | `"rainy (v2)"`, `reload("weather")` | returns; `weather.today` is v2 |
//! | 6 | `remove("weather")`, `status()` | no `weather`; `rutis.json` as at step 1, byte for byte; `weather: removed`; `weather.today` has no provider |
//!
//! Unix only: `rutis-host call` is not available on Windows.
#![cfg(unix)]

use std::time::Duration;

use rutis_e2e::{host_program, Host, Scenario, Stream};
use serde_json::{json, Value};

/// The scripted plugin: each step returns what the test checks.
const MAKER: &str = r#"
import pathlib

inject = ["loader"]
WEATHER = pathlib.Path(__file__).parent / "weather.py"

V1 = '''class Weather:
    async def today(self, city):
        return "sunny (v1)"


def apply(ctx, config):
    ctx.provide("weather", Weather())
'''
BROKEN = V1.replace("def apply(ctx, config):", "def apply(ctx, config)")
V2 = V1.replace("sunny (v1)", "rainy (v2)")


class Maker:
    def __init__(self, loader):
        self.loader = loader

    async def seen(self):
        status = await self.loader.status()
        return {
            "rows": [row["id"] for row in status["rows"]],
            "services": [service["name"] for service in status["services"]],
        }

    async def step(self, n):
        if n == 1:
            WEATHER.write_text(V1)
            return await self.seen()
        if n == 2:
            return await self.loader.create({"id": "weather", "name": "py:weather"})
        if n == 4:
            WEATHER.write_text(BROKEN)
            try:
                await self.loader.reload("weather")
            except Exception as error:
                return {
                    "name": error.name,
                    "code": error.code,
                    "data": error.data,
                    "message": error.message,
                }
            return "reloaded"
        if n == 5:
            WEATHER.write_text(V2)
            return await self.loader.reload("weather")
        if n == 6:
            await self.loader.remove("weather")
            return await self.seen()


def apply(ctx, config):
    ctx.provide("maker", Maker(ctx.use("loader")))
"#;

/// `rutis-host call <args>` in the project, started (not waited for).
fn start_call(s: &Scenario, args: &[&str]) -> Host {
    let mut all = vec!["call"];
    all.extend_from_slice(args);
    s.program("", host_program(), all, &[])
}

/// Wait for a started `rutis-host call`: its exit code, stdout, stderr.
fn finish_call(mut call: Host) -> (Option<i32>, String, String) {
    let status = call.wait_exit();
    call.wait_output_end();
    let text = |stream| {
        call.lines()
            .iter()
            .filter(|line| line.stream == stream)
            .map(|line| format!("{}\n", line.text))
            .collect::<String>()
    };
    (status.code(), text(Stream::Stdout), text(Stream::Stderr))
}

/// `rutis-host call <args>`: its exit code, stdout, stderr.
fn call(s: &Scenario, args: &[&str]) -> (Option<i32>, String, String) {
    finish_call(start_call(s, args))
}

/// `rutis-host call <args>` that succeeds: its result.
fn result(s: &Scenario, args: &[&str]) -> Value {
    let (code, stdout, stderr) = call(s, args);
    assert_eq!(code, Some(0), "rutis-host call {args:?}: {stderr}");
    serde_json::from_str(&stdout).unwrap_or_else(|error| panic!("{stdout:?}: {error}"))
}

/// `weather.today("Oslo")`.
fn today(s: &Scenario) -> (Option<i32>, String, String) {
    call(s, &["weather.today", "\"Oslo\""])
}

/// Wait for the host's line `line`, exactly.
fn expect_line(host: &mut Host, line: &str) {
    host.wait_for(&format!("the line {line:?}"), |text| text == line);
}

/// guarantee: loader/service, host/rows-written-back
#[test]
fn a_plugin_writes_loads_fixes_and_removes_a_plugin_while_the_host_runs() {
    let s = Scenario::new("host-writes");
    s.write("maker.py", MAKER);
    let config = json!({
        "id": "s1",
        "runtimes": { "py": { "project": "." } },
        "rows": [{ "id": "maker", "name": "py:maker" }],
    });
    // As rutis-host writes it: write-back keeps the fields and their
    // order, not a hand-made layout.
    let step_1 = format!("{}\n", serde_json::to_string_pretty(&config).unwrap());
    s.write("rutis.json", &step_1);
    let mut host = s.host(["run", "rutis.json"]);
    expect_line(&mut host, "maker: running");

    // 1: the plugin's code is written; no row runs it yet.
    let seen = result(&s, &["maker.step", "1"]);
    assert!(s.path("weather.py").is_file());
    assert_eq!(
        seen,
        json!({ "rows": ["maker"], "services": ["loader", "maker"] })
    );

    // 2: the row is added, runs, and is written into rutis.json.
    assert_eq!(result(&s, &["maker.step", "2"]), json!("weather"));
    expect_line(&mut host, "weather: running");
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(s.path("rutis.json")).unwrap()).unwrap();
    let mut expected = config.clone();
    expected["rows"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "weather", "name": "py:weather" }));
    assert_eq!(written, expected);

    // 3: the new service answers.
    assert_eq!(
        today(&s),
        (Some(0), "\"sunny (v1)\"\n".into(), String::new())
    );

    // 4: broken code is refused; v1 keeps answering every 100 ms meanwhile.
    let mut step = start_call(&s, &["maker.step", "4"]);
    loop {
        let done = step.exited();
        assert_eq!(
            today(&s),
            (Some(0), "\"sunny (v1)\"\n".into(), String::new()),
            "weather.today while the broken code is reloaded"
        );
        if done {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let (code, stdout, stderr) = finish_call(step);
    assert_eq!(code, Some(0), "{stderr}");
    let refused: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(refused["name"], "Error", "{refused}");
    assert_eq!(refused["code"], "Error", "{refused}");
    assert_eq!(refused["data"], json!({ "row": "weather" }), "{refused}");
    let message = refused["message"].as_str().unwrap();
    assert!(
        message.starts_with("weather: cannot reload: cannot load weather.py: SyntaxError")
            && message.contains("weather.py")
            && message.ends_with("; the previous version keeps running"),
        "{message}"
    );
    assert_eq!(
        today(&s),
        (Some(0), "\"sunny (v1)\"\n".into(), String::new())
    );

    // 5: the fixed code replaces it.
    assert_eq!(result(&s, &["maker.step", "5"]), Value::Null);
    for _ in 0..3 {
        assert_eq!(
            today(&s),
            (Some(0), "\"rainy (v2)\"\n".into(), String::new())
        );
    }

    // 6: removed: gone from the host and from rutis.json.
    let seen = result(&s, &["maker.step", "6"]);
    assert_eq!(
        seen,
        json!({ "rows": ["maker"], "services": ["loader", "maker"] })
    );
    expect_line(&mut host, "weather: removed");
    assert_eq!(
        std::fs::read_to_string(s.path("rutis.json")).unwrap(),
        step_1
    );
    assert_eq!(
        today(&s),
        (
            Some(1),
            String::new(),
            "rutis-host: no row provides weather (services: loader, maker)\n".into()
        )
    );

    // No line of the host's says a row failed: the broken code never ran.
    let failed: Vec<String> = host
        .lines()
        .into_iter()
        .map(|line| line.text)
        .filter(|text| text.contains("failed"))
        .collect();
    assert!(failed.is_empty(), "{failed:#?}");

    host.ctrl_c();
    let status = host.wait_exit();
    assert_eq!(status.code(), Some(0));
    s.finish();
}
