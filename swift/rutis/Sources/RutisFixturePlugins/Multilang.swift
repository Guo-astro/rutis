// The Swift plugins of the Rust suites that run several runtimes
// (crates/rutis-loader/tests/multilang_swift.rs, swift_rows.rs,
// caller_rows.rs; crates/rutis-bridge/tests/swift_runtime.rs): the same
// plugins as the Go ones, using and providing services across runtimes.
import Foundation
import Rutis

@RutisClient
public protocol Probe {
    func record(_ line: String) async throws
}

/// The `swift_weather` service.
@RutisService
public final class SwiftWeather: @unchecked Sendable {
    init() {}
    public func today() -> String { "swift sunny" }
    public func later() async -> String { "swift later" }

    /// Calls back into the caller during a synchronous call.
    public func each(_ days: [String], _ callback: @escaping (String) async throws -> String) async throws -> [String] {
        var out: [String] = []
        for day in days { out.append(try await callback(day)) }
        return out
    }

    /// Fails with an error that has a code and data (#440).
    public func fail() throws { throw Stormy() }
}

/// An error with a code and data: plain stored properties.
struct Stormy: Error, LocalizedError {
    let code = "Stormy"
    let data: [String: UInt64] = ["wind": 1 << 60]
    var errorDescription: String? { "too windy" }
}

@RutisClient
public protocol RemoteWeather {
    func today() async throws -> String
    func later() async throws -> String
}

public let swiftProvider = Rutis.define(Plugin(
    name: "swift_provider",
    inject: ["probe"],
    provides: ["swift_weather": .of(SwiftWeather.self, sync: ["today", "each"])],
    apply: { ctx in
        let probe = try ctx.use("probe", ProbeProxy.self)
        ctx.provide("swift_weather", SwiftWeather())
        ctx.effect { try await probe.record("swift provider: bye") }
        try await probe.record("swift provider: start")
    }))

public let swiftConsumer = Rutis.define(Plugin(
    name: "swift_consumer",
    inject: ["js_weather", "py_weather", "probe"],
    apply: { ctx in
        let probe = try ctx.use("probe", ProbeProxy.self)
        var lines: [String] = []
        for name in ["js_weather", "py_weather"] {
            let weather = try ctx.use(name, RemoteWeatherProxy.self)
            lines.append("\(try await weather.today()) / \(try await weather.later())")
        }
        ctx.effect { try await probe.record("swift consumer: bye") }
        try await probe.record("swift consumer: " + lines.joined(separator: " / "))
    }))

/// The weather service as a user in this process sees it.
@RutisClient
public protocol LocalWeather {
    func today() async throws -> String
}

extension SwiftWeather: LocalWeather {}

public let swiftLocal = Rutis.define(Plugin(
    name: "swift_local",
    inject: ["swift_weather", "probe"],
    apply: { ctx in
        let probe = try ctx.use("probe", ProbeProxy.self)
        let weather = try ctx.use("swift_weather", LocalWeatherProxy.self)
        ctx.effect { try await probe.record("swift local: bye") }
        try await probe.record("swift local: " + (weather is SwiftWeather ? "native" : "proxy"))
    }))

@RutisClient
public protocol Llm {
    func ask() async throws -> String
}

public let swiftGated = Rutis.define(Plugin(
    name: "swift_gated",
    inject: ["llm", "probe"],
    apply: { ctx in
        let probe = try ctx.use("probe", ProbeProxy.self)
        let llm = try ctx.use("llm", LlmProxy.self)
        let answer = try await llm.ask()
        ctx.effect { try await probe.record("swift gated: bye") }
        try await probe.record("swift gated: " + answer)
    }))

public struct RegistrantConfig: Decodable {
    var name: String
}

@RutisClient
public protocol Ledger {
    func register(_ name: String) async throws
}

/// Registers its config's name with the host's `ledger`, which sees the row
/// that calls it (crates/rutis-loader/tests/caller_rows.rs).
public let swiftRegistrant = Rutis.define(Plugin(
    name: "swift_registrant",
    inject: ["ledger"],
    config: RegistrantConfig.self,
    apply: { ctx, config in
        try await ctx.use("ledger", LedgerProxy.self).register(config.name)
    }))

// ── ping and caller: plugins in two binaries ─────────────────────────────

/// The `ping` service, answering as the binary it runs in (its file name).
@RutisService
public final class Ping: @unchecked Sendable {
    let from: String
    init(from: String) { self.from = from }
    public func echo(_ text: String) -> String { "\(from) pong \(text)" }
    public func later(_ text: String) async -> String { "\(from) later \(text)" }
    public func crash() { exit(17) }
}

public let pingPlugin = Rutis.define(Plugin(
    name: "ping",
    provides: ["ping": .of(Ping.self, sync: ["echo", "crash"])],
    apply: { ctx in
        let binary = URL(fileURLWithPath: CommandLine.arguments[0]).lastPathComponent
        ctx.provide("ping", Ping(from: binary))
    }))

@RutisClient
public protocol PingClient {
    func echo(_ text: String) async throws -> String
    func later(_ text: String) async throws -> String
}

@RutisService
public final class Caller: @unchecked Sendable {
    let ping: any PingClient
    init(ping: any PingClient) { self.ping = ping }
    public func relay(_ text: String) async throws -> String { try await ping.echo(text) }
    public func relayLater(_ text: String) async throws -> String { try await ping.later(text) }
}

/// Uses `ping` (from another binary) and provides `caller`.
public let callerPlugin = Rutis.define(Plugin(
    name: "caller",
    inject: ["ping"],
    provides: ["caller": .of(Caller.self, sync: ["relay"])],
    apply: { ctx in
        ctx.provide("caller", Caller(ping: try ctx.use("ping", PingClientProxy.self)))
    }))

// ── probe: what crosses the session ──────────────────────────────────────

/// The `work` service: cancellation, errors, objects by reference.
@RutisService
public final class Work: @unchecked Sendable {
    private let lock = NSLock()
    private var started = false
    private var aborted = false
    let shared = Counter()

    init() {}

    /// Waits until its caller cancels (the call's Task).
    public func wait() async {
        lock.withLock { started = true }
        while !Task.isCancelled { try? await Task.sleep(nanoseconds: 5_000_000) }
        lock.withLock { aborted = true }
    }

    public func isStarted() -> Bool { lock.withLock { started } }
    public func isAborted() -> Bool { lock.withLock { aborted } }
    public func ping() -> String { "pong" }

    /// Fails with a code and data.
    public func fail() throws { throw Quota() }

    /// Throws again an error it received, with its whole graph.
    public func rethrow(_ callback: @escaping () async throws -> String) async throws -> String { try await callback() }

    /// The same counter, by reference.
    public func counter() -> Counter { shared }

    public func record() -> Point { Point(x: 1, y: 2) }
    public func sum(_ values: [Int]) -> Int { values.reduce(0, +) }
}

public struct Point: Codable {
    var x: Int
    var y: Int
}

struct Quota: Error, LocalizedError {
    let code = "QuotaExceeded"
    let data: [String: Int] = ["left": 0]
    var errorDescription: String? { "quota exhausted" }
}

/// An object that crosses by reference.
@RutisService
public final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    public private(set) var total = 0
    init() {}
    public func add(_ n: Int) -> Int { lock.withLock { total += n; return total } }
}

public let probePlugin = Rutis.define(Plugin(
    name: "probe",
    provides: ["work": .of(Work.self, sync: ["isStarted", "isAborted", "ping", "fail", "rethrow", "counter", "record", "sum"])],
    apply: { ctx in ctx.provide("work", Work()) }))

// ── swift_relay: the wait-cycle suite (crates/rutis-loader/tests/sync_cycles.rs)

@RutisClient
public protocol BWork {
    func work() async throws -> String
}

/// The `s_relay` service: calls `b_work` synchronously, on the caller's
/// chain (the Task's).
@RutisService
public final class SwiftRelay: @unchecked Sendable {
    let other: any BWork
    init(other: any BWork) { self.other = other }

    public func work() async throws -> String {
        do {
            return "s_relay relays " + (try await other.work())
        } catch let error as RutisError where error.name == RutisError.Name.syncWaitCycle {
            return "s_relay: SyncWaitCycle"
        }
    }
}

public let swiftRelay = Rutis.define(Plugin(
    name: "swift_relay",
    inject: ["b_work"],
    provides: ["s_relay": .of(SwiftRelay.self, sync: ["work"])],
    apply: { ctx in ctx.provide("s_relay", SwiftRelay(other: try ctx.use("b_work", BWorkProxy.self))) }))
