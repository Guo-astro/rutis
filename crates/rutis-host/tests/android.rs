//! An Android application hosting rutis plugins, on a device or emulator
//! (feature `android`; needs `adb` with one device attached, or
//! `ANDROID_SERIAL`; JDK 17 and the Android SDK for the Gradle build;
//! Python 3.10+): kotlin/rutis/android-sample dials this test's host
//! through `adb reverse`, `language: "kotlin"`, each connection an
//! instance of the group `phone` with a `device` row from the application
//! and a Python row using it.
//!
//! - Its release build (R8, as an application ships), started and stopped
//!   by intents as a business layer would: the method tables, proxies and
//!   values found at run time as they are in a shrunk application.
//! - Its device test `AppHostingTest` (the debug build) with two
//!   connections of the application: it starts and stops them, this test
//!   checks rutis's side between its steps and tells it to go on with a
//!   broadcast.
//!
//! `RUTIS_ANDROID_APKS` names a directory with the APKs built beforehand
//! (`android-sample-release.apk`, `android-sample-debug.apk`,
//! `android-sample-debug-androidTest.apk`); otherwise they are built here
//! with the Gradle wrapper.
#![cfg(feature = "android")]
// The tests hold the environment until they end, across their awaits: the
// variables they set are for the host they start, and the device is one.
#![allow(clippy::await_holding_lock)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use common::{environment, repo, write};
use rutis_host::config::HostConfig;
use rutis_host::host::Host;
use rutis_test_support::{Guard, HANG_GUARD};
use serde_json::json;
use tokio::io::AsyncBufReadExt;

/// An emulator is slow to start a process: a minute for each step.
const DEVICE: Guard = Guard(Duration::from_secs(60));

/// The application's package, and the instrumentation of its device tests.
const PACKAGE: &str = "arcships.rutis.sample";
const RUNNER: &str = "arcships.rutis.sample.test/androidx.test.runner.AndroidJUnitRunner";

/// The host's row in each instance: it loads only once the connection's
/// `device` answered with what an Android device is.
const WATCHER: &str = r#"
inject = ["device"]


async def apply(ctx, config):
    info = await ctx.use("device").info()
    if info["sdk"] < 29 or not info["model"]:
        raise RuntimeError("not an Android device: %r" % (info,))
"#;

fn adb() -> Command {
    let sdk = std::env::var_os("ANDROID_HOME").map(PathBuf::from);
    let tool = sdk
        .map(|sdk| sdk.join("platform-tools").join("adb"))
        .filter(|adb| adb.exists())
        .unwrap_or_else(|| PathBuf::from("adb"));
    Command::new(tool)
}

/// `adb <args>`, which must succeed: its output.
fn run(args: &[&str]) -> String {
    let output = adb().args(args).output().expect("adb runs");
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "adb {args:?}: {text}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    text
}

/// The directory of the APKs, built once (or given).
fn apks() -> &'static Path {
    static APKS: OnceLock<PathBuf> = OnceLock::new();
    APKS.get_or_init(|| {
        if let Some(dir) = std::env::var_os("RUTIS_ANDROID_APKS") {
            return PathBuf::from(dir);
        }
        let project = repo().join("kotlin/rutis");
        let status = Command::new(project.join("gradlew"))
            .args([
                "--quiet",
                ":android-sample:assembleRelease",
                ":android-sample:assembleDebug",
                ":android-sample:assembleDebugAndroidTest",
            ])
            .current_dir(&project)
            .status()
            .expect("the Gradle wrapper runs (JDK 17, an Android SDK)");
        assert!(status.success(), "gradlew :android-sample:assemble…");
        let outputs = project.join("android-sample/build/outputs/apk");
        let dir = tempfile::tempdir().unwrap().keep();
        for apk in [
            "release/android-sample-release.apk",
            "debug/android-sample-debug.apk",
            "androidTest/debug/android-sample-debug-androidTest.apk",
        ] {
            let apk = outputs.join(apk);
            std::fs::copy(&apk, dir.join(apk.file_name().unwrap())).unwrap();
        }
        dir
    })
}

/// Installs the APKs `names` (of [`apks`]), the application stopped first.
async fn install(names: &[&str]) {
    let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let dir = apks();
        let _ = adb().args(["shell", "am", "force-stop", PACKAGE]).status();
        for name in names {
            run(&["install", "-r", "-t", dir.join(name).to_str().unwrap()]);
        }
    })
    .await
    .unwrap();
}

/// The host: the group `phone`, an instance per connection of the
/// application, with the rows `rows` (by default its `device` row and the
/// Python `watcher` using it) and the Python files `files`; forwarded to
/// the device at the same port.
struct Rutis {
    host: Host,
    port: u16,
    _dir: tempfile::TempDir,
}

impl Rutis {
    async fn start() -> Self {
        Self::with(
            json!([
                { "id": "device", "name": "phone:device" },
                { "id": "watcher", "name": "py:watcher" }
            ]),
            &[("watcher.py", WATCHER)],
        )
        .await
    }

    async fn with(rows: serde_json::Value, files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), files);
        std::env::set_var("RUTIS_TOKEN_PHONE", "shared");
        std::env::set_var("RUTIS_PYTHON_PATH", repo().join("python/rutis"));
        let python = std::env::var_os("RUTIS_PYTHON").map(PathBuf::from);
        let mut config: HostConfig = serde_json::from_value(json!({
            "id": "test",
            "listen": [{ "name": "public", "address": "127.0.0.1:0" }],
            "runtimes": {
                "py": { "project": dir.path(), "python": python },
                "remote": [{ "name": "phone", "language": "kotlin", "listen": "public", "instances": "phone" }]
            },
            "rows": [{ "id": "phone", "group": true, "instanced": true, "config": rows }]
        }))
        .unwrap();
        config.rebase(dir.path());
        let host = Host::start(&config).await.unwrap();
        Guard(2 * HANG_GUARD)
            .within("the Python runtime", host.runtimes_ready())
            .await
            .unwrap();
        host.load(config.rows()).await.unwrap();
        let port = host.listening[0].1.port();
        let forward = format!("tcp:{port}");
        run(&["reverse", &forward, &forward]);
        Self {
            host,
            port,
            _dir: dir,
        }
    }

    /// The address the device dials.
    fn address(&self) -> String {
        format!("ws://127.0.0.1:{}/rutis", self.port)
    }

    /// The copies of row `id` running in an instance.
    fn active(&self, id: &str) -> usize {
        self.host
            .loader
            .entries()
            .into_iter()
            .filter(|entry| entry.id == id && entry.instance.is_some())
            .filter(|entry| {
                matches!(&entry.status, rutis_loader::EntryStatus::Running(snapshot)
                    if snapshot.state == rutis::FiberState::Active)
            })
            .count()
    }

    /// Instances whose `device` row runs, and whose watcher uses it.
    fn instances(&self) -> (usize, usize) {
        (self.active("device"), self.active("watcher"))
    }

    /// Waits until each row of `want` runs in that many instances.
    async fn reach_rows(&self, want: &[(&str, usize)]) {
        DEVICE
            .eventually_with(
                || {
                    want.iter()
                        .all(|(id, count)| self.active(id) == *count)
                        .then_some(())
                },
                &format!("{want:?}"),
                || self.rows(),
            )
            .await
    }

    /// The rows of the instances, for a failure.
    fn rows(&self) -> String {
        self.host
            .loader
            .entries()
            .into_iter()
            .filter(|entry| entry.instance.is_some())
            .map(|entry| format!("{}: {:?}\n", entry.id, entry.status))
            .collect()
    }

    /// Waits until [`Rutis::instances`] is `want`; `ended` says why to
    /// stop waiting at once (what ended on the device), if anything.
    async fn reach(&self, want: (usize, usize), what: &str, ended: impl Fn() -> Option<String>) {
        DEVICE
            .eventually_with(
                || {
                    if self.instances() == want {
                        return Some(());
                    }
                    if let Some(why) = ended() {
                        panic!("{what}: {why}\n{}", self.rows());
                    }
                    None
                },
                what,
                || self.rows(),
            )
            .await
    }

    async fn stop(self) {
        let forward = format!("tcp:{}", self.port);
        let _ = adb().args(["reverse", "--remove", &forward]).status();
        self.host.shut_down(HANG_GUARD).await.ok();
    }
}

/// The release build, R8 on, started and stopped as its business layer
/// would (here, intents pressing its buttons): its instance comes, its
/// `device` is used through what R8 kept, and it goes and comes back.
/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn the_release_build_dials_in_and_stops_and_starts_again() {
    let _environment = environment();
    install(&["android-sample-release.apk"]).await;
    // Granted already: the screen asks for nothing.
    grant(&[
        "android.permission.READ_CONTACTS",
        "android.permission.POST_NOTIFICATIONS",
    ]);
    let rutis = Rutis::start().await;
    let command = |command: &str| press(&rutis.address(), command);

    command("start");
    rutis
        .reach((1, 1), "the application's instance, used", || None)
        .await;
    command("stop");
    rutis.reach((0, 0), "its instance closed", || None).await;
    command("start");
    rutis.reach((1, 1), "its instance back", || None).await;
    command("stop");
    rutis.reach((0, 0), "its instance closed", || None).await;
    let _ = adb().args(["shell", "am", "force-stop", PACKAGE]).status();
    rutis.stop().await;
}

/// Presses a button of the application's screen (`start`, `stop`), with an
/// intent that also fills in where rutis is.
fn press(address: &str, command: &str) {
    let activity = format!("{PACKAGE}/.MainActivity");
    run(&[
        "shell", "am", "start", "-W", "-n", &activity, "--es", "connect", address, "--es", "id",
        "pixel", "--es", "token", "shared", "--es", "command", command,
    ]);
}

/// Grants the application `permissions`, as its user would.
fn grant(permissions: &[&str]) {
    for permission in permissions {
        run(&["shell", "pm", "grant", PACKAGE, permission]);
    }
}

/// Tells the device test that `step` is checked.
fn checked(step: &str) {
    run(&[
        "shell",
        "am",
        "broadcast",
        "-a",
        "arcships.rutis.sample.test.STEP",
        "-p",
        PACKAGE,
        "--es",
        "step",
        step,
    ]);
}

/// Two connections of the application (`AppHostingTest`) are two
/// instances of rutis's group, each with its own `device` row and a Python
/// row using it; stopping one closes its instance alone, and its user with
/// it; starting it again brings them back.
/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn connections_of_an_application_are_instances() {
    let _environment = environment();
    install(&[
        "android-sample-debug.apk",
        "android-sample-debug-androidTest.apk",
    ])
    .await;
    let rutis = Rutis::start().await;
    let mut instrument = tokio::process::Command::from(adb())
        .args(["shell", "am", "instrument", "-w", "-r"])
        .args(["-e", "address", &rutis.address()])
        .args(["-e", "token", "shared"])
        .args(["-e", "class", "arcships.rutis.sample.AppHostingTest"])
        .arg(RUNNER)
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    // What the device test says; a test that ends early fails this one at
    // once, with its words.
    let said = Arc::new(Mutex::new(String::new()));
    let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut lines = tokio::io::BufReader::new(instrument.stdout.take().unwrap()).lines();
    let (said_by, ended_by) = (said.clone(), ended.clone());
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("device: {line}");
            let mut said = said_by.lock().unwrap();
            *said += &line;
            said.push('\n');
        }
        ended_by.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let device_ended = || {
        ended
            .load(std::sync::atomic::Ordering::SeqCst)
            .then(|| format!("the device test ended: {}", said.lock().unwrap()))
    };

    rutis
        .reach((2, 2), "two instances, each used", device_ended)
        .await;
    checked("connected");
    rutis
        .reach((1, 1), "the stopped one's instance closed", device_ended)
        .await;
    checked("stopped");
    rutis
        .reach((2, 2), "the started one's instance back", device_ended)
        .await;
    checked("started");
    // The device test ends once it stopped both: rutis sees the instances
    // close just after.
    rutis.reach((0, 0), "both stopped", || None).await;

    let status = DEVICE
        .within("the device test's end", instrument.wait())
        .await
        .unwrap();
    // The last lines, read after the exit.
    DEVICE
        .until(
            || ended.load(std::sync::atomic::Ordering::SeqCst),
            "its output",
        )
        .await;
    let said = said.lock().unwrap().clone();
    assert!(status.success(), "{said}");
    assert!(said.contains("OK (1 test)"), "the device test: {said}");
    rutis.stop().await;
}

/// Uses the connection's `contacts`: it loads once a call answered.
const READER: &str = r#"
inject = ["contacts"]


async def apply(ctx, config):
    await ctx.use("contacts").count()
"#;

/// Posts through the connection's `notify`, and says what came of it
/// (`caller.outcome()`): `posted`, or the error's code and permission.
const CALLER: &str = r#"
import rutis

inject = ["notify"]


class Caller:
    def __init__(self, said):
        self.said = said

    def outcome(self):
        return self.said


provides = {"caller": Caller}


async def apply(ctx, config):
    try:
        await ctx.use("notify").post("rutis", "from the caller")
        said = "posted"
    except rutis.RemoteError as error:
        said = "%s %s" % (error.code, (error.data or {}).get("permission"))
    ctx.provide("caller", Caller(said))
"#;

/// The screen as uiautomator sees it.
fn screen() -> String {
    run(&["shell", "uiautomator", "dump", "/sdcard/rutis-ui.xml"]);
    run(&["shell", "cat", "/sdcard/rutis-ui.xml"])
}

/// The centre of the node `resource-id` on `screen`, if it is there.
fn find(screen: &str, id: &str) -> Option<(i32, i32)> {
    let node = &screen[screen.find(&format!("resource-id=\"{id}\""))?..];
    let bounds = &node[node.find("bounds=\"[")? + 9..];
    let numbers: Vec<i32> = bounds[..bounds.find("]\"")?]
        .split(['[', ']', ','])
        .filter(|n| !n.is_empty())
        .map(|n| n.parse().unwrap())
        .collect();
    Some(((numbers[0] + numbers[2]) / 2, (numbers[1] + numbers[3]) / 2))
}

/// The text of the node `resource-id` on `screen`.
fn text(screen: &str, id: &str) -> String {
    let Some(at) = screen.find(&format!("resource-id=\"{id}\"")) else {
        return String::new();
    };
    let open = screen[..at].rfind("<node").unwrap_or(0);
    let node = &screen[open..];
    let node = &node[..node.find('>').unwrap_or(node.len())];
    let Some(start) = node.find(" text=\"") else {
        return String::new();
    };
    let value = &node[start + 7..];
    value[..value.find('"').unwrap_or(value.len())].to_owned()
}

/// The prompt for a plugin's permission is the application's, naming it;
/// the user denying it fails the plugin that needs it at load, rutis seeing
/// the error with the permission named, and a call needing it fails with
/// `permission-denied` at the caller (a Python row) with the permission in
/// its data. Granted, the plugins load and the calls go through.
/// guarantee: loader/runtime-connection-instances
#[tokio::test(flavor = "multi_thread")]
async fn a_denied_permission_fails_the_plugin_and_reaches_the_caller() {
    let _environment = environment();
    // Installed afresh: nothing granted, nothing denied before.
    let _ = adb().args(["uninstall", PACKAGE]).output();
    install(&["android-sample-release.apk"]).await;
    let rutis = Rutis::with(
        json!([
            { "id": "contacts", "name": "phone:contacts" },
            { "id": "notify", "name": "phone:notify" },
            { "id": "reader", "name": "py:reader" },
            { "id": "caller", "name": "py:caller" }
        ]),
        &[("reader.py", READER), ("caller.py", CALLER)],
    )
    .await;
    let sdk: u32 = run(&["shell", "getprop", "ro.build.version.sdk"])
        .trim()
        .parse()
        .unwrap();
    // READ_CONTACTS, and POST_NOTIFICATIONS from Android 13.
    let prompts = if sdk >= 33 { 2 } else { 1 };

    press(&rutis.address(), "start");
    let deny = "com.android.permissioncontroller:id/permission_deny_button";
    let message = "com.android.permissioncontroller:id/permission_message";
    let mut denied = Vec::new();
    while denied.len() < prompts {
        let (screen, at) = DEVICE
            .eventually(
                || {
                    let screen = screen();
                    find(&screen, deny).map(|at| (screen, at))
                },
                "the application's permission prompt",
            )
            .await;
        let asked = text(&screen, message);
        assert!(
            asked.contains("rutis sample"),
            "the prompt names the application: {asked}"
        );
        if denied.last() == Some(&asked) {
            continue;
        }
        eprintln!("denying: {asked}");
        run(&[
            "shell",
            "input",
            "tap",
            &at.0.to_string(),
            &at.1.to_string(),
        ]);
        denied.push(asked);
    }

    // `contacts` failed to load, saying which permission; `notify` loaded,
    // and the caller got the denial with its code and permission.
    let failed = |id: &str| {
        rutis.host.loader.entries().into_iter().find_map(|entry| {
            match (&entry.status, entry.instance.is_some() && entry.id == id) {
                (rutis_loader::EntryStatus::Running(snapshot), true)
                    if snapshot.state == rutis::FiberState::Failed =>
                {
                    snapshot.error.as_ref().map(|error| error.to_string())
                }
                _ => None,
            }
        })
    };
    let why = DEVICE
        .eventually_with(|| failed("contacts"), "contacts failing", || rutis.rows())
        .await;
    assert!(
        why.contains("android.permission.READ_CONTACTS is not granted to this application"),
        "{why}"
    );
    DEVICE
        .eventually_with(
            || (rutis.active("notify") == 1 && rutis.active("caller") == 1).then_some(()),
            "notify and its caller",
            || rutis.rows(),
        )
        .await;
    assert_eq!(rutis.active("reader"), 0);
    let outcome = |want: &'static str| {
        let host = &rutis.host;
        move || {
            let caller = host.service("caller")?;
            let said = caller
                .invoke("outcome", json!([]).into())
                .ok()?
                .json()
                .ok()?;
            (said == json!(want)).then_some(())
        }
    };
    if sdk >= 33 {
        DEVICE
            .eventually(
                outcome("permission-denied android.permission.POST_NOTIFICATIONS"),
                "the caller told why",
            )
            .await;
    }

    // Granted (as in the application's settings): connected again, every
    // plugin loads, and the calls go through.
    grant(&["android.permission.READ_CONTACTS"]);
    if sdk >= 33 {
        grant(&["android.permission.POST_NOTIFICATIONS"]);
    }
    press(&rutis.address(), "stop");
    rutis.reach_rows(&[("contacts", 0), ("notify", 0)]).await;
    press(&rutis.address(), "start");
    rutis
        .reach_rows(&[("contacts", 1), ("notify", 1), ("reader", 1), ("caller", 1)])
        .await;
    DEVICE
        .eventually(outcome("posted"), "the caller posted")
        .await;
    press(&rutis.address(), "stop");
    let _ = adb().args(["shell", "am", "force-stop", PACKAGE]).status();
    rutis.stop().await;
}
