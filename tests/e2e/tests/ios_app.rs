//! An iOS app hosting rutis plugins, on the iOS simulator (Swift A1, #389;
//! design `docs/design-swift-sdk-2026-10-12.md` §七, §八): the sample app
//! `swift/examples/HealthApp` (a HealthKit plugin `health` and a plugin
//! `device`, `RutisApp` started in the foreground and stopped in the
//! background), run by `rutis-host run` with a Python row using its
//! services in each app's instance.
//!
//! - Two simulators run the app, each with an id of its own: each is an
//!   instance, and the Python row in each sees its own app's `device`.
//! - The app's business layer stops rutis in the background and starts it
//!   in the foreground: its instance's Python row stops and comes back,
//!   the other app's runs on.
//! - HealthKit access belongs to the app: before the user decides, the
//!   plugin's refusal reaches the Python caller as its error (name,
//!   message, `code`); asking shows the system's sheet over the app, which
//!   names the app (read off a screenshot).
//!
//! The simulator cannot answer the sheet, nor grant or revoke HealthKit
//! (`simctl privacy` has no HealthKit service): a denial and a grant are
//! the manual checklist of docs/guide/swift-app.md §8, as is everything on
//! a real device.
//!
//! Needs Xcode with an iOS simulator runtime (`xcodebuild -downloadPlatform
//! iOS`): `cargo test -p rutis-e2e --test ios_app -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use rutis_e2e::{build_guard, hang_guard, repo, Scenario};
use serde_json::{json, Value};

const BUNDLE: &str = "dev.rutis.example.health";

/// The Python row in each app's instance: records what it sees (its app's
/// id, HealthKit's status, the steps or the error reading them) in
/// `<dir>/<id>.json` while it runs; with `<dir>/authorize` there, it asks
/// for access (the sheet), recording the answer in `<id>.authorize.json`.
const COACH: &str = r#"
import asyncio
import json
import os

inject = ["health", "device"]

# The calls asking for access, kept until they end.
asking = set()


def write(path, record):
    with open(path + ".tmp", "w") as file:
        json.dump(record, file)
    os.replace(path + ".tmp", path)


def failure(error):
    return {"error": str(error), "name": getattr(error, "name", None), "code": getattr(error, "code", None)}


async def apply(ctx, config):
    out = config["dir"]
    health = ctx.use("health")
    me = ctx.use("device").id()
    record = {"id": me, "status": health.status()}
    try:
        record["steps"] = await health.stepsToday()
    except Exception as error:
        record.update(failure(error))
    path = os.path.join(out, me + ".json")
    write(path, record)
    ctx.effect(lambda: os.remove(path))
    if os.path.exists(os.path.join(out, "authorize")):
        async def ask():
            try:
                answer = {"granted": await health.authorize()}
            except Exception as error:
                answer = failure(error)
            write(os.path.join(out, me + ".authorize.json"), answer)

        task = asyncio.ensure_future(ask())
        asking.add(task)
        task.add_done_callback(asking.discard)
"#;

fn xcrun(args: &[&str]) -> Output {
    let output = Command::new("xcrun")
        .args(args)
        .output()
        .expect("xcrun (Xcode) is on PATH");
    assert!(
        output.status.success(),
        "xcrun {}: {}{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn json_of(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("simctl's JSON")
}

/// The newest iOS simulator runtime, and an iPhone it supports.
fn runtime() -> (String, String) {
    let listed = json_of(&xcrun(&["simctl", "list", "runtimes", "-j"]));
    let runtime = listed["runtimes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|runtime| runtime["platform"] == "iOS" && runtime["isAvailable"] == true)
        .max_by_key(|runtime| runtime["version"].as_str().unwrap_or("").to_owned())
        .unwrap_or_else(|| {
            panic!("no iOS simulator runtime: install one with `xcodebuild -downloadPlatform iOS`")
        });
    let iphone = runtime["supportedDeviceTypes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|kind| kind["productFamily"] == "iPhone")
        .last()
        .expect("an iPhone the runtime supports");
    (
        runtime["identifier"].as_str().unwrap().to_owned(),
        iphone["identifier"].as_str().unwrap().to_owned(),
    )
}

/// A simulator of its own, deleted when dropped.
struct Simulator {
    udid: String,
    /// The app's output (the SDK's and the app's lines).
    log: PathBuf,
    /// `simctl launch --console-pty`, writing the app's output to `log`
    /// while the app runs (`--stderr=<file>` writes nothing on iOS 26.5).
    console: std::cell::RefCell<Option<std::process::Child>>,
}

impl Simulator {
    fn new(name: &str, runtime: &(String, String), log: PathBuf) -> Self {
        let created = xcrun(&["simctl", "create", name, &runtime.1, &runtime.0]);
        let udid = String::from_utf8(created.stdout).unwrap().trim().to_owned();
        let simulator = Simulator {
            udid,
            log,
            console: Default::default(),
        };
        xcrun(&["simctl", "boot", &simulator.udid]);
        xcrun(&["simctl", "bootstatus", &simulator.udid, "-b"]);
        simulator
    }

    fn install(&self, app: &Path) {
        xcrun(&["simctl", "install", &self.udid, app.to_str().unwrap()]);
    }

    /// Starts the app as `id`, dialing `address`; its output goes to `log`.
    fn launch(&self, address: &str, id: &str) {
        let log = std::fs::File::create(&self.log).unwrap();
        let console = Command::new("xcrun")
            .args([
                "simctl",
                "launch",
                "--terminate-running-process",
                "--console-pty",
            ])
            .args([&self.udid, BUNDLE])
            .args([
                "-RutisConnect",
                address,
                "-RutisID",
                id,
                "-RutisToken",
                "shared",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("xcrun simctl launch");
        if let Some(mut old) = self.console.replace(Some(console)) {
            let _ = old.kill();
            let _ = old.wait();
        }
    }

    /// Another app in front: the app goes to the background.
    fn background(&self) {
        xcrun(&["simctl", "launch", &self.udid, "com.apple.Preferences"]);
    }

    /// The app in front again.
    fn foreground(&self) {
        xcrun(&["simctl", "launch", &self.udid, BUNDLE]);
    }

    fn terminate(&self) {
        let _ = Command::new("xcrun")
            .args(["simctl", "terminate", &self.udid, BUNDLE])
            .output();
    }

    /// The text on the screen.
    fn text(&self, dir: &Path) -> String {
        let shot = dir.join(format!("{}.png", self.udid));
        xcrun(&[
            "simctl",
            "io",
            &self.udid,
            "screenshot",
            shot.to_str().unwrap(),
        ]);
        let script = repo().join("tests/e2e/ios/text.swift");
        let read = xcrun(&["swift", script.to_str().unwrap(), shot.to_str().unwrap()]);
        String::from_utf8_lossy(&read.stdout).into_owned()
    }

    fn lines(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Simulator {
    fn drop(&mut self) {
        if let Some(mut console) = self.console.take() {
            let _ = console.kill();
            let _ = console.wait();
        }
        if std::thread::panicking() {
            eprintln!("simulator {}: the app said:\n{}", self.udid, self.lines());
        }
        let _ = Command::new("xcrun")
            .args(["simctl", "shutdown", &self.udid])
            .output();
        let _ = Command::new("xcrun")
            .args(["simctl", "delete", &self.udid])
            .output();
    }
}

/// The first `Some` of `check` within `guard`.
fn eventually<T>(guard: Duration, what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + guard;
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out ({guard:?}): {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn record(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Builds the sample app for the simulator: its `.app`.
fn build(s: &Scenario) -> PathBuf {
    let derived = s.path("derived");
    let project = repo().join("swift/examples/HealthApp/HealthApp.xcodeproj");
    let mut build = s.program(
        ".",
        "xcodebuild",
        [
            "-project",
            project.to_str().unwrap(),
            "-scheme",
            "HealthApp",
            "-destination",
            "generic/platform=iOS Simulator",
            "-derivedDataPath",
            derived.to_str().unwrap(),
            "-skipMacroValidation",
            "-quiet",
            "build",
        ],
        &[],
    );
    assert!(
        build.wait_exit_within(build_guard()).success(),
        "xcodebuild the sample app"
    );
    derived.join("Build/Products/Debug-iphonesimulator/HealthApp.app")
}

/// guarantee: swift/app-start-stop, swift/app-access-belongs-to-the-app
#[test]
#[ignore = "needs macOS, Xcode and an iOS simulator runtime: cargo test -p rutis-e2e --test ios_app -- --ignored"]
fn an_ios_app_hosts_plugins_per_instance_stops_in_the_background_and_its_health_access_is_its_own()
{
    let s = Scenario::new("ios-app");
    let runtime = runtime();
    let app = build(&s);
    let simulators = [("sim-a", "a"), ("sim-b", "b")].map(|(id, name)| {
        let simulator = Simulator::new(
            &format!("rutis-e2e-{}-{name}", std::process::id()),
            &runtime,
            s.path(format!("{id}.log")),
        );
        simulator.install(&app);
        (id, simulator)
    });

    let records = s.path("records");
    std::fs::create_dir_all(&records).unwrap();
    s.write("coach.py", COACH);
    s.write_json(
        "rutis.json",
        &json!({
            "id": "main",
            "listen": [{ "name": "public", "address": "127.0.0.1:0" }],
            "runtimes": {
                "py": { "project": "." },
                "remote": [{ "name": "phone", "language": "swift", "listen": "public", "instances": "phone" }]
            },
            "rows": [{ "id": "phone", "group": true, "instanced": true, "config": [
                { "id": "health", "name": "phone:health" },
                { "id": "device", "name": "phone:device" },
                { "id": "coach", "name": "py:coach", "config": { "dir": records } }
            ] }]
        }),
    );
    let mut host = s.host_with(["run", "rutis.json"], &[("RUTIS_TOKEN_PHONE", "shared")]);
    let listening = s.listening(&mut host, "public");
    let address = format!("ws://127.0.0.1:{}/rutis", listening.port());
    for (id, simulator) in &simulators {
        simulator.launch(&address, id);
    }
    let seen = |id: &str| record(&records.join(format!("{id}.json")));

    // Each app is an instance: its row sees its own app, and HealthKit not
    // decided yet is the plugin's refusal, reaching the Python caller.
    for (id, simulator) in &simulators {
        let record = eventually(build_guard(), &format!("{id}'s row"), || seen(id));
        assert_eq!(record["id"], *id, "{record}");
        let status = record["status"].as_str().unwrap_or("");
        assert_eq!(
            status, "not-determined",
            "HealthKit on the simulator: {record}"
        );
        assert_eq!(record["name"], "HealthAccessDenied", "{record}");
        assert_eq!(record["code"], "not-determined", "{record}");
        assert_eq!(
            record["error"],
            "HealthAccessDenied: step counts are not shared with this app (not-determined)",
            "{record}"
        );
        assert!(
            simulator.lines().contains("app: state connected"),
            "{}",
            simulator.lines()
        );
    }

    // sim-a goes to the background: its business layer stops rutis, and
    // its instance's row stops; sim-b's runs on.
    let (a, b) = (&simulators[0].1, &simulators[1].1);
    a.background();
    eventually(hang_guard(), "sim-a's row stopped", || {
        seen("sim-a").is_none().then_some(())
    });
    assert!(seen("sim-b").is_some(), "sim-b's row runs on");
    eventually(hang_guard(), "sim-a stopped", || {
        a.lines().contains("app: state stopped").then_some(())
    });
    // In front again: started, and the row back.
    a.foreground();
    eventually(hang_guard(), "sim-a's row back", || seen("sim-a"));

    // Asking for access shows the system's sheet over the app, naming it.
    std::fs::write(records.join("authorize"), "").unwrap();
    a.background();
    eventually(hang_guard(), "sim-a's row stopped", || {
        seen("sim-a").is_none().then_some(())
    });
    a.foreground();
    eventually(hang_guard(), "sim-a's row back", || seen("sim-a"));
    std::fs::remove_file(records.join("authorize")).unwrap();
    let text = eventually(hang_guard(), "the HealthKit sheet", || {
        let text = a.text(s.dir());
        (text.contains("Rutis Health") && text.contains("Rutis Health records steps"))
            .then_some(text)
    });
    assert!(
        text.contains("Rutis Health records steps"),
        "the sheet shows the app's usage description (in any language): {text}"
    );
    assert!(
        record(&s.path("records/sim-a.authorize.json")).is_none(),
        "the call waits for the user's answer"
    );

    // The app ends: its instance closes, the waiting call fails.
    a.terminate();
    eventually(hang_guard(), "sim-a's row stopped", || {
        seen("sim-a").is_none().then_some(())
    });
    let answer = eventually(hang_guard(), "the call asking ended", || {
        record(&s.path("records/sim-a.authorize.json"))
    });
    assert!(answer["error"].is_string(), "{answer}");
    b.terminate();
    eventually(hang_guard(), "sim-b's row stopped", || {
        seen("sim-b").is_none().then_some(())
    });

    host.ctrl_c();
    assert_eq!(host.wait_exit().code(), Some(0));
    host.wait_output_end();
    drop(simulators);
    s.finish();
}
