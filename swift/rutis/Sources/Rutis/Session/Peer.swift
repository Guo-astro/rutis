import Foundation

// One session of the rutis protocol, in the endpoint format (version 4),
// on a local channel or a network one.
//
// One thread reads the channel and parses frames; every incoming call runs
// in a Task of its own, so a synchronous call suspends only the Task that
// made it and any incoming call can run at any time: the session is
// reentrant. The call chain (`path`) of a call travels in a task-local:
// an incoming call's Task carries its chain, and outgoing calls take theirs
// from the Task they are made in.

package let endpointProtocol = 4
package let maxSafe: Int64 = 9_007_199_254_740_991

/// The event protocol of rutis 0.13 (#464), which both ends of every 0.13
/// session declare.
package let eventsCapability = "events-v2"

/// What every runtime of this SDK declares. `sync-wait`: its synchronous
/// calls say so (`sync: true`). Not `sync-stack`: every incoming call runs
/// in a Task of its own.
package let runtimeCapabilities = ["objects", "signals", "reentrant-sync", "sync-wait", eventsCapability] + valueCapabilities

/// How deep a value crossing a session may nest (#279): each array, object,
/// map or set is a level, so 64 nested arrays cross and 65 do not; a call's
/// argument list is not a level.
package let maxDepth = 64

/// The call chain of the running Task.
package enum CallChain {
    @TaskLocal package static var path: [String] = []
}

private func valid(id: String) -> Bool {
    !id.isEmpty && id.utf8.allSatisfy { ($0 >= 0x61 && $0 <= 0x7A) || ($0 >= 0x30 && $0 <= 0x39) || $0 == 0x2D }
}

/// Whether `id` is a valid endpoint id (`[a-z0-9-]+`).
package func validEndpointID(_ id: String) -> Bool { valid(id: id) }

private func sequenceNumber(_ text: Substring) -> UInt64? {
    guard let first = text.first, first != "0", text.allSatisfy({ $0.isASCII && $0.isNumber }) else { return nil }
    return UInt64(text)
}

/// Whether `text` is a call id another session may originate (`origin`).
private func validOrigin(_ text: String) -> Bool {
    guard let colon = text.lastIndex(of: ":"), sequenceNumber(text[text.index(after: colon)...]) != nil else { return false }
    var prefix = text[..<colon]
    if let slash = prefix.firstIndex(of: "/") {
        let session = prefix[..<slash]
        guard session.first == "s", sequenceNumber(session.dropFirst()) != nil || session == "s0" else { return false }
        prefix = prefix[prefix.index(after: slash)...]
        return valid(id: String(prefix))
    }
    return valid(id: String(prefix))
}

package struct Endpoint {
    package var local: String
    package var expected: String?
    /// Capabilities to declare beyond the session's.
    package var declare: [String]

    package init(local: String, expected: String? = nil, declare: [String] = []) {
        self.local = local
        self.expected = expected
        self.declare = declare
    }
}

package struct PeerOptions {
    /// Serves `invoke`: target "" is a control operation.
    package var dispatch: ((_ target: String, _ method: String, _ args: [Value]) async throws -> Value)?
    /// Sees each incoming control call (target "") on the reading thread,
    /// before the next frame is read: the calls a protocol needs handled in
    /// frame order. It returns the rest of the call, run in a Task as
    /// `dispatch` would be, or nil to have `dispatch` serve it.
    package var arrive: ((_ method: String, _ args: [Value]) -> (() async throws -> Value)?)?
    /// This side's endpoint, and the far end it expects.
    package var endpoint: Endpoint
    /// Sent in the endpoint handshake.
    package var implementation: [(String, String)] = [("name", Rutis.Advanced.implementation), ("version", Rutis.Advanced.sdkVersion)]
    /// What this side declares in the endpoint format.
    package var capabilities: [String] = runtimeCapabilities
    /// This side is a test host driving a runtime (RutisTest): it takes the
    /// far end's plugin definitions, and declares `plugins` as rutis does.
    package var host = false
    /// Capabilities the far end must declare: the session ends right after
    /// its handshake when it does not, refused for the reason `refuse` gives.
    package var require: [String] = []
    package var refuse: ((String) -> String)?
    /// A test host (RutisTest) in the runtime's process: the plugin
    /// definition the runtime exported as `id`, so that a test double gets
    /// the definition itself, as the runtime would take it back.
    package var localPlugin: ((_ id: Int64) -> Rutis.Definition?)?

    package init(endpoint: Endpoint) {
        self.endpoint = endpoint
    }
}

/// Why a session ended.
package struct SessionEnded: Error, CustomStringConvertible, CustomErrorName {
    package enum Kind { case closed, refused, incompatible, mismatch, failed }
    package let kind: Kind
    package let reason: String
    /// Why the host went, for a call's `peer-gone` data (`disconnected`,
    /// `closed`, or what failed); nil for a refusal.
    package let gone: String?
    package init(_ kind: Kind, _ reason: String, gone: String? = nil) {
        self.kind = kind
        self.reason = reason
        self.gone = gone
    }

    /// The host disconnected: `the host disconnected`, or with why in
    /// parentheses.
    package static func hostGone(_ why: String? = nil) -> SessionEnded {
        guard let why else { return SessionEnded(.closed, "the host disconnected", gone: "disconnected") }
        return SessionEnded(.failed, "the host disconnected (\(why))", gone: why)
    }

    package var description: String {
        switch kind {
        case .incompatible: return "incompatible session: \(reason)"
        case .mismatch: return "endpoint mismatch: \(reason)"
        default: return reason
        }
    }

    package var rutisErrorName: String { RutisError.Name.bindingError }
}

/// The greeting of the far end.
package struct Greeting {
    package var endpoint: String
    package var implementation: JSON?
    package var capabilities: [String]
}

final class Imported: @unchecked Sendable {
    let id: Int64
    let kind: String
    let origin: [String]
    var grants: Int64 = 1
    var released = false
    weak var function: FarFunction?
    weak var future: FarFuture?
    weak var plugin: FarPlugin?
    weak var object: FarObject?

    init(id: Int64, kind: String, origin: [String]) {
        self.id = id
        self.kind = kind
        self.origin = origin
    }
}

enum ExportedValue {
    case function(SessionFunction)
    case object(SessionObject)
    case plugin(Rutis.Definition)
}

final class Exported {
    let value: ExportedValue
    let kind: String
    let origin: [String]
    var grants: Int64 = 0

    init(value: ExportedValue, kind: String, origin: [String]) {
        self.value = value
        self.kind = kind
        self.origin = origin
    }
}

/// A function of the far end.
package final class FarFunction: @unchecked Sendable {
    let peer: Peer
    let record: Imported

    init(peer: Peer, record: Imported) {
        self.peer = peer
        self.record = record
    }

    deinit {
        let peer = self.peer, record = self.record
        DispatchQueue.global().async { peer.release(record) }
    }

    /// Calls the function: synchronously (the far end may call back on this
    /// chain while it runs, and a host checking wait cycles hears so) or
    /// asynchronously (cancelling the Task cancels the call).
    package func call(_ args: [Value], sync: Bool) async throws -> Value {
        try peer.check(record)
        let fields = peer.waiting([("reference", .int(record.id))], sync)
        let value = try await peer.call(op: "call", fields: fields, args: args, origin: [], cancellable: !sync)
        return sync ? value : try await settle(value)
    }

    /// Gives the reference back now instead of when it goes.
    package func release() { peer.release(record) }
}

/// An object with methods of the far end (#687): calls of its methods reach
/// the original object. It has no method table: a method it lacks is the far
/// end's to answer (no-method).
package final class FarObject: @unchecked Sendable {
    let peer: Peer
    let record: Imported
    let lock = Lock()
    /// Its client, while one is held: one object is one client.
    weak var client: Rutis.Advanced.Client?

    init(peer: Peer, record: Imported) {
        self.peer = peer
        self.record = record
    }

    deinit {
        let peer = self.peer, record = self.record
        DispatchQueue.global().async { peer.release(record) }
    }

    /// Calls the method `method` asynchronously (cancelling the Task cancels
    /// the call).
    package func call(_ method: String, _ args: [Value], sync: Bool) async throws -> Value {
        try peer.check(record)
        let fields = peer.waiting([("reference", .int(record.id)), ("method", .string(method))], sync)
        let value = try await peer.call(op: "call", fields: fields, args: args, origin: [], cancellable: !sync)
        return sync ? value : try await settle(value)
    }

    /// Gives the reference back now instead of when it goes.
    package func release() { peer.release(record) }
}

/// A plugin definition of the far end, held by a test host (RutisTest):
/// it can only be handed back. `local` is the definition itself when the
/// runtime is in this process.
package final class FarPlugin: @unchecked Sendable {
    let peer: Peer
    let record: Imported
    package let local: Rutis.Definition?

    init(peer: Peer, record: Imported, local: Rutis.Definition?) {
        self.peer = peer
        self.record = record
        self.local = local
    }

    deinit {
        let peer = self.peer, record = self.record
        DispatchQueue.global().async { peer.release(record) }
    }
}

/// An asynchronous result of the far end.
package final class FarFuture: @unchecked Sendable {
    let peer: Peer
    let record: Imported
    private let lock = Lock()
    private var result: Result<Value, Error>?

    init(peer: Peer, record: Imported) {
        self.peer = peer
        self.record = record
    }

    deinit {
        let peer = self.peer, record = self.record
        DispatchQueue.global().async { peer.release(record) }
    }

    /// Waits for the result; cancelling the Task stops waiting.
    package func value() async throws -> Value {
        lock.lock()
        if let result {
            lock.unlock()
            return try result.get()
        }
        lock.unlock()
        try peer.check(record)
        do {
            let value = try await peer.call(op: "await", fields: [("reference", .int(record.id))], args: nil,
                                            origin: record.origin, cancellable: true)
            lock.lock()
            result = .success(value)
            lock.unlock()
            return value
        } catch {
            if !(error is CancellationError) {
                lock.lock()
                result = .failure(error)
                lock.unlock()
            }
            throw error
        }
    }
}

/// Awaits `value` while it is a future.
package func settle(_ value: Value) async throws -> Value {
    var value = value
    while case let .remoteFuture(future) = value {
        value = try await future.value()
    }
    return value
}

/// The wait for one outgoing call's answer.
final class Pending: @unchecked Sendable {
    let id: String
    private let lock = Lock()
    private var outcome: Result<Value, Error>?
    private var continuation: CheckedContinuation<Value, Error>?
    /// Told the answer where it is taken (the reader, as it reads the
    /// reply's frame), before the waiter.
    private let answered: (@Sendable (Result<Value, Error>) -> Void)?

    init(id: String, answered: (@Sendable (Result<Value, Error>) -> Void)? = nil) {
        self.id = id
        self.answered = answered
    }

    /// Answers the call; false if it already was.
    @discardableResult
    func resolve(_ result: Result<Value, Error>) -> Bool {
        lock.lock()
        if outcome != nil {
            lock.unlock()
            return false
        }
        outcome = result
        let continuation = self.continuation
        self.continuation = nil
        lock.unlock()
        answered?(result)
        continuation?.resume(with: result)
        return true
    }

    func wait() async throws -> Value {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Value, Error>) in
            lock.lock()
            if let outcome {
                lock.unlock()
                continuation.resume(with: outcome)
                return
            }
            self.continuation = continuation
            lock.unlock()
        }
    }
}

/// An incoming call that runs.
final class Running {
    var task: Task<Void, Never>?
    var cancelled = false
    let business: Bool
    init(business: Bool) { self.business = business }
}

/// The state of decoding one frame's value: the call it is the arguments of
/// (nil elsewhere), the references it imported anew, and the first value
/// error. A value error does not stop the decoding: every reference in the
/// value is still imported, so that a refused call releases each one.
final class Decoding {
    let call: String?
    var fresh: [Imported] = []
    var refused: Error?
    /// The value is a call's argument list, whose items are whole arguments
    /// and keep undefined (#687 §七); only the outermost list.
    var arguments = false

    init(call: String?) { self.call = call }

    func refuse(_ error: Error) {
        if refused == nil { refused = error }
    }
}

/// A frame the session cannot take: it ends the session.
struct Malformed: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

/// One session.
package final class Peer: @unchecked Sendable {
    package let channel: Channel
    let options: PeerOptions

    private let lock = Lock()
    /// Spans taking a call id and writing its frame: the far end refuses an
    /// id not above the last it received, so ids go out in order.
    private let writeLock = Lock()

    private var local: String
    private var remote: String
    private var next: UInt64 = 0
    private var received: UInt64 = 0
    private var ref: Int64 = 0
    private var handshake = false
    private var greeting: Greeting?
    private var declared: [String] = []

    private var pending: [String: Pending] = [:]
    private var exports: [Int64: Exported] = [:]
    private var identities: [ObjectIdentifier: Int64] = [:]
    private var imports: [Int64: Imported] = [:]
    /// A test host's plugin definitions of the far end, by the definition
    /// itself, held until the session ends (or the reference is released):
    /// one handed back goes home as the original reference, as rutis sends
    /// it. So the test host never lets one go before the session ends, and
    /// a test does not see rutis dropping a definition and the runtime
    /// releasing it with it.
    private var heldPlugins: [ObjectIdentifier: FarPlugin] = [:]
    private var running: [String: Running] = [:]
    private var signals: [String: Signal] = [:]
    private var active = 0
    private var draining: [Event] = []

    private let readyEvent = Event()
    private let closedEvent = Event()
    private var closeError: Error?

    package init(channel: Channel, options: PeerOptions) throws {
        self.channel = channel
        self.options = options
        guard valid(id: options.endpoint.local) else { throw ChannelError("invalid endpoint id \(options.endpoint.local)") }
        local = options.endpoint.local + ":"
        remote = ""
    }

    /// Sends the handshake and starts reading.
    package func start() throws {
        let thread = Thread { [self] in read() }
        thread.stackSize = 8 << 20
        thread.name = "rutis session"
        thread.start()
        let endpoint = options.endpoint
        // A test host takes plugin definitions, as rutis does.
        let capabilities = options.capabilities + endpoint.declare + (options.host ? ["plugins"] : [])
        try send(.object([
            ("op", .string("hello")),
            ("version", .int(endpointProtocol)),
            ("endpoint", .string(endpoint.local)),
            ("implementation", .object(options.implementation.map { ($0.0, .string($0.1)) })),
            ("capabilities", .array(capabilities.map(JSON.string))),
        ]))
    }

    /// Returns once the far end greeted (or the session closed).
    package func ready() async { await readyEvent.wait() }

    /// Returns once the session ended.
    package func closed() async { await closedEvent.wait() }

    package var isClosed: Bool { closedEvent.isSet }

    /// Why the session ended, once it did.
    package var error: Error? {
        lock.lock()
        defer { lock.unlock() }
        return closeError
    }

    /// Whether the far end declared `capability`.
    package func supports(_ capability: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return declared.contains(capability)
    }

    package var farGreeting: Greeting? {
        lock.lock()
        defer { lock.unlock() }
        return greeting
    }

    /// The sizes of the reference tables: what this side exports and what
    /// it holds of the far end's (the release-count property test).
    package var referenceTables: (exports: Int, imports: Int) {
        lock.lock()
        defer { lock.unlock() }
        return (exports.count, imports.values.filter { !$0.released }.count)
    }

    /// The number of incoming calls running now.
    package var incoming: Int {
        lock.lock()
        defer { lock.unlock() }
        return running.count
    }

    // ── I/O ──────────────────────────────────────────────────────────

    private func send(_ frame: JSON) throws {
        let data = JSONWriter.write(frame)
        writeLock.lock()
        defer { writeLock.unlock() }
        try write(data)
    }

    /// Writes a frame; writeLock is held.
    private func write(_ data: [UInt8]) throws {
        if let error = self.error { throw error }
        try channel.send(data)
    }

    private func read() {
        while true {
            let message: [UInt8]?
            do {
                message = try channel.receive()
            } catch {
                // The channel goes too: nothing reads it any more, and its
                // descriptor would stay open.
                fault(SessionEnded.hostGone(errorMessage(error)))
                return
            }
            guard let message else {
                close(SessionEnded.hostGone())
                return
            }
            do {
                let frame = try JSONReader.parse(message)
                try receive(frame)
            } catch let error as SessionEnded {
                fault(error)
                return
            } catch {
                fault(SessionEnded.hostGone(errorMessage(error)))
                return
            }
        }
    }

    /// Ends the session with `error` (default: closed).
    package func close(_ error: Error? = nil) {
        lock.lock()
        if closeError != nil {
            lock.unlock()
            return
        }
        let error = error ?? SessionEnded(.closed, "the connection to the host is closed", gone: "closed")
        closeError = error
        let pending = self.pending
        self.pending = [:]
        exports = [:]
        identities = [:]
        imports = [:]
        let held = heldPlugins
        heldPlugins = [:]
        let draining = self.draining
        self.draining = []
        let running = Array(self.running.values)
        lock.unlock()
        readyEvent.fire()
        let failure = RutisError.ended(error)
        for waiter in pending.values { waiter.resolve(.failure(failure)) }
        for event in draining { event.fire() }
        for call in running { call.task?.cancel() }
        _ = held
        closedEvent.fire()
    }

    private func fault(_ error: Error) {
        close(error)
        channel.close()
    }

    // ── References ───────────────────────────────────────────────────

    func check(_ record: Imported) throws {
        lock.lock()
        defer { lock.unlock() }
        if record.released { throw RutisError(name: "Error", message: "reference released") }
    }

    func release(_ record: Imported) {
        lock.lock()
        if record.released || closeError != nil {
            record.released = true
            lock.unlock()
            return
        }
        record.released = true
        if imports[record.id] === record { imports[record.id] = nil }
        // A released plugin definition is no longer held; dropped once the
        // lock is released.
        var dropped: [FarPlugin] = []
        if record.kind == "plugin" {
            for (key, held) in heldPlugins where held.record === record {
                dropped.append(held)
                heldPlugins[key] = nil
            }
        }
        let count = record.grants
        lock.unlock()
        _ = dropped
        do {
            try send(.object([("op", .string("release")), ("reference", .int(record.id)), ("count", .int(count))]))
        } catch {
            fault(error)
        }
    }

    /// Gives the references a refused value carried back.
    private func releaseAll(_ records: [Imported]) {
        for record in records { release(record) }
    }

    /// Exports `value`; lock is held. `grants` collects the exports granted,
    /// for a rollback.
    private func export(_ value: ExportedValue, kind: String, identity: ObjectIdentifier?, origin: [String],
                        grants: inout [Int64]) -> JSON
    {
        var entry: Exported?
        var id: Int64 = 0
        if let identity, let found = identities[identity], let existing = exports[found] {
            entry = existing
            id = found
        }
        if entry == nil {
            ref += 1
            id = ref
            let made = Exported(value: value, kind: kind, origin: origin)
            exports[id] = made
            if let identity { identities[identity] = id }
            entry = made
        }
        entry!.grants += 1
        grants.append(id)
        return .object([("type", .string("reference")), ("value", .object([
            ("id", .int(id)), ("kind", .string(entry!.kind)), ("home", .bool(false)),
            ("origin", .array(entry!.origin.map(JSON.string))),
        ]))])
    }

    private func rollback(_ grants: [Int64]) {
        for id in grants {
            guard let entry = exports[id] else { continue }
            entry.grants -= 1
            if entry.grants == 0 { forget(id) }
        }
    }

    private func forget(_ id: Int64) {
        exports[id] = nil
        for (key, value) in identities where value == id { identities[key] = nil }
    }

    // ── Values ───────────────────────────────────────────────────────

    /// The wire form of `value`; lock is held. `depth` counts the arrays and
    /// objects around it (an argument list starts at -1: it is not a level).
    private func encode(_ value: Value, _ grants: inout [Int64], _ origin: [String], _ depth: Int) throws -> JSON {
        switch value {
        case .list, .record, .map, .set:
            if depth >= maxDepth { throw tooDeep }
        default: break
        }
        if let tag = value.tag, !declared.contains("value-" + tag) {
            throw ValueError("cannot send \(value.described): the far end does not take \(tag) values (it did not declare the capability value-\(tag))")
        }
        switch value {
        case .undefined:
            return .object([("type", .string("undefined"))])
        case let .data(json):
            if depth + json.depth > maxDepth { throw tooDeep }
            if case let .number(text) = json, JSONNumber.isIntegerLiteral(text), let big = Rutis.Advanced.BigInt(text), !big.isSafe {
                return try encode(.bigint(big), &grants, origin, depth)
            }
            if containsUnsafeInteger(json) {
                // Integers beyond ±(2^53−1) cross as bigints (design V6).
                return try encode(liftUnsafe(json), &grants, origin, depth)
            }
            return .object([("type", .string("data")), ("value", json)])
        case let .list(items):
            var out: [JSON] = []
            out.reserveCapacity(items.count)
            // Undefined inside a value is null (#687 §七); an argument list
            // (depth -1) keeps it, as an omitted argument.
            for item in items { out.append(try encode(item.isUndefined && depth >= 0 ? .null : item, &grants, origin, depth + 1)) }
            return .object([("type", .string("list")), ("value", .array(out))])
        case let .record(fields):
            var out: [(String, JSON)] = []
            for (key, item) in fields { out.append((key, try encode(item.isUndefined ? .null : item, &grants, origin, depth + 1))) }
            return .object([("type", .string("record")), ("value", .object(out))])
        case let .map(entries):
            try checkKeys(entries.map(\.0), "map key")
            var out: [JSON] = []
            for (key, item) in entries {
                let k = try encode(key, &grants, origin, depth + 1)
                // Undefined is null in data (design V10).
                let v = try encode(item.isUndefined ? .null : item, &grants, origin, depth + 1)
                out.append(.array([k, v]))
            }
            return .object([("type", .string("map")), ("value", .array(out))])
        case let .set(elements):
            try checkKeys(elements, "set element")
            var out: [JSON] = []
            for element in elements { out.append(try encode(element, &grants, origin, depth + 1)) }
            return .object([("type", .string("set")), ("value", .array(out))])
        case .instant, .dateTime, .date, .time, .bytes, .bigint, .decimal, .uuid:
            let text = try taggedText(value)!
            return .object([("type", .string(value.tag!)), ("value", .string(text))])
        case let .function(function):
            return export(.function(function), kind: "function", identity: ObjectIdentifier(function), origin: origin, grants: &grants)
        case let .object(object):
            // Only to a far end that can call its methods; only this call
            // fails (#675).
            if !declared.contains("objects") {
                let shown: String
                if let service = object as? ServiceObject {
                    shown = String(describing: Swift.type(of: service.service))
                } else {
                    shown = String(describing: Swift.type(of: object))
                }
                let far = greeting?.endpoint ?? "the far end"
                // One older than rutis 0.18 takes none.
                throw ValueError("an object with methods (\(shown)) cannot cross to \(far); pass plain data or a function instead, or upgrade \(far) to rutis 0.18")
            }
            return export(.object(object), kind: "object", identity: object.identity, origin: origin, grants: &grants)
        case let .plugin(definition):
            // A test host hands a definition of the far end back home.
            if let held = heldPlugins[ObjectIdentifier(definition)], !held.record.released {
                return homeReference(held.record)
            }
            // Only to a far end that takes them; only this call fails.
            if !declared.contains("plugins") {
                throw ValueError("cannot send a plugin definition: the far end does not accept plugin definitions (it did not declare the capability plugins)")
            }
            return export(.plugin(definition), kind: "plugin", identity: ObjectIdentifier(definition), origin: origin, grants: &grants)
        case let .remotePlugin(plugin):
            if plugin.record.released { throw RutisError(name: "Error", message: "reference released") }
            return homeReference(plugin.record)
        case let .remoteFunction(function):
            if function.record.released { throw RutisError(name: "Error", message: "reference released") }
            return homeReference(function.record)
        case let .remoteObject(object):
            if object.record.released { throw RutisError(name: "Error", message: "reference released") }
            return homeReference(object.record)
        case let .remoteFuture(future):
            if future.record.released { throw RutisError(name: "Error", message: "reference released") }
            return homeReference(future.record)
        case .signal:
            throw ValueError("a signal cannot cross processes")
        }
    }

    private var tooDeep: ValueError {
        ValueError("value nested deeper than \(maxDepth) levels, the most a value crossing processes may nest")
    }

    private func homeReference(_ record: Imported) -> JSON {
        .object([("type", .string("reference")), ("value", .object([
            ("id", .int(record.id)), ("kind", .string(record.kind)), ("home", .bool(true)),
            ("origin", .array(record.origin.map(JSON.string))),
        ]))])
    }

    /// The value `wire` names; lock is held. A malformed wire value throws
    /// (the session ends); a value this side cannot take is refused in
    /// `decoding`, failing only its call.
    private func decode(_ wire: JSON?, _ decoding: Decoding) throws -> Value {
        guard let wire else { return .undefined }
        guard case .object = wire, let type = wire["type"]?.stringValue else { throw Malformed("invalid wire value") }
        let inner = wire["value"]
        switch type {
        case "undefined": return .undefined
        case "data": return .data(inner ?? .null)
        case "list":
            guard let items = inner?.arrayValue else { throw Malformed("invalid list") }
            // Undefined inside a value is null, also from an older far end
            // (#687 §七); the items of an argument list keep it.
            let keep = decoding.arguments
            decoding.arguments = false
            return .list(try items.map { let item = try decode($0, decoding); return item.isUndefined && !keep ? .null : item })
        case "record":
            guard let fields = inner?.objectValue else { throw Malformed("invalid record") }
            decoding.arguments = false
            return .record(try fields.map { let item = try decode($0.1, decoding); return ($0.0, item.isUndefined ? .null : item) })
        case "signal":
            guard let call = decoding.call else { throw Malformed("a signal is only valid as a call argument") }
            if let signal = signals[call] { return .signal(signal) }
            let signal = Signal()
            signals[call] = signal
            return .signal(signal)
        case "reference":
            return try decodeReference(inner, decoding)
        case "map", "set":
            guard let items = inner?.arrayValue else { throw Malformed("invalid \(type)") }
            let before = decoding.refused
            if type == "set" {
                let elements = try items.map { try decode($0, decoding) }
                if decoding.refused != nil, before == nil { return .undefined }
                do { try checkKeys(elements, "set element") } catch { decoding.refuse(error) }
                return .set(elements)
            }
            var entries: [(Value, Value)] = []
            for item in items {
                guard let pair = item.arrayValue, pair.count == 2 else { throw Malformed("invalid map") }
                let key = try decode(pair[0], decoding)
                var value = try decode(pair[1], decoding)
                // Undefined is null in data (design V10).
                if value.isUndefined { value = .null }
                entries.append((key, value))
            }
            if decoding.refused != nil, before == nil { return .undefined }
            do { try checkKeys(entries.map(\.0), "map key") } catch { decoding.refuse(error) }
            return .map(entries)
        default:
            guard valueTags.contains(type) else { throw Malformed("invalid wire value") }
            guard let text = inner?.stringValue else { throw Malformed("invalid \(type)") }
            do {
                return try parseTagged(type, text)
            } catch {
                decoding.refuse(error)
                return .undefined
            }
        }
    }

    private func decodeReference(_ json: JSON?, _ decoding: Decoding) throws -> Value {
        let invalid = Malformed("invalid reference")
        guard let json, case .object = json, let idJSON = json["id"], case let .number(idText) = idJSON,
              let id = JSONNumber.integer(idText), id > 0, id <= maxSafe,
              let kind = json["kind"]?.stringValue, let home = json["home"]?.boolValue,
              let originJSON = json["origin"]?.arrayValue else { throw invalid }
        guard kind == "function" || kind == "future" || kind == "object" || kind == "plugin" else { throw invalid }
        var origin: [String] = []
        for item in originJSON {
            guard let text = item.stringValue, validOrigin(text) else { throw invalid }
            origin.append(text)
        }
        if home {
            guard let entry = exports[id], entry.kind == kind else {
                throw Malformed("unknown, released or mismatched reference")
            }
            switch entry.value {
            case let .function(function): return .function(function)
            case let .object(object): return .object(object)
            case let .plugin(definition): return .plugin(definition)
            }
        }
        // Only a test host takes the far end's plugin definitions; rutis
        // never sends one that is not this side's.
        if kind == "plugin", !options.host { throw Malformed("plugin definitions from the far end are not supported") }
        if let record = imports[id] {
            let live: Value?
            switch record.kind {
            case "function": live = record.function.map(Value.remoteFunction)
            case "plugin": live = record.plugin.map(Value.remotePlugin)
            case "object": live = record.object.map(Value.remoteObject)
            default: live = record.future.map(Value.remoteFuture)
            }
            if let live {
                guard record.kind == kind, record.origin == origin else { throw Malformed("invalid repeated grant") }
                record.grants += 1
                return live
            }
        }
        let record = Imported(id: id, kind: kind, origin: origin)
        imports[id] = record
        decoding.fresh.append(record)
        if kind == "function" {
            let proxy = FarFunction(peer: self, record: record)
            record.function = proxy
            return .remoteFunction(proxy)
        }
        if kind == "object" {
            let proxy = FarObject(peer: self, record: record)
            record.object = proxy
            return .remoteObject(proxy)
        }
        if kind == "plugin" {
            let proxy = FarPlugin(peer: self, record: record, local: options.localPlugin?(id))
            record.plugin = proxy
            if let local = proxy.local { heldPlugins[ObjectIdentifier(local)] = proxy }
            return .remotePlugin(proxy)
        }
        let proxy = FarFuture(peer: self, record: record)
        record.future = proxy
        return .remoteFuture(proxy)
    }

    /// The plugin definition exported as `id`, while the far end holds it.
    package func exportedPlugin(_ id: Int64) -> Rutis.Definition? {
        lock.lock()
        defer { lock.unlock() }
        if case let .plugin(definition) = exports[id]?.value { return definition }
        return nil
    }

    /// Decodes a received value for a test or tool; lock is not held.
    package func decodeWire(_ wire: JSON) throws -> Value {
        lock.lock()
        defer { lock.unlock() }
        let decoding = Decoding(call: nil)
        let value = try decode(wire, decoding)
        if let refused = decoding.refused { throw refused }
        return value
    }

    // ── Outgoing calls ───────────────────────────────────────────────

    /// Marks a synchronous call's frame: this Task waits for the reply, and
    /// a host that checks for wait cycles needs to know. Only for a far end
    /// that declared `sync-wait`: others would refuse the field.
    func waiting(_ fields: [(String, JSON)], _ sync: Bool) -> [(String, JSON)] {
        if sync, supports("sync-wait") { return fields + [("sync", .bool(true))] }
        return fields
    }

    /// Invokes `method` of `target` on the far end: synchronously, or as an
    /// asynchronous call (cancelling the Task cancels it, and the far end
    /// hears so). The call carries the Task's chain.
    package func call(target: String, method: String, args: [Value], sync: Bool) async throws -> Value {
        let fields = waiting([("target", .string(target)), ("method", .string(method))], sync)
        return try await call(op: "invoke", fields: fields, args: args, origin: [], cancellable: !sync)
    }

    /// Sends a call now and returns the wait for its answer: frames sent by
    /// successive calls go out in that order.
    package func send(target: String, method: String, args: [Value], sync: Bool) throws -> OutgoingCall {
        let fields = waiting([("target", .string(target)), ("method", .string(method))], sync)
        let pending = try request(op: "invoke", fields: fields, args: args, origin: [], chain: CallChain.path)
        return OutgoingCall(peer: self, pending: pending, cancellable: !sync)
    }

    /// Sends a call nobody waits for, and tells `answered` its answer as the
    /// reply's frame is taken, before the next frame (or at the session's
    /// end): frames sent by successive calls go out in that order.
    package func sendAnswered(target: String, method: String, args: [Value],
                              answered: @escaping @Sendable (Result<Value, Error>) -> Void) throws
    {
        _ = try request(op: "invoke", fields: [("target", .string(target)), ("method", .string(method))],
                        args: args, origin: [], chain: CallChain.path, answered: answered)
    }

    /// Sends a call whose answer nobody awaits.
    package func notify(target: String, method: String, args: [Value]) {
        if let pending = try? request(op: "invoke", fields: [("target", .string(target)), ("method", .string(method))],
                                      args: args, origin: [], chain: [])
        {
            // Its reply, when it comes, is a late one: dropped.
            lock.lock()
            self.pending[pending.id] = nil
            lock.unlock()
        }
    }

    func call(op: String, fields: [(String, JSON)], args: [Value]?, origin: [String], cancellable: Bool) async throws -> Value {
        let pending = try request(op: op, fields: fields, args: args, origin: origin, chain: CallChain.path)
        return try await OutgoingCall(peer: self, pending: pending, cancellable: cancellable).value()
    }

    /// Gives up waiting for `pending`: an asynchronous call hears so.
    func abandon(_ pending: Pending, cancellable: Bool) {
        lock.lock()
        let waiting = self.pending[pending.id] != nil
        self.pending[pending.id] = nil
        lock.unlock()
        pending.resolve(.failure(CancellationError()))
        if waiting, cancellable {
            try? send(.object([("op", .string("cancel")), ("id", .string(pending.id))]))
        }
    }

    private func request(op: String, fields: [(String, JSON)], args: [Value]?, origin: [String], chain: [String],
                         answered: (@Sendable (Result<Value, Error>) -> Void)? = nil) throws -> Pending
    {
        writeLock.lock()
        defer { writeLock.unlock() }
        lock.lock()
        if let closeError {
            lock.unlock()
            throw RutisError.ended(closeError)
        }
        guard handshake else {
            lock.unlock()
            throw RutisError.binding("protocol handshake incomplete")
        }
        next += 1
        guard next <= UInt64(maxSafe) else {
            lock.unlock()
            throw RutisError.binding("call identifiers exhausted")
        }
        let id = local + String(next)
        var path: [String] = []
        var seen = Set<String>()
        for item in chain + origin where seen.insert(item).inserted { path.append(item) }
        var frame: [(String, JSON)] = [("op", .string(op)), ("id", .string(id)), ("path", .array(path.map(JSON.string)))]
        frame.append(contentsOf: fields)
        var grants: [Int64] = []
        // What cannot be sent fails only this call, here (#675).
        var encoded: [JSON] = []
        if let args {
            do {
                for (index, arg) in args.enumerated() {
                    do {
                        encoded.append(try encode(arg, &grants, chain, 0))
                    } catch let error as ValueError {
                        throw ofArgument(error.description, index, describe(op: op, fields: fields))
                    }
                }
                frame.append(("args", .object([("type", .string("list")), ("value", .array(encoded))])))
            } catch {
                rollback(grants)
                lock.unlock()
                throw error
            }
        }
        let data = JSONWriter.write(.object(frame))
        if data.count > maxMessage {
            rollback(grants)
            lock.unlock()
            // Blamed on the largest argument.
            let sizes = encoded.map { JSONWriter.write($0).count }
            let call = describe(op: op, fields: fields)
            if let index = sizes.indices.max(by: { sizes[$0] < sizes[$1] }) {
                throw ofArgument(overLimit(data.count), index, call, limit: maxMessage)
            }
            throw ValueError("\(call): the arguments cannot be sent: \(overLimit(data.count))",
                             data: ["part": "arguments", "limit": maxMessage])
        }
        let waiter = Pending(id: id, answered: answered)
        pending[id] = waiter
        lock.unlock()
        do {
            try write(data)
        } catch {
            lock.lock()
            pending[id] = nil
            rollback(grants)
            lock.unlock()
            throw error
        }
        return waiter
    }

    /// What an outgoing call is, for messages.
    private func describe(op: String, fields: [(String, JSON)]) -> String {
        func field(_ name: String) -> String? { fields.first { $0.0 == name }?.1.stringValue }
        switch op {
        case "invoke": return describeInvocation(target: field("target") ?? "", method: field("method") ?? "")
        case "call": return field("method").map { "the method \($0)" } ?? "a function"
        default: return "an awaited result"
        }
    }

    /// Waits until no call that is the far end's business runs.
    package func drain() async {
        lock.lock()
        if active == 0 || closeError != nil {
            lock.unlock()
            return
        }
        let event = Event()
        draining.append(event)
        lock.unlock()
        await event.wait()
    }

    private func finish(business: Bool) {
        guard business else { return }
        lock.lock()
        active -= 1
        var draining: [Event] = []
        if active == 0 {
            draining = self.draining
            self.draining = []
        }
        lock.unlock()
        for event in draining { event.fire() }
    }

    // ── Incoming frames ──────────────────────────────────────────────

    private func receive(_ frame: JSON) throws {
        guard case .object = frame, let op = frame["op"]?.stringValue else { throw Malformed("invalid frame") }
        lock.lock()
        if closeError != nil {
            lock.unlock()
            return
        }
        if op == "hello" {
            defer { lock.unlock() }
            if handshake { throw Malformed("duplicate protocol handshake") }
            try greet(frame)
            handshake = true
            readyEvent.fire()
            return
        }
        guard handshake else {
            lock.unlock()
            throw Malformed("request before protocol handshake")
        }
        switch op {
        case "return", "throw":
            guard let id = frame["id"]?.stringValue else {
                lock.unlock()
                throw Malformed("invalid response")
            }
            var outcome: Result<Value, Error>
            var fresh: [Imported] = []
            if op == "return" {
                let decoding = Decoding(call: nil)
                do {
                    outcome = .success(try decode(frame["value"], decoding))
                } catch {
                    lock.unlock()
                    throw error
                }
                if let refused = decoding.refused {
                    // A value this side cannot take fails the call waiting
                    // for it; the references it carried go back.
                    outcome = .failure(refused)
                    fresh = decoding.fresh
                }
            } else {
                guard let error = frame["error"] else {
                    lock.unlock()
                    throw Malformed("invalid response")
                }
                outcome = .failure(decodeError(error, decodeData: { ValueCodec.native($0) }))
            }
            guard let waiter = pending.removeValue(forKey: id) else {
                lock.unlock()
                releaseAll(fresh)
                // A late reply to a call of ours given up (cancelled, or a
                // notification) is dropped; any other is a fault.
                if issued(id) { return }
                throw Malformed("response for unknown call")
            }
            lock.unlock()
            releaseAll(fresh)
            waiter.resolve(outcome)
            return
        case "cancel":
            guard let id = frame["id"]?.stringValue else {
                lock.unlock()
                throw Malformed("invalid cancel")
            }
            let call = running[id]
            call?.cancelled = true
            let task = call?.task
            let signal = signals[id]
            lock.unlock()
            signal?.abort()
            task?.cancel()
            return
        case "release":
            defer { lock.unlock() }
            guard let reference = frame["reference"]?.int64Value, let entry = exports[reference] else {
                throw Malformed("unknown, released or mismatched reference")
            }
            guard let count = frame["count"]?.int64Value, count > 0, count <= entry.grants else {
                throw Malformed("invalid reference release count")
            }
            entry.grants -= count
            if entry.grants == 0 { forget(reference) }
            return
        default:
            break
        }
        let job: Job
        do {
            job = try invocation(frame, op: op)
        } catch {
            lock.unlock()
            throw error
        }
        lock.unlock()
        if let arrive = options.arrive, job.op == "invoke", job.target == "", job.refused == nil {
            let chain = job.path + [job.id]
            job.then = CallChain.$path.withValue(chain) { arrive(job.method ?? "", job.args) }
        }
        let task = Task.detached { [self] in await execute(job) }
        lock.lock()
        if let call = running[job.id] {
            call.task = task
            if call.cancelled { task.cancel() }
        }
        lock.unlock()
    }

    /// Whether `id` names a call this side made; lock is not held.
    private func issued(_ id: String) -> Bool {
        guard id.hasPrefix(local), let n = sequenceNumber(id.dropFirst(local.count)) else { return false }
        lock.lock()
        defer { lock.unlock() }
        return n <= next
    }

    /// Why the far end's protocol version is refused, and which end to upgrade.
    private func versionMismatch(_ frame: JSON, _ local: Int) -> String {
        let far = frame["version"].map(JSONWriter.string) ?? "null"
        let mismatch = "the far end speaks protocol \(far), this side \(local)"
        guard let version = frame["version"]?.int64Value, version != Int64(local) else { return mismatch }
        if version > Int64(local) { return mismatch + ": upgrade this side (Swift package \(Rutis.Advanced.implementation))" }
        if let name = frame["implementation"]?["name"]?.stringValue, let v = frame["implementation"]?["version"]?.stringValue {
            return "\(mismatch): upgrade the far end (\(name) \(v))"
        }
        return mismatch + ": upgrade the far end"
    }

    private func greet(_ frame: JSON) throws {
        let version = frame["version"]?.int64Value
        func strings(_ json: JSON?) -> [String] { json?.arrayValue?.compactMap(\.stringValue) ?? [] }
        let endpoint = options.endpoint
        guard version == Int64(endpointProtocol) else {
            throw SessionEnded(.incompatible, versionMismatch(frame, endpointProtocol))
        }
        guard let far = frame["endpoint"]?.stringValue, valid(id: far) else {
            throw SessionEnded(.incompatible, "the far end named no valid endpoint")
        }
        if let expected = endpoint.expected, expected != far {
            throw SessionEnded(.mismatch, "the far end greeted as \(far), but \(expected) is the expected endpoint")
        }
        if far == endpoint.local {
            throw SessionEnded(.mismatch, "the far end greeted as this endpoint (\(far))")
        }
        remote = far + ":"
        declared = strings(frame["capabilities"])
        greeting = Greeting(endpoint: far, implementation: frame["implementation"], capabilities: declared)
        try required()
    }

    /// Refuses a far end lacking a capability `require` lists; lock is held.
    private func required() throws {
        for capability in options.require where !declared.contains(capability) {
            throw SessionEnded(.refused, options.refuse?(capability) ?? "the far end does not declare \(capability)")
        }
    }

    final class Job: @unchecked Sendable {
        let op: String
        let id: String
        let path: [String]
        var target = ""
        var method: String?
        var property = ""
        var args: [Value] = []
        var entry: Exported?
        var business = false
        var refused: Error?
        var fresh: [Imported] = []
        var then: (() async throws -> Value)?

        init(op: String, id: String, path: [String]) {
            self.op = op
            self.id = id
            self.path = path
        }
    }

    /// Validates an incoming call; lock is held.
    private func invocation(_ frame: JSON, op: String) throws -> Job {
        let invalid = Malformed("invalid invocation")
        guard ["invoke", "call", "get", "await"].contains(op) else { throw invalid }
        guard let id = frame["id"]?.stringValue, let pathJSON = frame["path"]?.arrayValue else { throw invalid }
        var path: [String] = []
        for item in pathJSON {
            guard let text = item.stringValue, text != id else { throw invalid }
            path.append(text)
        }
        guard id.hasPrefix(remote), !remote.isEmpty, let seq = sequenceNumber(id.dropFirst(remote.count)) else { throw invalid }
        guard seq > received, seq <= UInt64(maxSafe) else { throw Malformed("invalid or repeated invocation identity") }
        received = seq
        let job = Job(op: op, id: id, path: path)
        if let method = frame["method"], !method.isNull {
            guard let text = method.stringValue else { throw Malformed("invalid method") }
            job.method = text
        }
        if op == "get" {
            guard let property = frame["property"]?.stringValue else { throw Malformed("invalid property") }
            job.property = property
        }
        if op == "invoke" || op == "call" {
            let decoding = Decoding(call: id)
            decoding.arguments = true
            let decoded = try decode(frame["args"], decoding)
            job.args = arguments(decoded)
            // Arguments this side cannot take fail the call: it is answered
            // by a throw, and the references they carried go back.
            job.refused = decoding.refused
            job.fresh = decoding.fresh
        }
        if op == "invoke" {
            guard let targetJSON = frame["target"], let target = targetJSON.stringValue, job.method != nil else { throw invalid }
            job.target = target
            job.business = !target.isEmpty
        } else {
            let kind = op == "await" ? "future" : (op == "get" || job.method != nil) ? "object" : "function"
            guard let reference = frame["reference"]?.int64Value, let entry = exports[reference], entry.kind == kind else {
                throw Malformed("unknown, released or mismatched reference")
            }
            job.entry = entry
            job.business = true
        }
        if job.business { active += 1 }
        running[id] = Running(business: job.business)
        return job
    }

    /// A call's argument list.
    private func arguments(_ decoded: Value) -> [Value] {
        switch decoded {
        case let .list(items): return items
        case .undefined, .data(.null): return []
        case let .data(.array(items)): return items.map(Value.data)
        default: return [decoded]
        }
    }

    private func execute(_ job: Job) async {
        defer { finish(business: job.business) }
        let chain = job.path + [job.id]
        // What it is, for a result that cannot be sent.
        let described: String?
        switch job.op {
        case "invoke": described = describeInvocation(target: job.target, method: job.method ?? "")
        case "call": described = job.method.map { "the method \($0)" } ?? "a function"
        case "get": described = "the property \(job.property)"
        default: described = nil
        }
        if let refused = job.refused {
            releaseAll(job.fresh)
            respond(job.id, .failure(refused), chain, described)
            return
        }
        // Where a service method this call ran threw (#677 §4.4).
        let site = ThrowSite()
        let result: Result<Value, Error> = await CallChain.$path.withValue(chain) {
            await ThrowSite.$current.withValue(site) {
                do {
                    if let then = job.then { return .success(try await then()) }
                    switch job.op {
                    case "invoke":
                        guard let dispatch = options.dispatch else {
                            throw RutisError(name: "Error", message: "no target \(job.target)")
                        }
                        return .success(try await dispatch(job.target, job.method!, job.args))
                    case "await":
                        throw RutisError(name: "Error", message: "this side exports no futures")
                    case "get":
                        guard case let .object(object) = job.entry!.value else { throw RutisError(name: "Error", message: "not an object") }
                        return .success(try await object.get(property: job.property))
                    default:
                        switch job.entry!.value {
                        case let .function(function):
                            return .success(try await function.body(job.args))
                        case let .object(object):
                            return .success(try await object.call(method: job.method ?? "", args: job.args))
                        case .plugin:
                            // Never reached: a call names a function or object.
                            throw RutisError(name: "Error", message: "a plugin definition cannot be called")
                        }
                    }
                } catch {
                    return .failure(error)
                }
            }
        }
        // Only a service method's own call is located (an invoke, a method of
        // an object passed by reference), not a function's.
        var method = job.then == nil && job.op == "invoke"
        if job.then == nil, job.op != "invoke", job.op != "await", job.op != "get", case .object = job.entry?.value { method = true }
        respond(job.id, result, chain, described, frame: method ? site.frame : nil)
    }

    /// Answers call `id`, a call of `call` (for messages): a result that
    /// cannot be sent (a value the far end does not take, a message over the
    /// limit) is answered with an error saying why instead (#675).
    private func respond(_ id: String, _ result: Result<Value, Error>, _ chain: [String], _ call: String?, frame: String? = nil) {
        lock.lock()
        running[id] = nil
        signals[id] = nil
        if closeError != nil {
            lock.unlock()
            return
        }
        var data: [UInt8]
        var failure: Error?
        // Only the method's own failure is located: a result that cannot be
        // sent is not where the method threw.
        var failureFrame: String?
        switch result {
        case let .success(value):
            var grants: [Int64] = []
            do {
                let encoded = try encode(value, &grants, chain, 0)
                data = JSONWriter.write(.object([("op", .string("return")), ("id", .string(id)), ("value", encoded)]))
                if data.count > maxMessage {
                    rollback(grants)
                    failure = ofResult(overLimit(data.count), call, limit: maxMessage)
                    data = []
                }
            } catch let error as ValueError {
                rollback(grants)
                failure = ofResult(error.description, call)
                data = []
            } catch {
                rollback(grants)
                failure = error
                data = []
            }
        case let .failure(error):
            failure = error
            failureFrame = frame
            data = []
        }
        if let failure {
            let encoded = encodeError(failure, frame: failureFrame) { value in try ValueCodec.encodeAny(value) }
            data = JSONWriter.write(.object([("op", .string("throw")), ("id", .string(id)), ("error", encoded)]))
            // An error too large to send: a short one saying so.
            if data.count > maxMessage {
                let what = "the error \(errorName(failure))" + (call.map { " of \($0)" } ?? "")
                let short: JSON = .object([("name", .string("Error")), ("message", .string("\(what): \(overLimit(data.count))"))])
                data = JSONWriter.write(.object([("op", .string("throw")), ("id", .string(id)), ("error", short)]))
            }
        }
        lock.unlock()
        do {
            try send(raw: data)
        } catch {
            fault(error)
        }
    }

    private func send(raw data: [UInt8]) throws {
        writeLock.lock()
        defer { writeLock.unlock() }
        try write(data)
    }
}

/// An outgoing call sent, waiting for its answer.
package struct OutgoingCall: Sendable {
    let peer: Peer
    let pending: Pending
    let cancellable: Bool

    /// The answer; cancelling the Task gives up waiting (an asynchronous
    /// call hears so).
    package func value() async throws -> Value {
        try await withTaskCancellationHandler {
            try await pending.wait()
        } onCancel: {
            peer.abandon(pending, cancellable: cancellable)
        }
    }
}

extension Value {
    var isUndefined: Bool {
        if case .undefined = self { return true }
        return false
    }
}

/// Whether data holds an integer beyond ±(2^53−1).
func containsUnsafeInteger(_ json: JSON) -> Bool {
    switch json {
    case let .number(text):
        guard JSONNumber.isIntegerLiteral(text) else { return false }
        if let big = Rutis.Advanced.BigInt(text) { return !big.isSafe }
        return false
    case let .array(items): return items.contains(where: containsUnsafeInteger)
    case let .object(fields): return fields.contains { containsUnsafeInteger($0.1) }
    default: return false
    }
}

/// Data with its integers beyond ±(2^53−1) as bigints.
func liftUnsafe(_ json: JSON) -> Value {
    switch json {
    case let .number(text):
        if JSONNumber.isIntegerLiteral(text), let big = Rutis.Advanced.BigInt(text), !big.isSafe { return .bigint(big) }
        return .data(json)
    case let .array(items): return .list(items.map(liftUnsafe))
    case let .object(fields): return .record(fields.map { ($0.0, liftUnsafe($0.1)) })
    default: return .data(json)
    }
}
