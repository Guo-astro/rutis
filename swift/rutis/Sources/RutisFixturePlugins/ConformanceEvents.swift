// The plugin conformance-events (#464, docs/design-events-protocol-2026-10-18.md
// §13.1): each row is one participant of an event expectation, a listener
// or an emitter, and logs what it does through the host's synchronous
// service events-log.
import Foundation
import Rutis

/// A JSON value of the participant's configuration.
public enum JSONValue: Codable, Sendable {
    case null
    case bool(Bool)
    case int(Int)
    case double(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() {
            self = .null
        } else if let b = try? c.decode(Bool.self) {
            self = .bool(b)
        } else if let i = try? c.decode(Int.self) {
            self = .int(i)
        } else if let d = try? c.decode(Double.self) {
            self = .double(d)
        } else if let s = try? c.decode(String.self) {
            self = .string(s)
        } else if let a = try? c.decode([JSONValue].self) {
            self = .array(a)
        } else {
            self = .object(try c.decode([String: JSONValue].self))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case let .bool(b): try c.encode(b)
        case let .int(i): try c.encode(i)
        case let .double(d): try c.encode(d)
        case let .string(s): try c.encode(s)
        case let .array(a): try c.encode(a)
        case let .object(o): try c.encode(o)
        }
    }

    var string: String? {
        if case let .string(s) = self { return s }
        return nil
    }

    subscript(key: String) -> JSONValue? {
        if case let .object(o) = self { return o[key] }
        return nil
    }
}

/// A row's configuration: a listener, or an emitter.
public struct Participant: Decodable {
    var label: String
    var emitter: Bool?
    var name: String?
    var log: JSONValue?
    var does: JSONValue?
    var options: Options?
    /// A service of this name the row provides besides (rutis/service).
    var provides: String?
    /// The row's apply fails with this error, having registered nothing.
    var fails: Thrown?

    struct Options: Decodable {
        var prepend: Bool?
        var global: Bool?
    }

    struct Thrown: Decodable {
        var name: String
        var message: String
    }
}

@RutisClient
public protocol EventsLog {
    func add(_ entry: String) async throws
    /// Logs `filter:<row>` and keeps the description (filterSees).
    func filter(_ description: Rutis.Listener) async throws
}

@RutisClient
public protocol EventsWait {
    func wait(_ entry: String) async throws
}

/// What listeners of this process logged: an emitter of this process waits
/// for one of them (cancel.when) when the host has no events-log.wait.
final class Logged: @unchecked Sendable {
    static let shared = Logged()
    private let lock = NSLock()
    private var entries: [String] = []

    func note(_ entry: String) { lock.withLock { entries.append(entry) } }

    func reset() { lock.withLock { entries = [] } }

    func wait(for entry: String) async throws {
        while !lock.withLock({ entries.contains(entry) }) {
            try await Task.sleep(nanoseconds: 5_000_000)
        }
    }
}

final class ParticipantRow: @unchecked Sendable {
    let ctx: Rutis.Ctx
    let participant: Participant
    let log: any EventsLog
    let waiter: (any EventsWait)?

    init(ctx: Rutis.Ctx, participant: Participant, log: any EventsLog, waiter: (any EventsWait)?) {
        self.ctx = ctx
        self.participant = participant
        self.log = log
        self.waiter = waiter
    }

    /// Logs `entry` with the host. Not with the caller's cancellation: a
    /// listener cancelled at its row's unload still says it was (`b:cancelled`),
    /// as Go's fixture logs with `context.WithoutCancel`. A call from the
    /// cancelled Task itself is sent and then cancelled at once, and whether
    /// the host logged it before the cancel arrived was a race (#387). An
    /// unstructured Task keeps the call chain and drops the cancellation.
    func add(_ entry: String) async {
        Logged.shared.note(entry)
        let log = self.log
        await Task { try? await log.add(entry) }.value
    }

    /// Waits until `entry` is logged, by any participant.
    func waitFor(_ entry: String) async throws {
        if let waiter { try await waiter.wait(entry) } else { try await Logged.shared.wait(for: entry) }
    }

    /// A logged value: numbers without a fraction as integers.
    static func text(_ value: Any?) -> String {
        switch value {
        case nil: return "null"
        case let d as Double where d == d.rounded() && abs(d) < 1e15: return String(Int64(d))
        case let some?: return "\(some)"
        }
    }

    /// What a listener logs when it starts (§13.2 `log`).
    func entry(_ args: [Any?]) -> String? {
        let label = participant.label
        let first = args.first ?? nil
        switch participant.log {
        case nil, .string("label")?: return label
        case .bool(false)?: return nil
        case .string("arg")?: return "\(label):\(ParticipantRow.text(first))"
        case let .array(fields)?:
            let record = first as? [String: Any?]
            let values = fields.map { field -> String in
                guard let key = field.string, let value = record?[key] else { return "null" }
                return ParticipantRow.text(value)
            }
            return label + ":" + values.joined(separator: ",")
        default: return label
        }
    }

    func listen(_ args: [Any?], _ next: Rutis.Next<Any>?) async throws -> Any? {
        if let entry = entry(args) { await add(entry) }
        return try await does(participant.does ?? .object([:]), args, next)
    }

    func does(_ does: JSONValue, _ args: [Any?], _ next: Rutis.Next<Any>?) async throws -> Any? {
        let first = args.first ?? nil
        if let value = does["returns"] { return value }
        if does["returnsNothing"] != nil { return nil }
        if does["echo"] != nil { return first }
        if let thrown = does["throws"] {
            throw RutisError(name: thrown["name"]?.string ?? "Error", message: thrown["message"]?.string ?? "")
        }
        if let after = does["after"] {
            let ms: Int
            switch after {
            case let .int(i): ms = i
            case let .double(d): ms = Int(d)
            default: ms = 0
            }
            try await Task.sleep(nanoseconds: UInt64(ms) * 1_000_000)
            return try await self.does(does["then"] ?? .object([:]), args, next)
        }
        if let prefix = does["next"]?.string {
            guard let next else { throw RutisError(name: "Error", message: "conformance-events: \(participant.label) has no next in this dispatch") }
            let rest = try await next() ?? ""
            return prefix + ParticipantRow.text(rest)
        }
        if let value = does["veto"] { return value }
        if does["nextTwice"] != nil {
            guard let next else { throw RutisError(name: "Error", message: "conformance-events: \(participant.label) has no next in this dispatch") }
            _ = try await next()
            return try await next()
        }
        if does["waits"] != nil {
            // Until cancelled.
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 5_000_000)
            }
            await add(participant.label + ":cancelled")
            return nil
        }
        if does["calls"] != nil {
            guard let function = first as? Rutis.RemoteFunction else {
                throw RutisError(name: "Error", message: "conformance-events: \(participant.label) needs a function, got \(String(describing: first))")
            }
            return try await function.call(["x"], as: Any?.self)
        }
        if let spec = does["emits"] {
            var request = try nativeSpec(spec)
            if request["name"] == nil || request["name"]! == nil { request["name"] = participant.name }
            return try await dispatch(request)
        }
        throw RutisError(name: "Error", message: "conformance-events: unknown does")
    }

    /// A spec in a listener's configuration, as the runner's are given.
    func nativeSpec(_ spec: JSONValue) throws -> [String: Any?] {
        func native(_ value: JSONValue) -> Any? {
            switch value {
            case .null: return nil
            case let .bool(b): return b
            case let .int(i): return i
            case let .double(d): return d
            case let .string(s): return s
            case let .array(a): return a.map(native)
            case let .object(o): return o.mapValues(native)
            }
        }
        return (native(spec) as? [String: Any?]) ?? [:]
    }

    /// Emits `spec` from the row, natively, and returns the result.
    func dispatch(_ spec: [String: Any?]) async throws -> Any? {
        let mode = (spec["mode"] ?? nil) as? String ?? ""
        let name = (spec["name"] ?? nil) as? String ?? (participant.name ?? "")
        let args = ((spec["args"] ?? nil) as? [Any?]) ?? []
        let filter = (spec["filter"] ?? nil) as? [String: Any?]
        let cancel = ((spec["cancel"] ?? nil) as? [String: Any?]).flatMap { ($0["when"] ?? nil) as? String }
        var keep: ((Rutis.Listener) async throws -> Bool)?
        if let filter {
            let labels = ((filter["keep"] ?? nil) as? [Any?])?.compactMap { $0 as? String } ?? []
            let thrown = (filter["throws"] ?? nil) as? [String: Any?]
            keep = { listener in
                if let thrown {
                    throw RutisError(name: (thrown["name"] ?? nil) as? String ?? "Error", message: (thrown["message"] ?? nil) as? String ?? "")
                }
                Logged.shared.note("filter:" + (listener.row ?? ""))
                try? await self.log.filter(listener)
                return labels.contains(listener.row ?? "")
            }
        }
        var terminal: (([Any?]) async throws -> Any?)?
        switch spec["terminal"] ?? nil {
        case let text as String:
            terminal = { _ in
                await self.add("terminal")
                return text
            }
        case let echo as [String: Any?] where (echo["echo"] ?? nil) as? Bool == true:
            terminal = { args in
                await self.add("terminal")
                return args.first ?? nil
            }
        default: break
        }
        let ctx = self.ctx
        let run: () async throws -> Any? = {
            switch mode {
            case "emit":
                if let keep { try ctx.filter(keep).emit(name, arguments: args) } else { try ctx.emit(name, arguments: args) }
                return nil
            case "parallel":
                if let keep { try await ctx.filter(keep).parallel(name, arguments: args) } else { try await ctx.parallel(name, arguments: args) }
                return nil
            case "serial":
                if let keep { return try await ctx.filter(keep).serial(name, arguments: args, as: Any?.self) ?? nil }
                return try await ctx.serial(name, arguments: args, as: Any?.self) ?? nil
            case "bail":
                if let keep { return try await ctx.filter(keep).bail(name, arguments: args, as: Any?.self) ?? nil }
                return try await ctx.bail(name, arguments: args, as: Any?.self) ?? nil
            case "waterfall":
                if let keep { return try await ctx.filter(keep).waterfall(name, arguments: args, as: Any?.self, terminal: terminal) ?? nil }
                return try await ctx.waterfall(name, arguments: args, as: Any?.self, terminal: terminal) ?? nil
            default:
                throw RutisError(name: "Error", message: "conformance-events: mode \(mode) is not the Swift runtime's to emit")
            }
        }
        guard let cancel else { return try await run() }
        // The emitter gives up once that listener started.
        let task = Task { try await run() }
        let watcher = Task {
            try await self.waitFor(cancel)
            task.cancel()
        }
        defer { watcher.cancel() }
        return try await withTaskCancellationHandler {
            try await task.value
        } onCancel: {
            task.cancel()
        }
    }
}

/// The service emitter-<label>.
@RutisService
public final class EventsEmitter: @unchecked Sendable {
    let row: ParticipantRow
    init(row: ParticipantRow) { self.row = row }

    /// Emits `spec` from the row, natively, and returns the result.
    public func dispatch(_ spec: [String: Any?]) async throws -> Any? { try await row.dispatch(spec) }

    /// `dispatch` for bail.
    public func dispatchSync(_ spec: [String: Any?]) async throws -> Any? { try await row.dispatch(spec) }
}

/// The service a row provides with `provides`.
@RutisService
public final class Present: @unchecked Sendable {
    init() {}
    public func ping() -> String { "pong" }
}

/// Forgets what this process's listeners logged (a test tool running one
/// case after another).
public func resetConformanceLog() { Logged.shared.reset() }

public let conformanceEventsPlugin = Rutis.define(Plugin(
    name: "conformance-events",
    inject: ["events-log"],
    config: Participant.self,
    apply: { ctx, participant in
        if let fails = participant.fails { throw RutisError(name: fails.name, message: fails.message) }
        let log = try ctx.use("events-log", EventsLogProxy.self)
        let waiter = try? ctx.use("events-log", EventsWaitProxy.self)
        let row = ParticipantRow(ctx: ctx, participant: participant, log: log, waiter: waiter)
        if participant.emitter == true {
            ctx.provide("emitter-" + participant.label, EventsEmitter(row: row))
        } else {
            ctx.on(participant.name ?? "", prepend: participant.options?.prepend ?? false, global: participant.options?.global ?? false) {
                (a: Any?, b: Any?, c: Any?, d: Any?, next: Rutis.Next<Any>?) in
                try await row.listen([a, b, c, d], next)
            }
        }
        if let provides = participant.provides { ctx.provide(provides, Present()) }
    }))
