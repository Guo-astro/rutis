import Foundation

/// A value as the session carries it: data, tagged values, and references.
/// The plugin API converts between these and Swift values
/// (`RutisEncoder` / `RutisDecoder`); the session only moves them.
///
/// `@unchecked Sendable`, and every case is safe to share: data, tagged
/// values and lists are value types; a function, object or plugin is an
/// immutable reference to code the SDK calls concurrently by contract
/// (design §5, "plugin requirements"), or to a `Far*` whose mutable state
/// is behind its peer's lock. The enum itself is never mutated after it is
/// built.
package indirect enum Value: @unchecked Sendable {
    /// The wire's `undefined`: an omitted argument or field.
    case undefined
    /// JSON data, copied.
    case data(JSON)
    /// An array holding a reference or tagged value somewhere.
    case list([Value])
    /// An object holding a reference or tagged value somewhere.
    case record([(String, Value)])
    case instant(Instant)
    case dateTime(Rutis.Advanced.DateTime)
    case date(Rutis.Advanced.CalendarDate)
    case time(Rutis.Advanced.TimeOfDay)
    case bytes(Data)
    case bigint(Rutis.Advanced.BigInt)
    /// Decimal text, as it crosses.
    case decimal(String)
    /// UUID text, lowercase.
    case uuid(String)
    /// A map's entries in the sender's order.
    case map([(Value, Value)])
    /// A set's elements in the sender's order.
    case set([Value])
    /// A function of this side: the far end gets a reference.
    case function(SessionFunction)
    /// An object of this side: the far end gets a reference.
    case object(SessionObject)
    /// A function of the far end.
    case remoteFunction(FarFunction)
    /// An object with methods of the far end (#687).
    case remoteObject(FarObject)
    /// An asynchronous result of the far end.
    case remoteFuture(FarFuture)
    /// A plugin definition of this side (`Rutis.define`): the far end gets
    /// a `plugin` reference, only to hand it back (plugin values, §5).
    case plugin(Rutis.Definition)
    /// A plugin definition of the far end: only a test host (RutisTest)
    /// takes one, and holds the reference until it is handed back.
    case remotePlugin(FarPlugin)
    /// The cancellation a call received as an argument.
    case signal(Signal)

    package static let null = Value.data(.null)

    /// The tag of a tagged value, nil for any other.
    var tag: String? {
        switch self {
        case .instant: return "instant"
        case .dateTime: return "datetime"
        case .date: return "date"
        case .time: return "time"
        case .bytes: return "bytes"
        case .bigint: return "bigint"
        case .decimal: return "decimal"
        case .uuid: return "uuid"
        case .map: return "map"
        case .set: return "set"
        default: return nil
        }
    }

    /// What a value is, for errors: "null", "an instant", ...
    package var described: String {
        switch self {
        case .undefined: return "undefined"
        case let .data(json):
            switch json {
            case .null: return "null"
            case .bool: return "a boolean"
            case .number: return "a number"
            case .string: return "a string"
            case .array: return "an array"
            case .object: return "an object"
            }
        case .list: return "an array"
        case .record: return "an object"
        case .instant: return "an instant"
        case .function, .object, .remoteFunction, .remoteFuture, .remoteObject: return "a reference"
        case .plugin, .remotePlugin: return "a plugin definition"
        case .signal: return "a signal"
        case .bytes: return "bytes"
        default: return "a " + (tag ?? "value")
        }
    }

    /// Whether it holds no reference, signal or tagged value: then it crosses
    /// as plain data.
    var isPlain: Bool {
        switch self {
        case .undefined, .data: return true
        case let .list(items): return items.allSatisfy(\.isPlain)
        case let .record(fields): return fields.allSatisfy(\.1.isPlain)
        default: return false
        }
    }

    /// As JSON data, when it is plain (undefined inside data as null).
    var plainJSON: JSON? {
        switch self {
        case .undefined: return .null
        case let .data(json): return json
        case let .list(items):
            var out: [JSON] = []
            out.reserveCapacity(items.count)
            for item in items {
                guard let json = item.plainJSON else { return nil }
                out.append(json)
            }
            return .array(out)
        case let .record(fields):
            var out: [(String, JSON)] = []
            for (key, item) in fields {
                guard let json = item.plainJSON else { return nil }
                out.append((key, json))
            }
            return .object(out)
        default: return nil
        }
    }

    /// A list of `items`: data when every item is (a list or record holding
    /// no reference or tagged value crosses as data).
    package static func list(of items: [Value]) -> Value {
        if items.allSatisfy(\.isPlain), let json = Value.list(items).plainJSON { return .data(json) }
        return .list(items)
    }

    /// A record of `fields`: data when every field is.
    package static func record(of fields: [(String, Value)]) -> Value {
        if fields.allSatisfy(\.1.isPlain), let json = Value.record(fields).plainJSON { return .data(json) }
        return .record(fields)
    }

    /// The items of a list (data arrays too), nil for anything else.
    package var items: [Value]? {
        switch self {
        case let .list(items): return items
        case let .data(.array(items)): return items.map(Value.data)
        default: return nil
        }
    }

    /// The fields of a record (data objects too), nil for anything else.
    package var fields: [(String, Value)]? {
        switch self {
        case let .record(fields): return fields
        case let .data(.object(fields)): return fields.map { ($0.0, Value.data($0.1)) }
        default: return nil
        }
    }

    /// The field `key` of a record or data object.
    package subscript(key: String) -> Value? {
        guard let fields else { return nil }
        for (name, value) in fields.reversed() where name == key { return value }
        return nil
    }

    package var string: String? {
        if case let .data(.string(s)) = self { return s }
        return nil
    }

    package var isNullish: Bool {
        switch self {
        case .undefined, .data(.null): return true
        default: return false
        }
    }
}

/// A function this side exports: the far end calls it with its arguments.
///
/// `@unchecked Sendable`, and safe as far as this class goes: `body` is
/// assigned once in `init` and never written again, so two tasks calling it
/// read the same closure. What the closure captures is its builder's: the
/// SDK's own closures capture `Sendable` state (a `Peer`, a `Runtime`, a
/// `Value`), and a plugin's handler runs concurrently by the contract
/// (design §5, "plugin requirements").
package final class SessionFunction: @unchecked Sendable {
    package let body: ([Value]) async throws -> Value

    package init(_ body: @escaping ([Value]) async throws -> Value) {
        self.body = body
    }
}

/// An object this side exports: the far end calls its methods and reads its
/// properties.
package protocol SessionObject: AnyObject, Sendable {
    /// The object's identity: the same identity is one reference.
    var identity: ObjectIdentifier { get }
    func call(method: String, args: [Value]) async throws -> Value
    func get(property: String) async throws -> Value
}

/// Continuations waiting for something, each resumed once: by the thing,
/// or by its task's cancellation.
///
/// `Sendable` without an escape hatch: its one mutable state is the
/// `Locked` box, whose lock every access takes.
package final class Waiters: Sendable {
    private struct State {
        var done = false
        var next = 0
        var waiting: [Int: CheckedContinuation<Void, Never>] = [:]
    }

    private let state = Locked(State())

    var isDone: Bool { state.read { $0.done } }

    /// Marks it done and resumes every waiter; false if it already was.
    @discardableResult
    func finish() -> Bool {
        var waiting: [Int: CheckedContinuation<Void, Never>] = [:]
        let first = state.withLock { state in
            guard !state.done else { return false }
            state.done = true
            waiting = state.waiting
            state.waiting = [:]
            return true
        }
        for continuation in waiting.values { continuation.resume() }
        return first
    }

    /// Returns once done, or once the calling task is cancelled.
    func wait() async {
        let id: Int? = state.withLock { state in
            guard !state.done else { return nil }
            state.next += 1
            return state.next
        }
        guard let id else { return }
        await withTaskCancellationHandler {
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                let resumeNow = state.withLock { state in
                    guard !state.done, !Task.isCancelled else { return true }
                    state.waiting[id] = continuation
                    return false
                }
                if resumeNow { continuation.resume() }
            }
        } onCancel: {
            let continuation = state.withLock { $0.waiting.removeValue(forKey: id) }
            continuation?.resume()
        }
    }

    /// Waits at most `seconds`; whether it is done.
    func wait(seconds: Double) async -> Bool {
        if isDone { return true }
        await withTaskGroup(of: Void.self) { group in
            group.addTask { await self.wait() }
            group.addTask { try? await Task.sleep(nanoseconds: UInt64(max(0, seconds) * 1e9)) }
            await group.next()
            group.cancelAll()
        }
        return isDone
    }
}

/// The cancellation a call received: aborted once the caller cancels.
///
/// `Sendable` without an escape hatch: the abort state is a `Waiters` and
/// the handlers a `Locked` box; both take their lock on every access.
package final class Signal: Sendable {
    private let waiters = Waiters()
    private let handlers = Locked<[() -> Void]>([])

    package init() {}

    package var isAborted: Bool { waiters.isDone }

    package func abort() {
        guard waiters.finish() else { return }
        let handlers = self.handlers.withLock { handlers in
            let taken = handlers
            handlers = []
            return taken
        }
        for handler in handlers { handler() }
    }

    /// Runs `handler` once aborted (at once if it already is).
    package func onAbort(_ handler: @escaping () -> Void) {
        let ran = handlers.withLock { handlers in
            guard !waiters.isDone else { return false }
            handlers.append(handler)
            return true
        }
        if !ran { handler() }
    }

    /// Returns once aborted, or once the calling task is cancelled.
    package func wait() async { await waiters.wait() }
}

/// A one-shot event tasks can wait for.
package final class Event: Sendable {
    private let waiters = Waiters()

    package init() {}

    package var isSet: Bool { waiters.isDone }

    package func fire() { waiters.finish() }

    /// Returns once fired, or once the calling task is cancelled.
    package func wait() async { await waiters.wait() }

    /// Waits at most `seconds`; whether it fired.
    package func wait(seconds: Double) async -> Bool { await waiters.wait(seconds: seconds) }

    /// Blocks the calling thread until `body` returns (never from a task).
    package static func blockUntil(_ body: @escaping @Sendable () async -> Void) {
        let semaphore = DispatchSemaphore(value: 0)
        Task.detached {
            await body()
            semaphore.signal()
        }
        semaphore.wait()
    }

    /// Blocks the calling thread until it fires (never from a task).
    package func block() {
        let semaphore = DispatchSemaphore(value: 0)
        let waiter = self
        Task.detached {
            await waiter.wait()
            semaphore.signal()
        }
        semaphore.wait()
    }
}
