import Foundation

// The runtime: the rows one controller loads into this process, the
// services they provide and use, their listeners, and the control
// operations of the runtime contract (§3, §4).

/// What this runtime supports of the runtime contract. `rows.announce`:
/// services rows provide without declaring them are announced with their
/// methods, to a host whose mount says `announce`. `plugin-values`: a plugin
/// definition this runtime handed over comes back to be described
/// (`plugins.read`) and loaded as a row's entry
/// (docs/design-plugin-values-2026-10-25.md §5).
let runtimeFeatures = ["rows.v2", "hosts", "leaf", "scopes", "rows.announce", "plugin-values"]

/// The Swift compiler this SDK was built with (the mount's `engine`).
let swiftVersion: String = {
    #if compiler(>=6.5)
    return "6.5"
    #elseif compiler(>=6.4)
    return "6.4"
    #elseif compiler(>=6.3)
    return "6.3"
    #elseif compiler(>=6.2)
    return "6.2"
    #elseif compiler(>=6.1)
    return "6.1"
    #elseif compiler(>=6.0)
    return "6.0"
    #else
    return "5.x"
    #endif
}()

/// How the service `name` is identified in the scope `label`: the name
/// outside any scope, the name, a NUL and the label inside one. Labels are
/// opaque: never parsed or compared, only checked.
func scopedID(_ name: String, _ label: String?) throws -> String {
    if name.contains("\0") || (label?.contains("\0") ?? false) {
        throw RutisError(name: "Error", message: "service \(name) or its scope label contains NUL")
    }
    guard let label else { return name }
    if label.isEmpty { throw RutisError(name: "Error", message: "service \(name) has an empty scope label") }
    return name + "\0" + label
}

func handleOf(_ id: String, _ generation: Int) -> String {
    if generation == 1 { return id }
    return id + (id.contains("\0") ? "\0" : "#") + String(generation)
}

func printable(_ id: String) -> String { id.replacingOccurrences(of: "\0", with: "@") }

/// Whether two service values are the same object.
func same(_ a: RutisExportable?, _ b: RutisExportable?) -> Bool {
    guard let a, let b else { return a == nil && b == nil }
    guard type(of: a) is AnyClass, type(of: b) is AnyClass else { return false }
    return (a as AnyObject) === (b as AnyObject)
}

final class Provided {
    let row: String
    let value: RutisExportable
    let shape: Rutis.Shape?
    init(row: String, value: RutisExportable, shape: Rutis.Shape?) {
        self.row = row
        self.value = value
        self.shape = shape
    }
}

final class Slot {
    let row: String
    let name: String
    var methods: Set<String>?
    var object: RutisExportable?
    var present = false
    var handle: String?
    var generation = 0
    /// Not declared: announced, with the methods of each object.
    let announced: Bool

    init(row: String, name: String, methods: Set<String>?, announced: Bool) {
        self.row = row
        self.name = name
        self.methods = methods
        self.announced = announced
    }
}

final class HandleEntry {
    let id: String
    /// The service's name, as the plugin wrote it.
    let name: String
    let object: RutisExportable
    let methods: Set<String>?
    var current = true
    var released = false
    init(id: String, name: String, object: RutisExportable, methods: Set<String>?) {
        self.id = id
        self.name = name
        self.object = object
        self.methods = methods
    }
}

/// A loaded row.
final class Row: @unchecked Sendable {
    let key: String
    /// The row's id in the configuration (rows.load's `id`; the key when
    /// not given): descriptors and reported lines call the row so.
    let name: String
    let entry: String
    let definition: Rutis.Definition
    let config: Value
    let exports: [String: [String: String]]
    let isolate: [String: String]
    let scope: [Rutis.Scope]
    /// Its runtime's: where its lines go, how long its listeners may take.
    let settings: Events.Settings
    var cleanups: [() async throws -> Void] = []
    var provided: [String] = []
    var announced: [String] = []
    /// What the host has yet to answer while the row loads (announcements,
    /// registrations): each waits for its answer, and an error fails the
    /// load. Nil once it loaded.
    var pending: [() async -> Error?] = []
    var loading = true
    /// A provide that failed while the row loads: it fails the load.
    var failure: Error?
    var order = 0
    var unloading = false

    // The invocations of its listeners running, and its spawned tasks: an
    // unload cancels them and waits for them.
    private let lock = Lock()
    private var listening = 0
    private var idle: [Event] = []
    private var running: [Int: Task<Void, Never>] = [:]
    private var spawned: [Int: Task<Void, Never>] = [:]
    private var nextTask = 0
    private var cancelled = false
    /// What waits on a task of the row's that the unload lets go (`held`).
    private var letGo: [Int: Event] = [:]

    init(key: String, name: String, entry: String, definition: Rutis.Definition, config: Value,
         exports: [String: [String: String]], isolate: [String: String], scope: [Rutis.Scope],
         settings: Events.Settings = Events.Settings())
    {
        self.settings = settings
        self.key = key
        self.name = name
        self.entry = entry
        self.definition = definition
        self.config = config
        self.exports = exports
        self.isolate = isolate
        self.scope = scope
    }

    /// The id this row uses for the service `name`: its own `isolate`
    /// gives the label (§6.2, caller's row).
    func id(_ name: String) -> String {
        (try? scopedID(name, isolate[name])) ?? name
    }

    var isCancelled: Bool { lock.withLock { cancelled } }

    /// Counts an invocation of one of its listeners, unless it unloads.
    func reserve() -> Bool {
        lock.withLock {
            if cancelled { return false }
            listening += 1
            return true
        }
    }

    func release() {
        let events: [Event] = lock.withLock {
            listening -= 1
            guard listening == 0 else { return [] }
            defer { idle = [] }
            return idle
        }
        for event in events { event.fire() }
    }

    /// Runs `body` in a Task of the row's: cancelled when the row unloads
    /// or the caller gives up.
    func track(_ body: @escaping () async throws -> Value) async throws -> Value {
        let result = ResultBox()
        let task: Task<Void, Never> = Task {
            do {
                result.set(.success(try await body()))
            } catch {
                result.set(.failure(error))
            }
        }
        let id: Int = lock.withLock {
            nextTask += 1
            running[nextTask] = task
            if cancelled { task.cancel() }
            return nextTask
        }
        await withTaskCancellationHandler {
            await task.value
        } onCancel: {
            task.cancel()
        }
        lock.withLock { running[id] = nil }
        return try result.get()
    }

    /// Runs `body` in a Task of the row's, cancelled when the row unloads,
    /// and returns once it ended or the row unloads, whichever comes
    /// first: a body that does not heed cancellation (a filter that never
    /// answers) holds neither the caller nor the unload. Nothing runs when
    /// the row unloads already.
    func held(_ body: @escaping () async -> Void) async {
        let done = Event()
        let id: Int? = lock.withLock {
            if cancelled { return nil }
            nextTask += 1
            running[nextTask] = Task {
                await body()
                done.fire()
            }
            letGo[nextTask] = done
            return nextTask
        }
        guard let id else { return }
        await done.wait()
        lock.withLock {
            running[id] = nil
            letGo[id] = nil
        }
    }

    /// Runs `body` until the row unloads (ctx.spawn).
    func spawn(_ body: @escaping () async throws -> Void) {
        lock.lock()
        nextTask += 1
        let id = nextTask
        let name = self.name
        let task = Task {
            do {
                try await body()
            } catch is CancellationError {
            } catch {
                self.settings.log("rutis: a task of row \(name) failed: \(Events.failure(error))")
            }
        }
        spawned[id] = task
        if cancelled { task.cancel() }
        lock.unlock()
        Task {
            await task.value
            self.lock.withLock { _ = self.spawned.removeValue(forKey: id) }
        }
    }

    /// Cancels its listeners and tasks; waits for the listeners, then for
    /// its tasks (ctx.spawn), within one shutdown deadline, reporting what
    /// was still running past it (§7.1): a task that ignores cancellation
    /// does not hold the unload.
    func stop() async {
        let budget = settings.shutdownTimeout
        let started = DispatchTime.now().uptimeNanoseconds
        let (tasks, waiting, released): ([Task<Void, Never>], Event?, [Event]) = lock.withLock {
            cancelled = true
            let tasks = Array(running.values) + Array(spawned.values)
            let released = Array(letGo.values)
            guard listening > 0 else { return (tasks, nil, released) }
            let event = Event()
            idle.append(event)
            return (tasks, event, released)
        }
        for task in tasks { task.cancel() }
        for event in released { event.fire() }
        if let waiting {
            if await !waiting.wait(seconds: budget) {
                Events.report("rutis: listeners of row \(name) still running after \(Events.seconds(budget))s; unloading anyway", to: settings.log)
                return
            }
        }
        let rest = lock.withLock { Array(spawned.values) }
        guard !rest.isEmpty else { return }
        let ended = Event()
        Task {
            for task in rest { await task.value }
            ended.fire()
        }
        let spent = Double(DispatchTime.now().uptimeNanoseconds - started) / 1e9
        if await !ended.wait(seconds: budget - spent) {
            Events.report("rutis: tasks of row \(name) still running after \(Events.seconds(budget))s; unloading anyway", to: settings.log)
        }
    }
}

final class ResultBox: @unchecked Sendable {
    private let lock = Lock()
    private var result: Result<Value, Error> = .failure(CancellationError())
    func set(_ value: Result<Value, Error>) { lock.withLock { result = value } }
    func get() throws -> Value { try lock.withLock { result }.get() }
}

/// The rows one controller loads into this process.
final class Runtime: @unchecked Sendable {
    let definitions: [String: Rutis.Definition]
    var session: Peer?
    let project: String
    let settings: Events.Settings

    let lock = Lock()
    var rows: [String: Row] = [:]
    var services: [String: Provided] = [:]
    var hosts: [String: [String: String]] = [:]
    var slots: [String: Slot] = [:]
    var handles: [String: HandleEntry] = [:]
    var version = 0
    var loaded = 0
    var closing = false
    // The listeners of each name, in position order, and by id.
    var listeners: [String: [LocalListener]] = [:]
    var byID: [String: LocalListener] = [:]
    var listenerSequence = 0
    var first = 0
    var last = 0
    // The names and prefixes with listeners outside this process
    // (events.elsewhere), and the emits through the host not answered yet.
    var elsewhere = Set<String>()
    var elsewherePrefixes = Set<String>()
    var hostEmits: [String: Int] = [:]
    /// Per name, the emits waiting behind a filtered one (§5.5): a filter
    /// may wait, and a later emit of the name must not start before it.
    var held: [String: (queue: SerialQueue, count: Int)] = [:]
    /// What the host calls this runtime (mount `runtime`), for descriptors.
    var runtimeName: String?
    /// The host takes announced services (mount `announce`).
    var announce = false
    /// Mounted by a host; one without `announce` is older than 0.12.
    var mounted = false
    var told = Set<String>()

    // Frames for the host, queued under `lock` in the order they are taken
    // and sent by flush once `lock` is released.
    private let outLock = Lock()
    private var outbox: [() -> Void] = []
    private let sendLock = Lock()

    init(_ definitions: [Rutis.Definition], project: String, settings: Events.Settings = Events.Settings()) {
        self.settings = settings
        var byName: [String: Rutis.Definition] = [:]
        for definition in definitions { byName[definition.name] = definition }
        self.definitions = byName
        self.project = project
    }

    /// Queues `send` behind every frame queued before; `lock` is held.
    func post(_ send: @escaping () -> Void) {
        outLock.withLock { outbox.append(send) }
    }

    /// Sends the queued frames in order; `lock` is not held.
    func flush() {
        sendLock.lock()
        defer { sendLock.unlock() }
        while true {
            let next: (() -> Void)? = outLock.withLock {
                outbox.isEmpty ? nil : outbox.removeFirst()
            }
            guard let next else { return }
            next()
        }
    }

    var live: Bool { session != nil && !closing }

    func definition(_ entry: String) throws -> Rutis.Definition {
        guard let definition = definitions[entry] else {
            throw RutisError(name: "NotFound", message: "plugin \(entry) is not in this binary; it has: \(definitions.keys.sorted().joined(separator: ", "))")
        }
        return try supported(definition)
    }

    /// `definition`, unless it needs a newer plugin API than this runtime's.
    func supported(_ definition: Rutis.Definition) throws -> Rutis.Definition {
        if definition.api > Rutis.pluginAPI {
            throw RutisError(name: "Error", message: "plugin \(definition.name) needs plugin API \(definition.api); this runtime supports \(Rutis.pluginAPI)")
        }
        return definition
    }

    // ── Services ─────────────────────────────────────────────────────

    /// Reports the object now in an exported slot under a new handle;
    /// `lock` is held, the report queued.
    func refresh(_ id: String) {
        guard let slot = slots[id] else { return }
        var current: RutisExportable?
        var present = false
        if let provided = services[id], provided.row == slot.row {
            current = provided.value
            present = true
        }
        if present == slot.present, same(current, slot.object) { return }
        if let handle = slot.handle { retire(handle) }
        slot.object = current
        slot.present = present
        slot.handle = nil
        if present, let current {
            slot.generation += 1
            let handle = handleOf(id, slot.generation)
            slot.handle = handle
            if slot.announced { slot.methods = Set(current.rutisMethods().keys) }
            handles[handle] = HandleEntry(id: id, name: slot.name, object: current, methods: slot.methods)
        }
        version += 1
        guard live, let session else { return }
        let handle: Value = slot.handle.map { .data(.string($0)) } ?? .null
        let version = Value.data(.int(self.version))
        guard slot.announced else {
            let args: [Value] = [.data(.string(id)), handle, version]
            post { session.notify(target: "", method: "service", args: args) }
            return
        }
        // An announced service says whose it is and what it offers, and a
        // row loading answers once rutis has heard it.
        let methods = JSON.object((slot.methods ?? []).sorted().map { ($0, .string("async")) })
        let args: [Value] = [.data(.string(id)), handle, version,
                             .data(.object([("row", .string(slot.row)), ("methods", methods)]))]
        let answered = Event()
        post {
            if let call = try? session.send(target: "", method: "service", args: args, sync: false) {
                Task.detached {
                    _ = try? await call.value()
                    answered.fire()
                }
            } else {
                answered.fire()
            }
        }
        if let row = rows[slot.row], row.loading {
            row.pending.append {
                await answered.wait()
                return nil
            }
        }
    }

    func retire(_ handle: String) {
        guard let entry = handles[handle] else { return }
        entry.current = false
        if entry.released { handles[handle] = nil }
    }

    /// Provides `value` as the service `name` of `row` until the returned
    /// function is called or the row unloads.
    func provide(_ row: Row, _ name: String, _ value: RutisExportable) -> () -> Void {
        let id = row.id(name)
        let shape = row.definition.provides[name]
        lock.lock()
        // A task of an unloaded row providing: refused, or the service would
        // outlive its row and the row's next load find it taken.
        guard rows[row.key] === row else {
            lock.unlock()
            fail(row, RutisError(name: "Error", message: "row \(row.name) is not active"))
            return {}
        }
        if let existing = services[id] {
            let owner = rows[existing.row]?.name ?? existing.row
            lock.unlock()
            fail(row, RutisError(name: "Error", message: "\(name) is already provided by \(owner)"))
            return {}
        }
        services[id] = Provided(row: row.key, value: value, shape: shape)
        row.provided.append(id)
        // Not declared: announced, with its methods; a host older than 0.12
        // does not take it, and the plugin is told, once per name.
        if slots[id] == nil, !name.contains("#") {
            if announce {
                slots[id] = Slot(row: row.key, name: name, methods: nil, announced: true)
                row.announced.append(id)
            } else if mounted, !told.contains(name) {
                told.insert(name)
                settings.log("rutis: service \(name) is not declared in provides, and this host (older than 0.12) does not take undeclared services; declare it or upgrade rutis-host")
            }
        }
        refresh(id)
        lock.unlock()
        flush()
        return { [weak self] in
            guard let self else { return }
            lock.lock()
            if let existing = services[id], existing.row == row.key, same(existing.value, value) {
                services[id] = nil
                row.provided.removeAll { $0 == id }
                refresh(id)
            }
            lock.unlock()
            flush()
        }
    }

    /// A failure of a plugin's registration: fails the row's load, or is
    /// reported once it loaded.
    func fail(_ row: Row, _ error: Error) {
        let loading: Bool = lock.withLock {
            if row.loading, row.failure == nil { row.failure = error }
            return row.loading
        }
        if !loading {
            settings.log("rutis: row \(row.name): \(Events.failure(error))")
        }
    }

    // ── Rows ─────────────────────────────────────────────────────────

    /// Loads the row `key` running the plugin `entry` names in this binary.
    func load(key: String, name: String?, entry: String, config: Value, exports: [String: [String: String]],
              isolate: [String: String], scope: [Rutis.Scope]) async throws
    {
        try await load(key: key, name: name, entry: entry, definition: try definition(entry), config: config,
                       exports: exports, isolate: isolate, scope: scope)
    }

    /// Loads the row `key` running `definition`: the plugin `entry` names in
    /// this binary, or a plugin definition this runtime handed over (a
    /// copy, `entry` its name).
    func load(key: String, name: String?, entry: String, definition: Rutis.Definition, config: Value,
              exports: [String: [String: String]], isolate: [String: String], scope: [Rutis.Scope]) async throws
    {
        let name = (name?.isEmpty ?? true) ? key : name!
        let row = Row(key: key, name: name, entry: entry, definition: definition, config: config, exports: exports,
                      isolate: isolate, scope: scope, settings: settings)
        try lock.withLock {
            if rows[key] != nil { throw RutisError(name: "Error", message: "row \(key) is already loaded") }
            for (service, label) in isolate { _ = try scopedID(service, label) }
            for service in exports.keys {
                if service.contains("#") { throw RutisError(name: "Error", message: "service name \(service) cannot be projected") }
                if let existing = slots[row.id(service)] {
                    throw RutisError(name: "Error", message: "\(service) is already provided by \(rows[existing.row]?.name ?? existing.row)")
                }
            }
            loaded += 1
            row.order = loaded
            rows[key] = row
            for (service, methods) in exports {
                slots[row.id(service)] = Slot(row: key, name: service, methods: Set(methods.keys), announced: false)
            }
        }
        do {
            try await definition.apply(Rutis.Ctx(runtime: self, row: row), config)
            if let failure = lock.withLock({ row.failure }) { throw failure }
        } catch {
            try? await unload(key)
            throw error
        }
        let pending: [() async -> Error?] = lock.withLock {
            for service in exports.keys { refresh(row.id(service)) }
            defer {
                row.pending = []
                row.loading = false
            }
            return row.pending
        }
        flush()
        // rutis has published the announced services and registered the
        // row's listeners before it hears that the row loaded; a refused
        // listener fails the load (§4.4).
        var refused: Error?
        for wait in pending {
            if let error = await wait(), refused == nil { refused = error }
        }
        if let refused {
            try? await unload(key)
            throw refused
        }
    }

    func unload(_ key: String) async throws {
        let row: Row? = lock.withLock {
            guard let row = rows[key] else { return nil }
            rows[key] = nil
            row.unloading = true
            forgetListeners(row)
            // Withdrawals first: rutis hears them before the plugin goes.
            for id in row.provided {
                if let existing = services[id], existing.row == key {
                    services[id] = nil
                    refresh(id)
                }
            }
            row.provided = []
            for id in row.announced + row.exports.keys.map(row.id) {
                if let slot = slots[id], slot.row == key {
                    slots[id] = nil
                    if let handle = slot.handle { retire(handle) }
                }
            }
            row.announced = []
            return row
        }
        guard let row else { return }
        // Taken and cleared before awaiting: a cleanup that captures the
        // row's context would otherwise form Row -> cleanup -> Ctx -> Row,
        // and the row would outlive its unload even when a cleanup hangs.
        let cleanups: [() async throws -> Void] = lock.withLock {
            let cleanups = row.cleanups
            row.cleanups = []
            return cleanups
        }
        flush()
        await row.stop()
        var errors: [Error] = []
        for cleanup in cleanups.reversed() {
            do { try await cleanup() } catch { errors.append(error) }
        }
        if let first = errors.first {
            if errors.count == 1 { throw first }
            throw RutisError(name: RutisError.Name.aggregateError, message: "\(errors.count) cleanups of row \(row.name) failed",
                             errors: errors.map(asRutisError))
        }
    }

    func update(_ key: String, config: Value) async throws {
        guard let row = lock.withLock({ rows[key] }) else {
            throw RutisError(name: "Error", message: "row \(key) is not loaded")
        }
        // Leaf plugins have no volatile fields: a new config restarts the row.
        try await unload(key)
        try await load(key: key, name: row.name, entry: row.entry, definition: row.definition, config: config, exports: row.exports,
                       isolate: row.isolate, scope: row.scope)
    }

    /// Unloads every row, latest first. Whether every cleanup finished
    /// within `seconds` (nil: no deadline).
    @discardableResult
    func dispose(within seconds: Double? = nil) async -> Bool {
        let ordered = lock.withLock { rows.values.sorted { $0.order < $1.order } }
        for row in ordered.reversed() {
            let done = Event()
            Task.detached { [self] in
                try? await unload(row.key)
                done.fire()
            }
            if let seconds {
                if await !done.wait(seconds: seconds) {
                    settings.log("rutis: gave up the cleanups of row \(row.name) and those after it: they did not finish within \(Events.seconds(seconds))s")
                    return false
                }
            } else {
                await done.wait()
            }
        }
        return true
    }

    // ── Dispatch ─────────────────────────────────────────────────────

    func dispatch(_ target: String, _ method: String, _ args: [Value]) async throws -> Value {
        if lock.withLock({ closing }) { throw RutisError(name: "Error", message: "runtime is closing") }
        if target.isEmpty { return try await control(method, args) }
        guard let entry = lock.withLock({ handles[target] }) else {
            throw RutisError(name: "Error", message: "unknown or released service object \(printable(target))")
        }
        let callee = Refusal.Callee(service: entry.name, method: method)
        let table = entry.object.rutisMethods()
        guard let body = table[method], entry.methods?.contains(method) ?? true else {
            throw Refusal.noMethod(callee, entry.methods ?? Set(table.keys))
        }
        return try await body(Rutis.Arguments(args, callee: callee)).value
    }

    private func argument<T: Decodable>(_ args: [Value], _ index: Int, _ type: T.Type = T.self) throws -> T {
        try Rutis.Arguments(args).decode(index, as: T.self)
    }

    func control(_ method: String, _ args: [Value]) async throws -> Value {
        switch method {
        case "mount":
            let mount = args.first ?? .undefined
            lock.withLock {
                announce = mount["announce"].map { if case .data(.bool(true)) = $0 { return true } else { return false } } ?? false
                runtimeName = mount["runtime"]?.string
                mounted = true
            }
            return .data(.object([
                ("services", .object([])),
                ("features", .array(runtimeFeatures.map(JSON.string))),
                ("plugins", .object(definitions.keys.sorted().map { ($0, definitions[$0]!.described) })),
                ("implementation", .object([("name", .string(Rutis.implementation)), ("version", .string(Rutis.sdkVersion))])),
                ("engine", .object([("name", .string("swift")), ("version", .string(swiftVersion))])),
            ]))
        case "events.elsewhere", "events.call":
            // Served by arrive on the reading thread; here only when called
            // directly.
            guard let then = arrive(method, args) else { return .undefined }
            return try await then()
        case "dispose":
            lock.withLock { closing = true }
            await dispose()
            await session?.drain()
            return .undefined
        case "rows.load":
            let key: String = try argument(args, 0)
            // A plugin's name in this binary, or a plugin definition this
            // runtime handed over, back as the entry of a copy.
            let entry: String
            let plugin: Rutis.Definition
            if args.count > 1, case let .plugin(handed) = args[1] {
                plugin = try supported(handed)
                entry = handed.name
            } else {
                entry = try argument(args, 1)
                plugin = try definition(entry)
            }
            let config = args.count > 2 ? args[2] : .undefined
            // args[4], the row's `inject`, names services a Cordis row waits
            // for in Cordis; a leaf runtime's rows wait in rutis.
            let pairs: [[String]] = (try? argument(args, 3, [[String]]?.self)) ?? []
            var isolate: [String: String] = [:]
            for pair in pairs where pair.count == 2 { isolate[pair[0]] = pair[1] }
            let exports: [String: [String: String]] = (try argument(args, 5, [String: [String: String]]?.self)) ?? [:]
            // The seventh, from rutis 0.13: the row's scope chain and its id.
            struct Placed: Decodable {
                var scope: [Rutis.Scope]?
                var id: String?
            }
            let placed = (try argument(args, 6, Placed?.self)) ?? Placed()
            try await load(key: key, name: placed.id, entry: entry, definition: plugin, config: config, exports: exports, isolate: isolate,
                           scope: placed.scope ?? [])
            return .undefined
        case "rows.update":
            let key: String = try argument(args, 0)
            try await update(key, config: args.count > 1 ? args[1] : .undefined)
            return .undefined
        case "rows.unload":
            let key: String = try argument(args, 0)
            try await unload(key)
            return .undefined
        case "rows.schema":
            let entry: String = try argument(args, 0)
            return .data(try definition(entry).described)
        case "plugins.read":
            // What a plugin definition this runtime handed over declares, as
            // rows.schema describes one of the binary's, with its name.
            guard let value = args.first, case let .plugin(handed) = value else {
                throw RutisError(name: "TypeError", message: "not a plugin definition: expected a Rutis.define result, got \((args.first ?? .undefined).described)")
            }
            guard case let .object(fields) = try supported(handed).described else { return .undefined }
            return .data(.object([("name", .string(handed.name))] + fields))
        case "hosts.provide":
            let name: String = try argument(args, 0)
            let methods: [String: String] = (try argument(args, 1, [String: String]?.self)) ?? [:]
            let label: String? = try argument(args, 2)
            let id = try scopedID(name, label)
            try lock.withLock {
                if hosts[id] != nil { throw RutisError(name: "Error", message: "host service \(printable(id)) is already provided") }
                hosts[id] = methods
            }
            return .undefined
        case "hosts.withdraw":
            let id: String = try argument(args, 0)
            lock.withLock { hosts[id] = nil }
            return .undefined
        case "release":
            let handle: String = try argument(args, 0)
            lock.withLock {
                if let entry = handles[handle] {
                    entry.released = true
                    if !entry.current { handles[handle] = nil }
                }
            }
            return .undefined
        case "get":
            let handle: String = try argument(args, 0)
            let property: String = try argument(args, 1)
            guard let entry = lock.withLock({ handles[handle] }) else {
                throw RutisError(name: "Error", message: "unknown or released service object \(printable(handle))")
            }
            return try readProperty(of: entry.object, property)
        default:
            throw RutisError(name: "Error", message: "unknown control method \(method)")
        }
    }

    // ── Using services ───────────────────────────────────────────────

    /// The service `name` as `row` uses it: by its own id.
    func client(_ row: Row, _ name: String) throws -> (Rutis.Client, RutisExportable?) {
        guard row.definition.inject.contains(name) else {
            throw RutisError(name: RutisError.Name.bindingError,
                             message: "service \(name) is not available to this plugin, which does not declare it in inject: add '\(name)' to its inject",
                             code: "not-declared", data: ["service": name])
        }
        let id = row.id(name)
        let (local, host) = lock.withLock { (services[id], hosts[id]) }
        if let local {
            let table = local.value.rutisMethods()
            var methods: [String: String] = [:]
            if let shape = local.shape {
                for (method, kind) in shape.kinds { methods[method] = kind.rawValue }
            } else {
                for method in table.keys { methods[method] = "async" }
            }
            let client = Rutis.Client(name: name, methods: methods) { method, args, _ in
                let callee = Refusal.Callee(service: name, method: method)
                guard let body = table[method] else { throw Refusal.noMethod(callee, methods.keys) }
                return try await body(Rutis.Arguments(args, callee: callee)).value
            }
            return (client, local.value)
        }
        if let host {
            let client = Rutis.Client(name: name, methods: host) { [weak self] method, args, sync in
                guard let session = self?.session else { throw RutisError.hostGone() }
                let value = try await session.call(target: "host:" + id, method: method, args: args, sync: sync)
                return try await settle(value)
            }
            return (client, nil)
        }
        throw RutisError(name: RutisError.Name.bindingError, message: "service \(name) is not available",
                         code: "unavailable", data: ["service": name])
    }
}

extension Rutis {
    /// A service used by name: calls its methods across the session (or on
    /// the provider's object, when it is in this process).
    public final class Client: @unchecked Sendable {
        /// The service's name.
        public let name: String
        /// Its methods, each "sync" or "async".
        public let methods: [String: String]
        let invoke: (String, [Value], Bool) async throws -> Value

        init(name: String, methods: [String: String], invoke: @escaping (String, [Value], Bool) async throws -> Value) {
            self.name = name
            self.methods = methods
            self.invoke = invoke
        }

        /// Calls `method` with `args` (encoded by `Rutis.encode` /
        /// `Rutis.export`) and decodes its result into `R`. A method the
        /// service declares `sync` is a synchronous call (the far end hears
        /// so); the Task waits without holding a thread.
        public func call<R>(_ method: String, _ args: [Encoded], as type: R.Type = R.self) async throws -> R {
            guard let kind = methods[method] else {
                throw Refusal.noMethod(Refusal.Callee(service: name, method: method), methods.keys)
            }
            let value = try await invoke(method, args.map(\.value), kind == "sync")
            return try ValueCodec.decode(R.self, from: value)
        }

        /// Calls `method` with `args` of any type, its result as `R`.
        public func call<R>(_ method: String, values args: [Any?], as type: R.Type = R.self) async throws -> R {
            try await call(method, try args.map { Encoded(try ValueCodec.encodeAny($0 as Any)) }, as: type)
        }
    }
}

/// A protocol's proxy, generated by `@RutisClient`: a value of the protocol
/// calling a service's methods.
public protocol RutisProxy {
    /// The protocol, as the existential `ctx.use` returns.
    associatedtype Service
    /// The methods the proxy calls.
    static var rutisMethodNames: [String] { get }
    /// A proxy calling `client`.
    static func rutisMake(_ client: Rutis.Client) -> Service
}
