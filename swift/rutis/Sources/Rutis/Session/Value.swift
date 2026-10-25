import Foundation

/// A value as the session carries it: data, tagged values, and references.
/// The plugin API converts between these and Swift values
/// (`RutisEncoder` / `RutisDecoder`); the session only moves them.
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
    case dateTime(Rutis.DateTime)
    case date(Rutis.CalendarDate)
    case time(Rutis.TimeOfDay)
    case bytes(Data)
    case bigint(Rutis.BigInt)
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
        case .function, .object, .remoteFunction, .remoteFuture: return "a reference"
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
final class Waiters: @unchecked Sendable {
    private let lock = Lock()
    private var done = false
    private var next = 0
    private var waiting: [Int: CheckedContinuation<Void, Never>] = [:]

    var isDone: Bool {
        lock.lock()
        defer { lock.unlock() }
        return done
    }

    /// Marks it done and resumes every waiter; false if it already was.
    @discardableResult
    func finish() -> Bool {
        lock.lock()
        if done {
            lock.unlock()
            return false
        }
        done = true
        let waiting = self.waiting
        self.waiting = [:]
        lock.unlock()
        for continuation in waiting.values { continuation.resume() }
        return true
    }

    /// Returns once done, or once the calling task is cancelled.
    func wait() async {
        lock.lock()
        if done {
            lock.unlock()
            return
        }
        next += 1
        let id = next
        lock.unlock()
        await withTaskCancellationHandler {
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                lock.lock()
                if done || Task.isCancelled {
                    lock.unlock()
                    continuation.resume()
                    return
                }
                waiting[id] = continuation
                lock.unlock()
            }
        } onCancel: {
            lock.lock()
            let continuation = waiting.removeValue(forKey: id)
            lock.unlock()
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
package final class Signal: @unchecked Sendable {
    private let lock = Lock()
    private let waiters = Waiters()
    private var handlers: [() -> Void] = []

    package init() {}

    package var isAborted: Bool { waiters.isDone }

    package func abort() {
        lock.lock()
        guard waiters.finish() else {
            lock.unlock()
            return
        }
        let handlers = self.handlers
        self.handlers = []
        lock.unlock()
        for handler in handlers { handler() }
    }

    /// Runs `handler` once aborted (at once if it already is).
    package func onAbort(_ handler: @escaping () -> Void) {
        lock.lock()
        if waiters.isDone {
            lock.unlock()
            handler()
            return
        }
        handlers.append(handler)
        lock.unlock()
    }

    /// Returns once aborted, or once the calling task is cancelled.
    package func wait() async { await waiters.wait() }
}

/// A one-shot event tasks can wait for.
package final class Event: @unchecked Sendable {
    private let waiters = Waiters()

    package init() {}

    package var isSet: Bool { waiters.isDone }

    package func fire() { waiters.finish() }

    /// Returns once fired, or once the calling task is cancelled.
    package func wait() async { await waiters.wait() }

    /// Waits at most `seconds`; whether it fired.
    package func wait(seconds: Double) async -> Bool { await waiters.wait(seconds: seconds) }

    /// Blocks the calling thread until `body` returns (never from a task).
    package static func blockUntil(_ body: @escaping () async -> Void) {
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
        Task.detached {
            await self.wait()
            semaphore.signal()
        }
        semaphore.wait()
    }
}
