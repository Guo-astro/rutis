//! A Swift plugin that reads the user's calendars (EventKit), run by
//! `rutis-host run` and called from a Python plugin (Swift S2, #387; design
//! `docs/design-swift-sdk-2026-10-12.md` §八). macOS only.
//!
//! What macOS decides about access to protected data (TCC) belongs to the
//! plugin binary, not to the host or the terminal that started it: the
//! runtime is responsible for itself (the Swift SDK disclaims its parent's
//! responsibility when it starts), and macOS knows it by the identifier
//! of its Info.plist, which also carries the usage description the prompt
//! shows. A refusal reaches the caller, in another language, as the
//! plugin's error.
//!
//! A prompt cannot be answered on a CI runner: the plugin never asks here
//! (`ask: false`), and reports where it stands. Where this binary's access
//! is not granted (not decided, or denied: what a CI runner has), the call
//! fails with the plugin's error; where it is (granted by hand), it
//! answers. With `RUTIS_E2E_TCC_ASK=1` it asks: the manual check of the
//! prompt (see the pull request of #387).

use std::path::Path;

use rutis_e2e::{build_guard, repo, Lang, Scenario};
use serde_json::json;

const PACKAGE: &str = r#"// swift-tools-version:6.0
import Foundation
import PackageDescription

let infoPlist = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("Info.plist").path

let package = Package(
    name: "calendar",
    platforms: [.macOS(.v14)],
    dependencies: [.package(path: "../rutis")],
    targets: [
        .executableTarget(
            name: "calendar",
            dependencies: [.product(name: "Rutis", package: "rutis")],
            linkerSettings: [
                .unsafeFlags(["-Xlinker", "-sectcreate", "-Xlinker", "__TEXT", "-Xlinker", "__info_plist", "-Xlinker", infoPlist]),
            ]
        ),
    ],
    swiftLanguageModes: [.v5]
)
"#;

const INFO_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>dev.rutis.e2e.calendar</string>
    <key>CFBundleName</key>
    <string>rutis e2e calendar</string>
    <key>NSCalendarsFullAccessUsageDescription</key>
    <string>The rutis end-to-end test reads the titles of this week's events.</string>
</dict>
</plist>
"#;

const PLUGIN: &str = r#"import EventKit
import Foundation
import Rutis

struct Config: Decodable {
    /// Whether to ask the user when access is not decided yet.
    var ask: Bool?
}

struct CalendarAccessDenied: Error, LocalizedError {
    let code = "denied"
    let status: String
    var errorDescription: String? { "calendar access is not granted to this plugin (\(status))" }
}

func describe(_ status: EKAuthorizationStatus) -> String {
    switch status {
    case .notDetermined: return "not determined"
    case .restricted: return "restricted"
    case .denied: return "denied"
    case .fullAccess: return "full access"
    case .writeOnly: return "write only"
    @unknown default: return "unknown"
    }
}

@RutisService
final class Calendar: @unchecked Sendable {
    let store = EKEventStore()
    let ask: Bool
    init(ask: Bool) { self.ask = ask }

    /// This process, and the one macOS holds responsible for its access.
    func identity() -> [Int] {
        typealias ResponsibleFor = @convention(c) (pid_t) -> pid_t
        guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "responsibility_get_pid_responsible_for_pid") else {
            return [Int(getpid()), -1]
        }
        return [Int(getpid()), Int(unsafeBitCast(symbol, to: ResponsibleFor.self)(getpid()))]
    }

    func status() -> String { describe(EKEventStore.authorizationStatus(for: .event)) }

    /// The titles of the next `days` days' events.
    func titles(_ days: Int) async throws -> [String] {
        let status = EKEventStore.authorizationStatus(for: .event)
        if status != .fullAccess {
            guard ask, status == .notDetermined, try await store.requestFullAccessToEvents() else {
                throw CalendarAccessDenied(status: describe(EKEventStore.authorizationStatus(for: .event)))
            }
        }
        let start = Date()
        let end = start.addingTimeInterval(Double(days) * 86_400)
        return store.events(matching: store.predicateForEvents(withStart: start, end: end, calendars: nil)).map { $0.title ?? "" }
    }
}

let calendar = Rutis.define(Plugin(
    name: "calendar",
    provides: ["calendar": .of(Calendar.self, sync: ["identity", "status"])],
    config: Config.self,
    apply: { ctx, config in
        ctx.provide("calendar", Calendar(ask: config.ask ?? false))
    }))

Rutis.serve(calendar)
"#;

/// guarantee: swift/tcc-belongs-to-the-binary
#[test]
#[cfg_attr(
    not(target_os = "macos"),
    ignore = "TCC and Swift plugin binaries are macOS only"
)]
fn calendar_access_belongs_to_the_plugin_binary_and_a_refusal_reaches_the_caller() {
    let s = Scenario::new("tcc-calendar");
    // A path dependency's package is named after its directory.
    s.link("rutis", &repo());
    s.write("calendar/Package.swift", PACKAGE);
    s.write("calendar/Info.plist", INFO_PLIST);
    s.write("calendar/Sources/calendar/main.swift", PLUGIN);
    let mut build = s.program("calendar", "swift", ["build", "--product", "calendar"], &[]);
    assert!(
        build.wait_exit_within(build_guard()).success(),
        "swift build"
    );
    let output = std::process::Command::new("swift")
        .args(["build", "--show-bin-path"])
        .current_dir(s.path("calendar"))
        .output()
        .expect("swift is on PATH");
    let built = Path::new(String::from_utf8_lossy(&output.stdout).trim()).join("calendar");
    let binary = s.path("plugins/calendar");
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::copy(&built, &binary).unwrap();

    // macOS knows the binary by the identifier of its Info.plist, which
    // carries the usage description its prompt shows.
    let signature = std::process::Command::new("codesign")
        .args(["-d", "-v"])
        .arg(&binary)
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&signature.stderr);
    assert!(said.contains("Identifier=dev.rutis.e2e.calendar"), "{said}");
    let contents = std::fs::read(&binary).unwrap();
    assert!(
        contents
            .windows(b"NSCalendarsFullAccessUsageDescription".len())
            .any(|window| window == b"NSCalendarsFullAccessUsageDescription"),
        "the usage description is in the binary"
    );

    let ask = std::env::var("RUTIS_E2E_TCC_ASK").is_ok_and(|value| value == "1");
    let probe = s.probe("probe", Lang::Python, &["calendar"]);
    s.write_json(
        "rutis.json",
        &json!({
            "id": "e2e",
            "runtimes": { "swift": { "dir": "plugins" }, "py": { "project": "." } },
            "rows": [
                { "id": "calendar", "name": "swift:calendar", "config": { "ask": ask } },
                probe.row(),
            ],
        }),
    );
    let mut host = s.host(["run", "rutis.json"]);
    host.expect("calendar: running");
    probe.started(&mut host);

    // The runtime answers for itself: not the host, not the terminal.
    let identity = probe
        .call(&mut host, "calendar", "identity", json!([]))
        .expect("calendar.identity");
    assert_eq!(identity[0], identity[1], "pid, responsible pid: {identity}");

    // Where it stands once asked (or not).
    let titles = probe.call(&mut host, "calendar", "titles", json!([7]));
    let status = probe
        .call(&mut host, "calendar", "status", json!([]))
        .expect("calendar.status");
    match status.as_str().unwrap() {
        // Granted to this binary by hand (or just now, asked): it answers.
        "full access" => assert!(
            titles.as_ref().is_ok_and(|titles| titles.is_array()),
            "{titles:?}"
        ),
        // Not granted (a CI runner): the refusal reaches the Python caller
        // as the Swift plugin's error, its name and message.
        other => assert_eq!(
            titles,
            Err(format!(
                "CalendarAccessDenied: calendar access is not granted to this plugin ({other})"
            ))
        ),
    }

    host.ctrl_c();
    assert_eq!(host.wait_exit().code(), Some(0));
    host.wait_output_end();
    s.finish();
}
