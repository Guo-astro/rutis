import Foundation

// The dispatch of unified events (docs/design-events-protocol-2026-10-18.md
// §5): the five modes over a snapshot of entries, wherever each entry runs.
// The runtime dispatches its fast path with it (every entry a listener of
// its own process); RutisTest dispatches as a host with it.

extension Rutis {
    /// One loader instance of a scope chain (a group copy, such as a tenant
    /// or a session), outermost first.
    public struct Scope: Codable, Hashable, Sendable {
        public var id: String
        public var group: String

        public init(id: String, group: String) {
            self.id = id
            self.group = group
        }
    }

    /// What the bus says of a listener: what a filter sees. Absent values
    /// are nil.
    public struct Listener: Codable, Hashable, Sendable {
        public var name: String
        /// Whether it is a Rust prefix listener.
        public var prefix: Bool
        /// The plugin that registered it.
        public var plugin: String
        /// Its row's id in the configuration; nil for a plugin a mount loaded.
        public var row: String?
        /// The runtime it runs in; nil for the host's own Rust plugins.
        public var runtime: String?
        /// The far node's id, for a listener a node link brought in.
        public var node: String?
        /// Its scope chain, outermost first; the root is empty.
        public var scope: [Scope]
        /// Whether it was registered with `global`.
        public var global: Bool

        public init(name: String, prefix: Bool = false, plugin: String, row: String?, runtime: String?, node: String? = nil,
                    scope: [Scope], global: Bool)
        {
            self.name = name
            self.prefix = prefix
            self.plugin = plugin
            self.row = row
            self.runtime = runtime
            self.node = node
            self.scope = scope
            self.global = global
        }

        enum CodingKeys: String, CodingKey { case name, prefix, plugin, row, runtime, node, scope, global }

        public init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            name = try c.decode(String.self, forKey: .name)
            prefix = try c.decodeIfPresent(Bool.self, forKey: .prefix) ?? false
            plugin = try c.decodeIfPresent(String.self, forKey: .plugin) ?? ""
            row = try c.decodeIfPresent(String.self, forKey: .row)
            runtime = try c.decodeIfPresent(String.self, forKey: .runtime)
            node = try c.decodeIfPresent(String.self, forKey: .node)
            scope = try c.decodeIfPresent([Scope].self, forKey: .scope) ?? []
            global = try c.decodeIfPresent(Bool.self, forKey: .global) ?? false
        }

        public func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(name, forKey: .name)
            try c.encode(prefix, forKey: .prefix)
            try c.encode(plugin, forKey: .plugin)
            // Absent values are the protocol's null.
            if let row { try c.encode(row, forKey: .row) } else { try c.encodeNil(forKey: .row) }
            if let runtime { try c.encode(runtime, forKey: .runtime) } else { try c.encodeNil(forKey: .runtime) }
            if let node { try c.encode(node, forKey: .node) } else { try c.encodeNil(forKey: .node) }
            try c.encode(scope, forKey: .scope)
            try c.encode(global, forKey: .global)
        }
    }

    /// The rest of a waterfall: a waterfall listener gets it as its last
    /// parameter, and calls it at most once. Its result is the rest of the
    /// chain's, nil when it gives none.
    public struct Next<R>: NextConstructible, @unchecked Sendable {
        let run: () async throws -> Value

        public func callAsFunction() async throws -> R? {
            let value = try await run()
            if value.isNullish { return nil }
            return try ValueCodec.decode(R.self, from: value)
        }

        static func make(from value: Value) -> Any? {
            switch value {
            case let .function(function): return Next<R>(run: { try await function.body([]) })
            case let .remoteFunction(far): return Next<R>(run: { try await settle(try await far.call([], sync: false)) })
            default: return nil
            }
        }
    }
}

/// A parameter type built from a `next` function (Rutis.Next).
protocol NextConstructible {
    static func make(from value: Value) -> Any?
}

/// How a dispatch runs its listeners.
package enum Mode: String, Sendable {
    case emit, parallel, serial, waterfall, bail
    case waterfallSync = "waterfall-sync"

    var isWaterfall: Bool { self == .waterfall || self == .waterfallSync }
}

/// The rest of a waterfall, as the dispatch runs it.
package typealias NextFunction = () async throws -> Value

/// Runs one reserved invocation of an entry: with the event's arguments
/// and, in a waterfall, `next`.
package typealias EventCall = (_ mode: Mode, _ args: [Value], _ next: NextFunction?) async throws -> Value

/// One listener of a dispatch's snapshot.
package protocol EventEntry: AnyObject {
    var info: Rutis.Listener { get }
    /// Reserves an invocation, unless the entry was withdrawn: then it is
    /// skipped, as if it had never been in the snapshot.
    func begin() -> EventCall?
    /// Starts an `emit` invocation after the earlier ones of this entry
    /// (§5.5), unless withdrawn. Its failure is reported by the entry.
    @discardableResult
    func emit(_ args: [Value]) -> Bool
}

/// The events' rules shared by the runtime and RutisTest.
package enum Events {
    /// Refuses what no plugin may emit: `internal/*` (the runtime's own)
    /// and `rutis/*` (rutis's own, ReservedEvent).
    package static func checkEmit(_ name: String) throws {
        if name.hasPrefix("internal/") { throw RutisError(name: "Error", message: "event \(name) is internal to the runtime") }
        if name.hasPrefix("rutis/") {
            throw RutisError(name: RutisError.Name.reservedEvent, message: "rutis/* events are emitted by rutis only: \(name)")
        }
    }

    /// Refuses what no leaf plugin may listen to: `internal/*`.
    package static func checkListen(_ name: String) throws {
        if name.hasPrefix("internal/") { throw RutisError(name: "Error", message: "event \(name) is internal to the runtime") }
    }

    /// Whether `value` is a result: anything but undefined, null and false.
    package static func hasResult(_ value: Value) -> Bool {
        switch value {
        case .undefined, .data(.null), .data(.bool(false)), .signal: return false
        default: return true
        }
    }

    /// How long an unload waits for its row's running listeners and
    /// cleanups may take once the session ended: RUTIS_SHUTDOWN_TIMEOUT
    /// seconds (rutis-host passes its own shutdown deadline on), default 10.
    package static var shutdownTimeout: Double {
        let text = ProcessInfo.processInfo.environment["RUTIS_SHUTDOWN_TIMEOUT"]?.trimmingCharacters(in: .whitespaces) ?? ""
        guard let seconds = Double(text), seconds > 0, seconds.isFinite else { return 10 }
        return seconds
    }

    /// Where a runtime's lines go and how long its cleanups may take once
    /// its session ends: a binary's are stderr and RUTIS_SHUTDOWN_TIMEOUT,
    /// an app's are RutisApp's `log` and `shutdownTimeout`.
    package struct Settings: Sendable {
        package var log: @Sendable (String) -> Void
        package var shutdownTimeout: Double

        package init(log: @escaping @Sendable (String) -> Void = Events.standardError,
                     shutdownTimeout: Double = Events.shutdownTimeout)
        {
            self.log = log
            self.shutdownTimeout = shutdownTimeout
        }
    }

    /// The seconds as the report lines write them (`10`, `0.5`).
    package static func seconds(_ value: Double) -> String {
        value == value.rounded() && abs(value) < 1e15 ? String(Int64(value)) : String(value)
    }

    private static let reportLock = Lock()
    nonisolated(unsafe) private static var reporters: [Int: (String) -> Void] = [:]
    nonisolated(unsafe) private static var nextReporter = 0

    /// Writes one line of §5.6 to `log` (stderr unless an app gave its
    /// own), and to every hook.
    package static func report(_ line: String, to log: (String) -> Void = standardError) {
        let hooks = reportLock.withLock { Array(reporters.values) }
        log(line)
        for hook in hooks { hook(line) }
    }

    /// Writes `line` on stderr: where a binary's lines go.
    @Sendable package static func standardError(_ line: String) {
        FileHandle.standardError.write(Data((line + "\n").utf8))
    }

    /// Hears every line reported until the returned function is called.
    package static func hear(_ hook: @escaping (String) -> Void) -> () -> Void {
        let id = reportLock.withLock { () -> Int in
            nextReporter += 1
            reporters[nextReporter] = hook
            return nextReporter
        }
        return { reportLock.withLock { reporters[id] = nil } }
    }

    /// An error as the lines of §5.6 show it: `<ErrorName>: <message>`.
    package static func failure(_ error: Error) -> String {
        if error is CancellationError { return "CancellationError: cancelled" }
        return "\(errorName(error)): \(errorMessage(error))"
    }
}

/// One dispatch: the snapshot, in registration order, already limited to
/// the emitter's scope (§5.1 step 1).
package struct Dispatch {
    package var name: String
    package var entries: [EventEntry]
    /// The filter (§6), nil for none: asked once per non-global entry, in
    /// order, before any invocation starts.
    package var keep: ((Rutis.Listener) async throws -> Bool)?

    package init(name: String, entries: [EventEntry], keep: ((Rutis.Listener) async throws -> Bool)? = nil) {
        self.name = name
        self.entries = entries
        self.keep = keep
    }

    private func selected() async throws -> [EventEntry] {
        guard let keep else { return entries }
        var kept: [EventEntry] = []
        for entry in entries {
            let info = entry.info
            if !info.global, try await !keep(info) { continue }
            kept.append(entry)
        }
        return kept
    }

    /// Starts every invocation in order and waits for none. A filter's
    /// failure is thrown, for the dispatcher to report (§5.6).
    /// `unlessCancelled`: the dispatching task cancelled once the filter
    /// decided, nothing starts.
    package func emit(_ args: [Value], unlessCancelled: Bool = false) async throws {
        let entries = try await selected()
        if unlessCancelled, Task.isCancelled { return }
        for entry in entries { entry.emit(args) }
    }

    /// Starts every invocation in order and waits for all; failures are an
    /// AggregateError in registration order, even a single one. The emitter
    /// giving up (its Task cancelled) cancels those running.
    package func parallel(_ args: [Value]) async throws {
        let entries = try await selected()
        var calls: [EventCall] = []
        for entry in entries {
            if Task.isCancelled { break }
            if let call = entry.begin() { calls.append(call) }
        }
        let failures = await withTaskGroup(of: (Int, Error?).self) { group -> [(Int, Error)] in
            for (index, call) in calls.enumerated() {
                group.addTask {
                    do {
                        _ = try await call(.parallel, args, nil)
                        return (index, nil)
                    } catch {
                        return (index, error)
                    }
                }
            }
            var failures: [(Int, Error)] = []
            for await (index, error) in group {
                if let error { failures.append((index, error)) }
            }
            return failures.sorted { $0.0 < $1.0 }
        }
        try Task.checkCancellation()
        if !failures.isEmpty {
            throw RutisError(name: RutisError.Name.aggregateError, message: "\(failures.count) listeners of \(name) failed",
                             errors: failures.map { asRutisError($0.1) })
        }
    }

    /// Runs one invocation at a time and returns the first result; a
    /// failure stops it. `mode` is serial or bail.
    package func serial(_ mode: Mode, _ args: [Value]) async throws -> Value {
        for entry in try await selected() {
            try Task.checkCancellation()
            guard let call = entry.begin() else { continue }
            let value = try await call(mode, args, nil)
            if Events.hasResult(value) { return value }
        }
        return .undefined
    }

    /// Calls the outermost entry with `next`, each `next` calling the entry
    /// after it and the last one `terminal` with the event's arguments (nil:
    /// no result), and returns the outermost value as it is.
    package func waterfall(_ mode: Mode, _ args: [Value], terminal: (([Value]) async throws -> Value)?) async throws -> Value {
        let entries = try await selected()
        let ended = Flag()
        defer { ended.set() }
        let name = self.name
        func run(_ from: Int) async throws -> Value {
            for index in from ..< entries.count {
                guard let call = entries[index].begin() else { continue }
                let called = Flag()
                let next: NextFunction = {
                    if called.testAndSet() {
                        throw RutisError(name: RutisError.Name.waterfallNext, message: "next of \(name) was already called")
                    }
                    if ended.isSet || Task.isCancelled {
                        throw RutisError(name: RutisError.Name.waterfallNext, message: "next of \(name) was called after its waterfall ended")
                    }
                    return try await run(index + 1)
                }
                return try await call(mode, args, next)
            }
            if let terminal { return try await terminal(args) }
            return .undefined
        }
        return try await run(0)
    }
}

/// An error as a RutisError: its name and message (and the graph of one
/// received).
func asRutisError(_ error: Error) -> RutisError {
    if let error = error as? RutisError { return error }
    let (code, data, _) = codeAndData(error)
    return RutisError(name: errorName(error), message: errorMessage(error), code: code, data: data)
}

/// A flag set once.
final class Flag: @unchecked Sendable {
    private let lock = Lock()
    private var value = false

    var isSet: Bool { lock.withLock { value } }
    func set() { lock.withLock { value = true } }

    /// Sets it; whether it already was.
    func testAndSet() -> Bool {
        lock.withLock {
            defer { value = true }
            return value
        }
    }
}

/// Runs functions one after another, in the order given (§5.5: the `emit`
/// invocations of one listener).
package final class SerialQueue: @unchecked Sendable {
    private let lock = Lock()
    private var pending: [() async -> Void] = []
    private var running = false

    package init() {}

    package func push(_ work: @escaping () async -> Void) {
        lock.lock()
        pending.append(work)
        if running {
            lock.unlock()
            return
        }
        running = true
        lock.unlock()
        Task.detached { [self] in
            while true {
                lock.lock()
                if pending.isEmpty {
                    running = false
                    lock.unlock()
                    return
                }
                let work = pending.removeFirst()
                lock.unlock()
                await work()
            }
        }
    }
}
