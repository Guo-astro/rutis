// RutisTest tests a plugin without a host (design §14, S21): it gives the
// plugin the services it injects, calls the services it provides, emits and
// listens to events as a host does, and unloads it.
//
//	let loaded = try await RutisTest.load(weatherPlugin, config: ["city": "Oslo"], services: ["clock": FakeClock()])
//	let today = try await loaded.service("weather").call("today", as: String.self)
//	await loaded.close()
//
// The plugin runs in its real runtime, across a real session (an in-memory
// socket pair): values cross as they would between processes, so what works
// only in one process fails here too. The test side is the host of the
// session and of the event bus: the plugin's listeners register with it,
// and Loaded's emit, parallel, serial, waterfall, bail and filter dispatch
// to them from that row's scope.
import Foundation
import Rutis

public enum RutisTest {
    /// Loads `plugin` with `config` (a Codable value or a dictionary; nil
    /// for none) and the services in `services` (instances of @RutisService
    /// types, their methods all `sync`). More rows load into the same
    /// runtime with `Loaded.load`.
    public static func load(_ plugin: Rutis.Definition, config: Any? = nil, services: [String: RutisExportable] = [:],
                            row: String = "test", scope: [Rutis.Advanced.Scope] = [], runtimeName: String = "swift") async throws -> Loaded
    {
        let harness = try await Harness.start(plugin, services: services, runtimeName: runtimeName)
        do {
            return try await harness.load(config: config, row: row, scope: scope)
        } catch {
            await harness.close()
            throw error
        }
    }

    /// The scope chain of the path `t1/s1`: one loader instance per segment,
    /// its id the path up to it, its group `groups[i]` (default: the
    /// segment).
    public static func scope(_ path: String, groups: [String] = []) -> [Rutis.Advanced.Scope] {
        if path.isEmpty || path == "root" { return [] }
        let segments = path.split(separator: "/").map(String.init)
        return segments.indices.map { i in
            Rutis.Advanced.Scope(id: segments[...i].joined(separator: "/"), group: i < groups.count ? groups[i] : segments[i])
        }
    }

    /// How long an unload may take: the runtime waits for running listeners
    /// up to RUTIS_SHUTDOWN_TIMEOUT (default 10 s), then runs the cleanups.
    static var unloadWait: Double { Events.shutdownTimeout + 30 }
}

/// The runtime's session, held weakly: it ends with the runtime.
final class WeakPeer: @unchecked Sendable {
    private let lock = Lock()
    private weak var peer: Peer?
    func set(_ peer: Peer) { lock.withLock { self.peer = peer } }
    func get() -> Peer? { lock.withLock { peer } }
}

/// A listener on the test side's bus: one of the runtime's (by its id) or
/// one of the test's own.
final class BusEntry: EventEntry, @unchecked Sendable {
    /// Weak: an invocation queued before the test host closed may run after
    /// it is gone, and then starts nothing.
    weak var harness: Harness?
    let id: String
    let name: String
    let row: String?
    let plugin: String
    let global: Bool
    let local: ListenerBody?
    var withdrawn = false
    let queue = SerialQueue()

    init(harness: Harness, id: String, name: String, row: String?, plugin: String, global: Bool, local: ListenerBody?) {
        self.harness = harness
        self.id = id
        self.name = name
        self.row = row
        self.plugin = plugin
        self.global = global
        self.local = local
    }

    var info: Rutis.Advanced.Listener {
        if local != nil { return Rutis.Advanced.Listener(name: name, plugin: "rutistest", row: nil, runtime: nil, scope: [], global: global) }
        let scope = harness.map { harness in harness.lock.withLock { harness.scopes[row ?? ""] ?? [] } } ?? []
        return Rutis.Advanced.Listener(name: name, plugin: plugin, row: row, runtime: harness?.runtimeName, scope: scope, global: global)
    }

    var live: Bool {
        guard let harness else { return false }
        return harness.lock.withLock { !withdrawn }
    }

    /// The test host, unless it is closed.
    func host() throws -> Harness {
        guard let harness else { throw RutisError.binding("the test host is closed") }
        return harness
    }

    func begin() -> EventCall? { live ? call : nil }

    /// The invocation within one dispatch.
    var dispatchID = ""
    var own = false

    func call(_ mode: Mode, _ args: [Value], _ next: NextFunction?) async throws -> Value {
        if let local { return try await local(args, next) }
        let value = try await host().session.call(target: "", method: "events.call", args: [record(mode, args, next)],
                                                   sync: mode == .bail || mode == .waterfallSync)
        return try await settle(value)
    }

    /// An `events.call` (§5.3).
    func record(_ mode: Mode, _ args: [Value], _ next: NextFunction?) -> Value {
        var fields: [(String, Value)] = [
            ("listener", .data(.string(id))), ("dispatch", .data(.string(dispatchID))), ("own", .data(.bool(own))),
            ("name", .data(.string(name))), ("mode", .data(.string(mode.rawValue))), ("args", .list(args)),
        ]
        if let next { fields.append(("next", .function(SessionFunction { _ in try await next() }))) }
        return .record(fields)
    }

    func emit(_ args: [Value]) -> Bool {
        guard live else { return false }
        if let local {
            let name = self.name
            queue.push { [self] in
                guard live else { return }
                do {
                    _ = try await local(args, nil)
                } catch {
                    Events.report("rutis: listener of \(name) in row rutistest failed: \(Events.failure(error))")
                }
            }
            return true
        }
        // Sent now, in order: the runtime starts them in arrival order.
        let report: (Error) -> Void = { [name, runtimeName = harness?.runtimeName ?? ""] error in
            Events.report("rutis: listener of \(name) in runtime \(runtimeName) failed: \(Events.failure(error))")
        }
        do {
            let call = try host().session.send(target: "", method: "events.call", args: [record(.emit, args, nil)], sync: false)
            Task.detached {
                do { _ = try await call.value() } catch { report(error) }
            }
        } catch {
            report(error)
        }
        return true
    }
}

/// An entry within one dispatch.
final class Invocation: EventEntry, @unchecked Sendable {
    let entry: BusEntry
    let dispatchID: String
    let own: Bool

    init(_ entry: BusEntry, dispatchID: String, own: Bool) {
        self.entry = entry
        self.dispatchID = dispatchID
        self.own = own
    }

    var info: Rutis.Advanced.Listener { entry.info }

    func begin() -> EventCall? {
        guard entry.live else { return nil }
        return { [self] mode, args, next in
            if let local = entry.local { return try await local(args, next) }
            let value = try await entry.host().session.call(target: "", method: "events.call", args: [record(mode, args, next)],
                                                             sync: mode == .bail || mode == .waterfallSync)
            return try await settle(value)
        }
    }

    func record(_ mode: Mode, _ args: [Value], _ next: NextFunction?) -> Value {
        entry.dispatchID = dispatchID
        entry.own = own
        return entry.record(mode, args, next)
    }

    func emit(_ args: [Value]) -> Bool {
        entry.dispatchID = dispatchID
        entry.own = own
        return entry.emit(args)
    }
}

/// The runtime under test and the host side of its session.
final class Harness: @unchecked Sendable {
    let plugin: Rutis.Definition
    /// The rows and config values warned about.
    var warned: Set<String> = []
    let services: [String: RutisExportable]
    let runtimeName: String
    var session: Peer!
    var served: Task<Void, Never>?
    let declaredProvides: [String: [String: String]]
    let declaredInject: [String]

    let lock = Lock()
    var handles: [String: String] = [:]
    var announced: [String: [String: String]] = [:]
    var scopes: [String: [Rutis.Advanced.Scope]] = [:]
    var rows: [String] = []
    var entries: [BusEntry] = []
    var dispatches = 0
    var reported: [String] = []
    var closed = false
    var stopHearing: (() -> Void)?
    let changed = Lock()
    var testListeners = 0

    init(plugin: Rutis.Definition, services: [String: RutisExportable], runtimeName: String) throws {
        self.plugin = plugin
        self.services = services
        self.runtimeName = runtimeName
        let manifest = try JSONReader.parse(Rutis.Advanced.manifest([plugin]))
        let declared = manifest["plugins"]?[plugin.name]
        declaredInject = declared?["inject"]?.arrayValue?.compactMap(\.stringValue) ?? []
        var provides: [String: [String: String]] = [:]
        for (name, kinds) in declared?["provides"]?.objectValue ?? [] {
            var methods: [String: String] = [:]
            for (method, kind) in kinds.objectValue ?? [] { methods[method] = kind.stringValue }
            provides[name] = methods
        }
        declaredProvides = provides
    }

    static func start(_ plugin: Rutis.Definition, services: [String: RutisExportable], runtimeName: String) async throws -> Harness {
        let harness = try Harness(plugin: plugin, services: services, runtimeName: runtimeName)
        for name in harness.declaredInject where services[name] == nil {
            throw RutisError(name: "Error", message: "RutisTest: the plugin injects \(name): give the test a service \(name)")
        }
        let (runtimeSide, hostSide) = try Channels.pair()
        // The runtime's session: a plugin definition the plugin hands to a
        // test double is the definition itself, as the runtime takes it
        // back (plugin values, R10).
        let runtimeSession = WeakPeer()
        let started = Event()
        harness.served = Task.detached {
            try? await Rutis.serveSession(channel: runtimeSide, project: FileManager.default.currentDirectoryPath, [plugin]) { peer in
                runtimeSession.set(peer)
                started.fire()
            }
            started.fire()
        }
        await started.wait()
        var options = PeerOptions()
        options.host = true
        options.localPlugin = { id in runtimeSession.get()?.exportedPlugin(id) }
        options.dispatch = { [weak harness] target, method, args in
            guard let harness else { throw RutisError(name: "Error", message: "the test host is closed") }
            return try await harness.dispatch(target, method, args)
        }
        options.arrive = { [weak harness] method, args in harness?.arrive(method, args) }
        let session = try Peer(channel: hostSide, options: options)
        harness.session = session
        harness.stopHearing = Events.hear { [weak harness] line in harness?.lock.withLock { harness?.reported.append(line) } }
        try session.start()
        await session.ready()
        if let error = session.error { throw error }
        // As a host does: the names with listeners elsewhere before the
        // mount (none yet); undeclared services are announced.
        _ = try await session.call(target: "", method: "events.elsewhere", args: [.data(.object([("add", .array([]))]))], sync: false)
        _ = try await session.call(target: "", method: "mount", args: [.data(.object([
            ("announce", .bool(true)), ("runtime", .string(runtimeName)),
        ]))], sync: false)
        for name in harness.declaredInject {
            let methods = JSON.object(services[name]!.rutisMethods().keys.sorted().map { ($0, .string("sync")) })
            _ = try await session.call(target: "", method: "hosts.provide", args: [.data(.string(name)), .data(methods)], sync: false)
        }
        return harness
    }

    /// The extra keys of `config` for row `row`, on standard error as the
    /// host's loader writes them (`<row>: config: …`), once per row and
    /// config value (config schema design §3.2).
    func warn(row: String, _ config: Value) {
        guard case .object = plugin.schema else { return }
        let json: JSON?
        switch config {
        case .undefined: json = nil
        case let .data(data): json = data == .null ? nil : data
        default: return
        }
        let warnings = ConfigSchema.check(plugin.schema, json).warnings
        let seen = "\(row)\u{0}\(json.map { String(decoding: JSONWriter.write($0), as: UTF8.self) } ?? "")"
        guard !warnings.isEmpty, lock.withLock({ warned.insert(seen).inserted }) else { return }
        for warning in warnings {
            FileHandle.standardError.write(Data("\(row): config: \(warning.message)\n".utf8))
        }
    }

    func load(config: Any?, row: String, scope: [Rutis.Advanced.Scope]) async throws -> Loaded {
        let exports: Value = try lock.withLock {
            if scopes[row] != nil { throw RutisError(name: "Error", message: "RutisTest: a row \(row) is already loaded") }
            scopes[row] = scope
            rows.append(row)
            // The first row exports what the plugin declares.
            guard rows.count == 1 else { return .data(.object([])) }
            return .data(.object(declaredProvides.sorted { $0.key < $1.key }.map { name, kinds in
                (name, .object(kinds.sorted { $0.key < $1.key }.map { ($0.key, .string($0.value)) }))
            }))
        }
        let configValue = config.map { try? ValueCodec.encodeAny($0) } ?? .null
        warn(row: row, configValue ?? .null)
        let placed = Value.data(.object([("scope", .array(scope.map { .object([("id", .string($0.id)), ("group", .string($0.group))]) }))]))
        do {
            _ = try await session.call(target: "", method: "rows.load", args: [
                .data(.string(row)), .data(.string(plugin.name)), configValue ?? .null, .data(.array([])), .data(.array([])), exports, placed,
            ], sync: false)
        } catch {
            forget(row)
            throw error
        }
        return Loaded(harness: self, row: row, scope: scope)
    }

    /// Withdraws a row's listeners, as a host does when it unloads the row.
    func forget(_ row: String) {
        lock.withLock {
            withdrawRow(row)
            scopes[row] = nil
            rows.removeAll { $0 == row }
        }
    }

    /// `lock` is held.
    func withdrawRow(_ row: String) {
        entries.removeAll { entry in
            guard entry.local == nil, entry.row == row else { return false }
            entry.withdrawn = true
            return true
        }
    }

    /// Takes the runtime's registrations in frame order (§4.1).
    func arrive(_ method: String, _ args: [Value]) -> (() async throws -> Value)? {
        switch method {
        case "events.on":
            do {
                try register(args.first ?? .undefined)
                return { .undefined }
            } catch {
                return { throw error }
            }
        case "events.off":
            let ids = (try? ValueCodec.decode([String].self, from: args.first ?? .undefined)) ?? []
            lock.withLock {
                entries.removeAll { entry in
                    guard entry.local == nil, ids.contains(entry.id) else { return false }
                    entry.withdrawn = true
                    return true
                }
            }
            return { .undefined }
        default:
            return nil
        }
    }

    private func register(_ registration: Value) throws {
        let name = registration["name"]?.string ?? ""
        if name.hasPrefix("internal/") {
            throw RutisError(name: RutisError.Name.bindingError, message: "event \(name) is internal to its runtime")
        }
        let row = registration["row"]?.string ?? ""
        try lock.withLock {
            guard scopes[row] != nil else {
                throw RutisError(name: RutisError.Name.bindingError, message: "row \(row) is not loaded in this runtime")
            }
            let entry = BusEntry(harness: self, id: registration["id"]?.string ?? "", name: name, row: row,
                                 plugin: registration["plugin"]?.string ?? "",
                                 global: registration["global"].map { if case .data(.bool(true)) = $0 { true } else { false } } ?? false,
                                 local: nil)
            let prepend = registration["prepend"].map { if case .data(.bool(true)) = $0 { true } else { false } } ?? false
            add(entry, prepend: prepend)
        }
    }

    /// Puts an entry on the bus; `lock` is held.
    func add(_ entry: BusEntry, prepend: Bool) {
        if prepend { entries.insert(entry, at: 0) } else { entries.append(entry) }
    }

    func dispatch(_ target: String, _ method: String, _ args: [Value]) async throws -> Value {
        if target.isEmpty, method == "service" {
            let id = args.first?.string ?? ""
            let handle = args.count > 1 ? args[1].string : nil
            let methods = args.count > 3 ? args[3]["methods"] : nil
            lock.withLock {
                if let handle {
                    handles[id] = handle
                    if let methods, let kinds = try? ValueCodec.decode([String: String].self, from: methods) { announced[id] = kinds }
                } else {
                    handles[id] = nil
                }
            }
            return .undefined
        }
        if target.isEmpty, method == "events.dispatch" { return try await dispatchFromRuntime(args.first ?? .undefined) }
        guard target.hasPrefix("host:") else {
            throw RutisError(name: "Error", message: "the test serves no \(target) \(method)")
        }
        let name = String(target.dropFirst(5))
        guard let service = services[name] else { throw RutisError(name: "Error", message: "the test gives no service \(name)") }
        let table = service.rutisMethods()
        let callee = Refusal.Callee(service: name, method: method)
        guard let body = table[method] else { throw Refusal.noMethod(callee, table.keys) }
        return try await body(Rutis._Generated.Arguments(args, callee: callee)).value
    }

    /// A dispatch of `name` (§5.1); delivery is by name.
    func snapshot(_ name: String, own: Bool) -> Dispatch {
        lock.withLock {
            dispatches += 1
            let id = "d\(dispatches)"
            let chosen = entries.filter { $0.name == name }
                .map { Invocation($0, dispatchID: id, own: own && $0.local == nil) }
            return Dispatch(name: name, entries: chosen)
        }
    }

    static func run(_ dispatch: Dispatch, _ mode: Mode, _ args: [Value], terminal: (([Value]) async throws -> Value)?) async throws -> Value {
        switch mode {
        case .emit:
            try await dispatch.emit(args)
            return .undefined
        case .parallel:
            try await dispatch.parallel(args)
            return .undefined
        case .waterfall, .waterfallSync:
            return try await dispatch.waterfall(mode, args, terminal: terminal)
        default:
            return try await dispatch.serial(mode, args)
        }
    }

    /// Serves the runtime's `events.dispatch` (§5.3).
    func dispatchFromRuntime(_ record: Value) async throws -> Value {
        guard let row = record["row"]?.string, let name = record["name"]?.string, let modeText = record["mode"]?.string,
              let mode = Mode(rawValue: modeText), mode != .waterfallSync else {
            throw RutisError(name: "Error", message: "events.dispatch: a row, a name and a mode")
        }
        try Events.checkEmit(name)
        let args = record["args"]?.items ?? []
        guard lock.withLock({ scopes[row] != nil }) else {
            throw RutisError(name: RutisError.Name.bindingError, message: "row \(row) is not loaded in this runtime")
        }
        var dispatch = snapshot(name, own: true)
        if case let .remoteFunction(filter) = record["filter"] ?? .undefined {
            // One call of the emitter's selector, with every candidate the
            // filter applies to (§6).
            let candidates = dispatch.entries.map(\.info).filter { !$0.global }
            do {
                let answer = try await settle(try await filter.call([try ValueCodec.encode(candidates)], sync: mode == .bail))
                let answers = (try? ValueCodec.decode([Bool].self, from: answer)) ?? []
                guard answers.count == candidates.count else {
                    throw RutisError(name: RutisError.Name.bindingError,
                                     message: "filter of \(name) returned \(answers.count) answers for \(candidates.count) listeners")
                }
                let asked = Counter()
                dispatch.keep = { _ in answers[asked.next()] }
            } catch {
                if mode == .emit {
                    Events.report("rutis: filter of \(name) from row \(row) failed: \(Events.failure(error))")
                    return .undefined
                }
                throw error
            }
        }
        var terminal: (([Value]) async throws -> Value)?
        if case let .remoteFunction(remote) = record["terminal"] ?? .undefined {
            terminal = { args in try await settle(try await remote.call(args, sync: false)) }
        }
        return try await Harness.run(dispatch, mode, args, terminal: terminal)
    }

    func close() async {
        let rows = lock.withLock { self.rows }
        for row in rows.reversed() { try? await unload(row) }
        lock.withLock { closed = true }
        stopHearing?()
        session.close()
        session.channel.close()
        await served?.value
    }

    func unload(_ row: String) async throws {
        // The host withdraws the row's listeners as it unloads it (§7.1).
        forget(row)
        if lock.withLock({ closed }) { return }
        _ = try await session.call(target: "", method: "rows.unload", args: [.data(.string(row))], sync: false)
    }

    /// Tells the runtime of a change of the names with listeners outside
    /// it, and waits until it applied it.
    func elsewhere(add: [String] = [], remove: [String] = []) async {
        if lock.withLock({ closed }) { return }
        _ = try? await session.call(target: "", method: "events.elsewhere", args: [.data(.object([
            ("add", .array(add.map(JSON.string))), ("remove", .array(remove.map(JSON.string))),
        ]))], sync: false)
    }
}

final class Counter: @unchecked Sendable {
    private let lock = Lock()
    private var value = 0
    func next() -> Int {
        lock.withLock {
            defer { value += 1 }
            return value
        }
    }
}

/// A row of a plugin loaded for a test.
public final class Loaded: @unchecked Sendable {
    let harness: Harness
    /// The row's key.
    public let row: String
    /// The row's scope chain.
    public let scope: [Rutis.Advanced.Scope]

    init(harness: Harness, row: String, scope: [Rutis.Advanced.Scope]) {
        self.harness = harness
        self.row = row
        self.scope = scope
    }

    /// Loads another row of the same plugin into the same runtime.
    public func load(config: Any? = nil, row: String? = nil, scope: [Rutis.Advanced.Scope] = []) async throws -> Loaded {
        let key = row ?? harness.lock.withLock { "test-\(harness.rows.count + 1)" }
        return try await harness.load(config: config, row: key, scope: scope)
    }

    /// The service `name` the plugin provides, as rutis sees it: its
    /// declared methods, or, undeclared, the methods of its type, all
    /// `async`. Waits for it (the announcement may be on its way).
    public func service(_ name: String) async throws -> Service {
        let deadline = Date().addingTimeInterval(10)
        while true {
            if let handle = harness.lock.withLock({ harness.handles[name] }) {
                let kinds = harness.declaredProvides[name] ?? harness.lock.withLock { harness.announced[name] } ?? [:]
                return Service(session: harness.session, handle: handle, name: name, methods: kinds)
            }
            if Date() > deadline { throw RutisError(name: "Error", message: "RutisTest: the plugin does not provide \(name)") }
            try await Task.sleep(nanoseconds: 5_000_000)
        }
    }

    /// The services the plugin provides now.
    public var provided: [String] { harness.lock.withLock { Array(harness.handles.keys) } }

    /// What the runtime reported so far (§5.6): one line per failure of a
    /// listener nobody waited for, as the runtime writes it on stderr.
    public var reported: [String] { harness.lock.withLock { harness.reported } }

    /// Unloads the row: the runtime cancels its running listeners and waits
    /// for them first, up to RUTIS_SHUTDOWN_TIMEOUT, as under a host.
    public func unload() async throws { try await harness.unload(row) }

    /// Changes the row's config, as a host's rows.update: its listeners
    /// register again, after every existing one.
    public func update(config: Any?) async throws {
        harness.lock.withLock { harness.withdrawRow(row) }
        let value = try config.map { try ValueCodec.encodeAny($0) } ?? .null
        harness.warn(row: row, value)
        _ = try await harness.session.call(target: "", method: "rows.update", args: [.data(.string(row)), value], sync: false)
    }

    /// Unloads every row, latest first, and ends the runtime.
    public func close() async { await harness.close() }

    // ── Events ───────────────────────────────────────────────────────

    /// Listens to `name` as the host does, at the root, until the returned
    /// function is called: the plugin's emissions of `name` reach the
    /// listener through the test side, as through rutis.
    @discardableResult
    public func on<each A, R>(_ name: String, prepend: Bool = false, global: Bool = false,
                              _ listener: @escaping (repeat each A) async throws -> R) async -> () async -> Void
    {
        let entry = BusEntry(harness: harness, id: "", name: name, row: nil, plugin: "rutistest", global: global,
                             local: listenerBody(name, listener))
        let first: Bool = harness.lock.withLock {
            let first = !harness.entries.contains { $0.local != nil && $0.name == name }
            harness.add(entry, prepend: prepend)
            return first
        }
        if first { await harness.elsewhere(add: [name]) }
        return { [harness] in
            let last: Bool = harness.lock.withLock {
                entry.withdrawn = true
                harness.entries.removeAll { $0 === entry }
                return !harness.entries.contains { $0.local != nil && $0.name == name }
            }
            if last { await harness.elsewhere(remove: [name]) }
        }
    }

    func dispatch(_ name: String, _ mode: Mode, _ args: [Any?], keep: ((Rutis.Advanced.Listener) async throws -> Bool)?,
                  terminal: (([Value]) async throws -> Value)?) async throws -> Value
    {
        try Events.checkEmit(name)
        let values = try args.map { try ValueCodec.encodeAny($0 as Any) }
        var dispatch = harness.snapshot(name, own: false)
        dispatch.keep = keep
        return try await Harness.run(dispatch, mode, values, terminal: terminal)
    }

    /// Starts every listener of `name` on the bus.
    public func emit(_ name: String, _ args: Any?...) async throws { _ = try await dispatch(name, .emit, args, keep: nil, terminal: nil) }

    /// Runs every listener of `name` and waits for all; failures are an
    /// AggregateError in registration order.
    public func parallel(_ name: String, _ args: Any?...) async throws {
        _ = try await dispatch(name, .parallel, args, keep: nil, terminal: nil)
    }

    /// The first result of the listeners of `name`, one at a time.
    public func serial<R>(_ name: String, _ args: Any?..., as _: R.Type) async throws -> R? {
        try Loaded.result(try await dispatch(name, .serial, args, keep: nil, terminal: nil))
    }

    /// The outermost listener's value; `terminal` gets the arguments as an
    /// array.
    public func waterfall<R>(_ name: String, _ args: Any?..., as _: R.Type, terminal: (([Any?]) async throws -> Any?)? = nil) async throws -> R? {
        let last = terminal.map { body in { (values: [Value]) in try ValueCodec.encodeAny(try await body(values.map(ValueCodec.native)) as Any) } }
        return try Loaded.result(try await dispatch(name, .waterfall, args, keep: nil, terminal: last))
    }

    /// `serial` as a synchronous call.
    public func bail<R>(_ name: String, _ args: Any?..., as _: R.Type) async throws -> R? {
        try Loaded.result(try await dispatch(name, .bail, args, keep: nil, terminal: nil))
    }

    static func result<R>(_ value: Value) throws -> R? {
        if value.isNullish { return nil }
        return try ValueCodec.decode(R.self, from: value)
    }
}

/// A service the plugin provides, called across the session.
public final class Service: @unchecked Sendable {
    let session: Peer
    let handle: String
    public let name: String
    /// Its methods, each "sync" or "async".
    public let methods: [String: String]

    init(session: Peer, handle: String, name: String, methods: [String: String]) {
        self.session = session
        self.handle = handle
        self.name = name
        self.methods = methods
    }

    /// The result as the session carries it.
    package func callValue(_ method: String, _ args: [Value]) async throws -> Value {
        guard let kind = methods[method] else { throw Refusal.noMethod(Refusal.Callee(service: name, method: method), methods.keys) }
        return try await settle(try await session.call(target: handle, method: method, args: args, sync: kind == "sync"))
    }

    /// Calls `method` with `args` and decodes its result into `R`.
    public func call<R>(_ method: String, _ args: Any?..., as _: R.Type = R.self) async throws -> R {
        let values = try args.map { try ValueCodec.encodeAny($0 as Any) }
        return try ValueCodec.decode(R.self, from: try await callValue(method, values))
    }
}
