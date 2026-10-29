import Foundation

// The runtime: the rows one controller loads into this process, the
// services they provide and use, their listeners, and the control
// operations of the runtime contract (§3, §4).

/// What this runtime supports of the runtime contract. `rows.announce`:
/// services rows provide without declaring them are announced to the host
/// with their methods. `plugin-values`: a plugin
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
    let shape: Rutis.Advanced.Shape?
    init(row: String, value: RutisExportable, shape: Rutis.Advanced.Shape?) {
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
    let scope: [Rutis.Advanced.Scope]
    /// An app's own plugin, installed by the app (`install`), not by a row
    /// of rutis's: its services are announced with `services.announce`.
    let own: Bool
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

    init(key: String, name: String, entry: String, definition: Rutis.Definition, config: Value,
         exports: [String: [String: String]], isolate: [String: String], scope: [Rutis.Advanced.Scope],
         own: Bool = false, settings: Events.Settings = Events.Settings())
    {
        self.own = own
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
        let (tasks, waiting): ([Task<Void, Never>], Event?) = lock.withLock {
            cancelled = true
            let tasks = Array(running.values) + Array(spawned.values)
            guard listening > 0 else { return (tasks, nil) }
            let event = Event()
            idle.append(event)
            return (tasks, event)
        }
        for task in tasks { task.cancel() }
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

/// Where an app's own plugin stands.
enum OwnState { case waiting, loaded, failed }

/// A service an app's own plugin provides, as rutis is told of it.
struct Shared {
    let row: String
    let plugin: String
    let value: RutisExportable
    let shape: [String: String]
    let version: Int
}

/// A service rutis announced to this side; withdrawn when `methods` is nil.
struct FarService {
    let version: Int
    let methods: [String: String]?
    let functions: [String: FarFunction]
}

/// The rows one controller loads into this process.
final class Runtime: @unchecked Sendable {
    let definitions: [String: Rutis.Definition]
    /// The plugins added while it runs, instead of `definitions` (RutisTest).
    let shelf: Shelf?
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
    // The listeners of the rows here, by id.
    var byID: [String: LocalListener] = [:]
    var listenerSequence = 0
    /// What the host calls this runtime (mount `runtime`), for descriptors.
    var runtimeName: String?
    /// An app's own plugins, in order, with their configurations, and
    /// where each stands; whether one task is installing or unloading them.
    var own: [(definition: Rutis.Definition, config: Value)] = []
    var ownStates: [String: OwnState] = [:]
    var settling = false
    /// The services an app's own plugins provide, by id: announced to
    /// rutis once it offers `services`, again to every new offer of it.
    var shared: [String: Shared] = [:]
    /// The session's announcement sequence (`services.announce`,
    /// `services.withdraw`).
    var announcements = 0
    /// Which offer of `services` rutis made last (`link.offers`): nil
    /// while it offers none; and the version of its offers.
    var servicesOffer: Int?
    var farOffers = 0
    /// Whether rutis offered `events`: it serves this side a runtime
    /// session (`host`, `service`, `events` and `rows` come together), and
    /// an app's own plugins install only from then on.
    var hosting = false
    /// The services rutis announced to this side, by name: the newest
    /// version it said of each, nil methods once withdrawn (kept so that an
    /// older announcement arriving later does not bring it back).
    var far: [String: FarService] = [:]

    // Frames for the host, queued under `lock` in the order they are taken
    // and sent by flush once `lock` is released.
    private let outLock = Lock()
    private var outbox: [() -> Void] = []
    private let sendLock = Lock()

    init(_ definitions: [Rutis.Definition], project: String, settings: Events.Settings = Events.Settings(), shelf: Shelf? = nil) {
        self.settings = settings
        self.shelf = shelf
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
        if let shelf {
            guard let definition = shelf.get(entry) else {
                throw RutisError(name: "NotFound", message: "plugin \(entry) is not in this test process")
            }
            return try supported(definition)
        }
        guard let definition = definitions[entry] else {
            throw RutisError(name: "NotFound", message: "plugin \(entry) is not in this binary; it has: \(definitions.keys.sorted().joined(separator: ", "))")
        }
        return try supported(definition)
    }

    /// `definition`, unless it needs a newer plugin API than this runtime's.
    func supported(_ definition: Rutis.Definition) throws -> Rutis.Definition {
        if definition.api > Rutis.Advanced.pluginAPI {
            throw RutisError(name: "Error", message: "plugin \(definition.name) needs plugin API \(definition.api); this runtime supports \(Rutis.Advanced.pluginAPI)")
        }
        return definition
    }

    // ── Services ─────────────────────────────────────────────────────

    /// Reports the object now in an exported slot under a new handle;
    /// `lock` is held, the report queued.
    func refresh(_ id: String) {
        if let gone = shared[id], services[id]?.row != gone.row {
            shared[id] = nil
            announcements += 1
            // Also while closing: rutis hears it before the plugin cleans up.
            if servicesOffer != nil, let session {
                let args = [servicesWithdraw(name: id, version: announcements)]
                post {
                    // Awaited, as an announcement is: rutis takes it then.
                    guard let call = try? session.send(target: "", method: "services.withdraw", args: args, sync: false) else { return }
                    Task.detached { _ = try? await settle(try await call.value()) }
                }
            }
            reconcile()
        }
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
        if row.own, far[id]?.methods != nil {
            lock.unlock()
            fail(row, RutisError(name: "Error", message: "\(name) is already provided by rutis"))
            return {}
        }
        services[id] = Provided(row: row.key, value: value, shape: shape)
        row.provided.append(id)
        if row.own {
            share(row, id, value, shape)
        } else if slots[id] == nil, !name.contains("#") {
            // Not declared: announced, with its methods.
            slots[id] = Slot(row: row.key, name: name, methods: nil, announced: true)
            row.announced.append(id)
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

    /// Shares the service `id` an app's own plugin provides: announced
    /// once rutis offers `services`; `lock` is held, the frame queued.
    private func share(_ row: Row, _ id: String, _ value: RutisExportable, _ declared: Rutis.Advanced.Shape?) {
        var shape: [String: String] = [:]
        for method in value.rutisMethods().keys { shape[method] = "async" }
        for (method, kind) in declared?.kinds ?? [:] { shape[method] = kind.rawValue }
        announcements += 1
        let service = Shared(row: row.key, plugin: row.definition.name, value: value, shape: shape, version: announcements)
        shared[id] = service
        if servicesOffer != nil { announce(id, service) }
        reconcile()
    }

    /// Sends `services.announce` for `service`; `lock` is held.
    private func announce(_ id: String, _ service: Shared) {
        guard live, let session else { return }
        let args = [servicesAnnounce(plugin: service.plugin, name: id, service: service.value, shape: service.shape,
                                     version: service.version)]
        let log = settings.log
        post {
            guard let call = try? session.send(target: "", method: "services.announce", args: args, sync: false) else { return }
            Task.detached {
                do {
                    // rutis answers with a result to await: the announcement
                    // is taken (or refused) once it is awaited.
                    _ = try await settle(try await call.value())
                } catch {
                    log("rutis: cannot announce \(id): \(Events.failure(error))")
                }
            }
        }
    }

    /// rutis's offers (`link.offers`): every service shared is announced
    /// to each new offer of `services`, as rutis nodes do; the first offer
    /// of `events` lets an app's own plugins install.
    private func offered(_ offers: JSON) {
        lock.lock()
        let version = offers["version"]?.int64Value.map(Int.init) ?? 0
        guard version > farOffers else { return lock.unlock() }
        farOffers = version
        let families = offers["families"]?.arrayValue?.compactMap(\.stringValue) ?? []
        let offer = families.contains("services") ? (offers["since"]?["services"]?.int64Value.map(Int.init) ?? 0) : nil
        if families.contains("events"), !hosting {
            hosting = true
            reconcile()
        }
        if offer != servicesOffer {
            servicesOffer = offer
            if offer != nil {
                for (id, service) in shared.sorted(by: { $0.value.version < $1.value.version }) { announce(id, service) }
            }
        }
        lock.unlock()
        flush()
    }

    /// A service rutis announces or withdraws (`services.announce`,
    /// `services.withdraw`): the newest version wins; the app's plugins
    /// that inject it install or unload as it comes and goes.
    private func heard(_ method: String, _ fields: Value) throws {
        guard let name = fields["name"]?.string else {
            throw RutisError(name: "TypeError", message: "\(method): no service name")
        }
        let version = fields["version"]?.plainJSON?.int64Value.map(Int.init) ?? 0
        try lock.withLock {
            if let known = far[name], known.version >= version { return }
            guard method == "services.announce" else {
                far[name] = FarService(version: version, methods: nil, functions: [:])
                return reconcile()
            }
            if let existing = services[name] {
                throw RutisError(name: "Error", message: "\(name) is already provided here by plugin \(rows[existing.row]?.name ?? existing.row)")
            }
            var methods: [String: String] = [:]
            for (method, kind) in fields["shape"]?.plainJSON?.objectValue ?? [] { methods[method] = kind.stringValue ?? "async" }
            var functions: [String: FarFunction] = [:]
            if case let .record(entries)? = fields["service"] {
                for (method, value) in entries { if case let .remoteFunction(function) = value { functions[method] = function } }
            }
            far[name] = FarService(version: version, methods: methods, functions: functions)
            reconcile()
        }
    }

    /// Whether the service `name` is there for an app's own plugin.
    private func available(_ name: String) -> Bool {
        services[name] != nil || far[name]?.methods != nil || hosts[name] != nil
    }

    /// Installs an app's own plugins once rutis greeted: offers `services`
    /// (rutis's services come to this side), then, once rutis offers
    /// `events` (the families its plugins call are there then), each
    /// plugin, in order, with its configuration, once every service it
    /// injects is there. A far end that never offers `events` serves no
    /// runtime session: the plugins have nothing to install into and do not
    /// install.
    func install(_ plugins: [(definition: Rutis.Definition, config: Value)]) {
        guard let session else { return }
        lock.withLock { own = plugins }
        _ = try? session.send(target: "", method: "link.offers", args: [linkOffers()], sync: false)
        reconcile()
    }

    /// Brings the app's own plugins in line with the services there, in
    /// order: one waiting whose injected services are all there loads, one
    /// loaded that lacks one unloads (it loads again once they are back). A
    /// plugin that fails is reported and waits until a service it injects
    /// goes and comes back; the others go on. One task at a time; called
    /// with `lock` held or not.
    func reconcile() {
        Task.detached { [weak self] in await self?.reconciling() }
    }

    private func reconciling() async {
        guard lock.withLock({ () -> Bool in
            if settling || own.isEmpty { return false }
            settling = true
            return true
        }) else { return }
        while true {
            let next: (Rutis.Definition, Value, Bool)? = lock.withLock {
                if !closing {
                    for (definition, config) in own {
                        let ready = hosting && definition.inject.allSatisfy(available)
                        switch ownStates[definition.name] ?? .waiting {
                        case .waiting where ready:
                            return (definition, config, true)
                        case .loaded where !ready:
                            return (definition, config, false)
                        case .failed where !ready:
                            ownStates[definition.name] = .waiting
                        default:
                            continue
                        }
                    }
                }
                settling = false
                return nil
            }
            guard let (definition, config, loading) = next else { return }
            let name = definition.name
            if loading {
                do {
                    try await load(key: name, name: name, entry: name, definition: try supported(definition), config: config,
                                   exports: [:], isolate: [:], scope: [], own: true)
                    lock.withLock { ownStates[name] = .loaded }
                } catch {
                    let closed = lock.withLock { () -> Bool in
                        ownStates[name] = .failed
                        return closing
                    }
                    if !closed { settings.log("rutis: plugin \(name): \(Events.failure(error))") }
                }
            } else {
                try? await unload(name)
                lock.withLock { ownStates[name] = .waiting }
            }
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
              isolate: [String: String], scope: [Rutis.Advanced.Scope]) async throws
    {
        try await load(key: key, name: name, entry: entry, definition: try definition(entry), config: config,
                       exports: exports, isolate: isolate, scope: scope)
    }

    /// Loads the row `key` running `definition`: the plugin `entry` names in
    /// this binary, or a plugin definition this runtime handed over (a
    /// copy, `entry` its name).
    func load(key: String, name: String?, entry: String, definition: Rutis.Definition, config: Value,
              exports: [String: [String: String]], isolate: [String: String], scope: [Rutis.Advanced.Scope],
              own: Bool = false) async throws
    {
        let name = (name?.isEmpty ?? true) ? key : name!
        let row = Row(key: key, name: name, entry: entry, definition: definition, config: config, exports: exports,
                      isolate: isolate, scope: scope, own: own, settings: settings)
        try lock.withLock {
            if closing { throw RutisError(name: "Error", message: "runtime is closing") }
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
        return try await body(Rutis._Generated.Arguments(args, callee: callee)).value
    }

    private func argument<T: Decodable>(_ args: [Value], _ index: Int, _ type: T.Type = T.self) throws -> T {
        try Rutis._Generated.Arguments(args).decode(index, as: T.self)
    }

    func control(_ method: String, _ args: [Value]) async throws -> Value {
        switch method {
        case "mount":
            let mount = args.first ?? .undefined
            lock.withLock {
                runtimeName = mount["runtime"]?.string
            }
            var answer: [(String, JSON)] = [
                ("services", .object([])),
                ("features", .array(runtimeFeatures.map(JSON.string))),
                ("implementation", .object([("name", .string(Rutis.Advanced.implementation)), ("version", .string(Rutis.Advanced.sdkVersion))])),
                ("engine", .object([("name", .string("swift")), ("version", .string(swiftVersion))])),
            ]
            // A shelf's plugins come later: the host asks for each
            // (`rows.schema`).
            if shelf == nil {
                answer.insert(("plugins", .object(definitions.keys.sorted().map { ($0, definitions[$0]!.described) })), at: 2)
            }
            return .data(.object(answer))
        case "link.offers":
            offered((args.first ?? .undefined).plainJSON ?? .null)
            return .undefined
        case "services.announce", "services.withdraw":
            try heard(method, args.first ?? .undefined)
            return .undefined
        case "events.call":
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
                var scope: [Rutis.Advanced.Scope]?
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
    func client(_ row: Row, _ name: String) throws -> (Rutis.Advanced.Client, RutisExportable?) {
        guard row.definition.inject.contains(name) else {
            throw RutisError(name: RutisError.Name.bindingError,
                             message: "service \(name) is not available to this plugin, which does not declare it in inject: add '\(name)' to its inject",
                             code: "not-declared", data: ["service": name])
        }
        let id = row.id(name)
        let (local, host, remote) = lock.withLock { (services[id], hosts[id], far[id]) }
        if let local {
            let table = local.value.rutisMethods()
            var methods: [String: String] = [:]
            if let shape = local.shape {
                for (method, kind) in shape.kinds { methods[method] = kind.rawValue }
            } else {
                for method in table.keys { methods[method] = "async" }
            }
            let client = Rutis.Advanced.Client(name: name, methods: methods) { method, args, _ in
                let callee = Refusal.Callee(service: name, method: method)
                guard let body = table[method] else { throw Refusal.noMethod(callee, methods.keys) }
                return try await body(Rutis._Generated.Arguments(args, callee: callee)).value
            }
            return (client, local.value)
        }
        if let remote, let methods = remote.methods {
            // A service rutis announced: its methods are function references.
            let client = Rutis.Advanced.Client(name: name, methods: methods) { method, args, sync in
                guard let function = remote.functions[method] else {
                    throw Refusal.noMethod(Refusal.Callee(service: name, method: method), methods.keys)
                }
                return try await settle(try await function.call(args, sync: sync))
            }
            return (client, nil)
        }
        if let host {
            let client = Rutis.Advanced.Client(name: name, methods: host) { [weak self] method, args, sync in
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

extension Rutis.Advanced {
    /// A service used by name: calls its methods across the session (or on
    /// the provider's object, when it is in this process).
    public final class Client: @unchecked Sendable {
        /// The service's name; empty for an object of another process.
        public let name: String
        /// Its methods, each "sync" or "async"; empty for an object of
        /// another process, whose methods are known only to it.
        public let methods: [String: String]
        let invoke: (String, [Value], Bool) async throws -> Value
        /// The far end's object this client calls, if it is one (#687).
        let object: FarObject?

        init(name: String, methods: [String: String], invoke: @escaping (String, [Value], Bool) async throws -> Value) {
            self.name = name
            self.methods = methods
            self.invoke = invoke
            object = nil
        }

        /// An object of another process as a client: its methods are
        /// asynchronous calls of the object's.
        init(object: FarObject) {
            name = ""
            methods = [:]
            self.object = object
            invoke = { method, args, sync in try await object.call(method, args, sync: sync) }
        }

        /// The client of `object`, one per object while it is held.
        static func of(_ object: FarObject) -> Client {
            object.lock.lock()
            defer { object.lock.unlock() }
            if let client = object.client { return client }
            let client = Client(object: object)
            object.client = client
            return client
        }

        /// Calls `method` with `args` (encoded by `Rutis._Generated.encode` /
        /// `Rutis._Generated.export`) and decodes its result into `R`. A method the
        /// service declares `sync` is a synchronous call (the far end hears
        /// so); the Task waits without holding a thread.
        public func call<R>(_ method: String, _ args: [Rutis._Generated.Encoded], as type: R.Type = R.self) async throws -> R {
            try await call(method, args, as: type, object: nil)
        }

        /// `call(_:_:as:)` whose result, an object of another process, is
        /// made into `R` by `object` (a @RutisClient protocol's proxy,
        /// `rutisObjectProxy`).
        public func call<R>(_ method: String, _ args: [Rutis._Generated.Encoded], as type: R.Type = R.self,
                            object proxy: ((Rutis.Advanced.Client) -> R)?) async throws -> R
        {
            // An object has no method table: one it lacks is its process's
            // to say (no-method).
            let kind = methods[method]
            if kind == nil, object == nil {
                throw Refusal.noMethod(Refusal.Callee(service: name, method: method), methods.keys)
            }
            let value = try await invoke(method, args.map(\.value), kind == "sync")
            if let proxy, case let .remoteObject(far) = value { return proxy(Client.of(far)) }
            return try ValueCodec.decode(R.self, from: value)
        }

        /// Calls `method` with `args` as the session carries them: the
        /// result as it came (RutisTest).
        package func callValue(_ method: String, _ args: [Value]) async throws -> Value {
            let kind = methods[method]
            if kind == nil, object == nil {
                throw Refusal.noMethod(Refusal.Callee(service: name, method: method), methods.keys)
            }
            return try await invoke(method, args, kind == "sync")
        }

        /// Calls `method` with `args` of any type, its result as `R`.
        public func call<R>(_ method: String, values args: [Any?], as type: R.Type = R.self) async throws -> R {
            try await call(method, try args.map { Rutis._Generated.Encoded(try ValueCodec.encodeAny($0 as Any)) }, as: type)
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
    static func rutisMake(_ client: Rutis.Advanced.Client) -> Service
    /// The client it calls: handed on, a proxy of another process's object
    /// is that object again (#687).
    var rutisClient: Rutis.Advanced.Client { get }
}

/// How a parameter or result of type `T` takes an object of another
/// process: nil, it takes none (and `Any` takes a `Rutis.Advanced.Client`).
/// `@RutisClient` adds an overload for its protocol, which makes its proxy;
/// the macros call this, and the overload wins (#687 §四).
public func rutisObjectProxy<T>(_: T.Type) -> ((Rutis.Advanced.Client) -> T)? { nil }

