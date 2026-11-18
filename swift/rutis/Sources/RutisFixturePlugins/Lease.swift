// The plugins of the two session-end suites (crates/rutis-host/tests/lease_e2e.rs,
// crates/rutis-bridge/tests/host_gone.rs): `swift_lease` is served on a
// listening runtime whose controller takes over the lease, and `host_gone`
// keeps work going that would keep a process alive while its host goes
// away. Each writes a log or a marker the tests read; the gates they wait
// for are files the tests write.
import Foundation
import Rutis

// ── The lease (`swift_lease`) ───────────────────────────────────────

/// What a lease is told: who it serves, where its log is, and the two
/// gates — the cleanup waits for `stop`, a held call for `hold`.
public struct SwiftLeaseConfig: Decodable {
    public var who: String
    public var log: String
    public var stop: String
    public var hold: String
}

/// One line, appended to the log: the test reads their order.
private func append(_ line: String, to path: String) {
    let data = Data((line + "\n").utf8)
    let url = URL(fileURLWithPath: path)
    guard FileManager.default.fileExists(atPath: path) else {
        try? data.write(to: url)
        return
    }
    guard let handle = try? FileHandle(forWritingTo: url) else { return }
    defer { try? handle.close() }
    _ = try? handle.seekToEnd()
    try? handle.write(contentsOf: data)
}

/// Until the test opens the gate `path`. A cancelled call stops waiting
/// instead: the runtime is unloading what it was called for.
private func waitFor(_ path: String) async throws {
    while !FileManager.default.fileExists(atPath: path) {
        try await Task.sleep(for: .milliseconds(10))
    }
}

/// The `log` service a lease provides.
@RutisService
public final class SwiftLeaseLogger: @unchecked Sendable {
    let config: SwiftLeaseConfig
    init(_ config: SwiftLeaseConfig) { self.config = config }

    /// Who this lease serves.
    public func who() -> String { config.who }

    /// An instruction of this lease that waits for the test's gate.
    public func hold() async throws -> String {
        append("hold \(config.who)", to: config.log)
        try await waitFor(config.hold)
        return "held \(config.who)"
    }
}

public let swiftLease = Rutis.define(Plugin(
    name: "swift_lease",
    provides: ["log": .of(SwiftLeaseLogger.self, sync: ["who"])],
    config: SwiftLeaseConfig.self,
    apply: { ctx, config in
        append("start \(config.who)", to: config.log)
        ctx.provide("log", SwiftLeaseLogger(config))
        ctx.effect {
            append("cleanup \(config.who)", to: config.log)
            try await waitFor(config.stop)
            append("stop \(config.who)", to: config.log)
        }
    }))

// ── A host that went away (`host_gone`) ─────────────────────────────

/// What the plugin is told: where its cleanup writes, and whether it then
/// never finishes.
public struct HostGoneConfig: Decodable {
    public var marker: String
    public var hang: Bool
}

/// Never returns, whatever happens to the Task that runs it: an unsafe
/// continuation, which is what never resuming one is for.
private func neverFinishes() async {
    await withUnsafeContinuation { (_: UnsafeContinuation<Void, Never>) in }
}

public let hostGone = Rutis.define(Plugin(
    name: "host_gone",
    config: HostGoneConfig.self,
    apply: { ctx, config in
        // Work nothing stops, like a timer nobody clears.
        let work = Thread {
            while true { Thread.sleep(forTimeInterval: 3600) }
        }
        work.start()
        ctx.effect {
            try Data("cleaned up".utf8).write(to: URL(fileURLWithPath: config.marker))
            if config.hang { await neverFinishes() }
        }
    }))
