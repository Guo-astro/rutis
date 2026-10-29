// RutisTest tests a plugin against the real rutis kernel
// (docs/design-platform-cleanup-2026-10-28.md §7.5): it gives the plugin the
// services it injects, calls the services it provides, emits and listens to
// events, and unloads it.
//
//	let loaded = try await RutisTest.load(weatherPlugin, config: ["city": "Oslo"], services: ["clock": FakeClock()])
//	let today = try await loaded.service("weather").call("today", as: String.self)
//	await loaded.close()
//
// The test process runs the real Swift runtime, and `rutis-host test` (on
// PATH, or RUTIS_HOST) a host whose session is with it: the plugin is a row
// of that host, which checks its config, gates it on the services it
// injects and dispatches every event. The test's side is a plugin of the
// same runtime (`rutistest`) using the public ctx API: it provides the
// test's services, listens and emits. One host per test process; a test's
// rows are unloaded when it closes.
#if os(macOS)
import Foundation
import Rutis

public enum RutisTest {
    /// Loads `plugin` with `config` (a Codable value or a dictionary; nil
    /// for none) and the services in `services` (instances of @RutisService
    /// types) as a row of the test host. `row` is its id (by default the
    /// plugin's name, then `-2`, `-3`…); `isolate` is the row's `isolate`
    /// (service name: `true` or a label), as in rutis.json. More rows of
    /// the plugin load with `Loaded.load`.
    public static func load(_ plugin: Rutis.Definition, config: Any? = nil, services: [String: RutisExportable] = [:],
                            row: String? = nil, isolate: [String: Any] = [:]) async throws -> Loaded
    {
        let host = try await TestHost.shared()
        let run = try TestRun(host: host, plugin: plugin)
        for name in run.injects where services[name] == nil {
            throw RutisError(name: "Error", message: "RutisTest: the plugin injects \(name): give the test a service \(name)")
        }
        do {
            // The test's services: the driver plugin's, while the test runs.
            run.withdraws = try await host.provide(services)
            return try await run.load(config: config, row: row, isolate: isolate)
        } catch {
            await run.close()
            throw error
        }
    }

    /// How long a row may take to start, and a service to come.
    static let wait = 10.0
}

/// The host of this test process: `rutis-host test`, and the runtime of its
/// session, in this process.
final class TestHost: @unchecked Sendable {
    let shelf = Shelf()
    let lock = NSLock()
    /// The driver plugin's ctx and its client of `rutis-host/test`.
    private var driver: (ctx: Rutis.Ctx, harness: Rutis.Advanced.Client)?
    /// Every line the runtime reported.
    var lines: [String] = []
    /// How many of the test's own plugins were put on the shelf.
    private var entries = 0
    /// The entry of each plugin put on the shelf. An entry is never given
    /// to another plugin: the host remembers what it learnt of a name.
    private var shelved: [ObjectIdentifier: String] = [:]
    /// The rows loaded and not unloaded, by id; the test services given.
    var live: Set<String> = []
    var services: Set<String> = []
    var process: HostProcess?

    var ctx: Rutis.Ctx { lock.withLock { driver!.ctx } }

    private static let starting = Starting()

    /// The host, started on first use.
    static func shared() async throws -> TestHost {
        try await starting.host()
    }

    actor Starting {
        var started: Task<TestHost, Error>?
        func host() async throws -> TestHost {
            if let started { return try await started.value }
            let task = Task { try await TestHost.start() }
            started = task
            return try await task.value
        }
    }

    static func start() async throws -> TestHost {
        let host = TestHost()
        let ready = Ready()
        host.shelf.put("rutistest", Rutis.define(Plugin(name: "rutistest", inject: ["rutis-host/test"]) { ctx in
            let harness = try ctx.use("rutis-host/test")
            host.lock.withLock { host.driver = (ctx, harness) }
            ready.fire(nil)
        }))
        let (channel, process) = try hostChannel(driver: "rutistest") { status in
            ready.fire(RutisError(name: "Error", message: "RutisTest: rutis-host test exited with status \(status)"))
        }
        host.process = process
        let log: @Sendable (String) -> Void = { [weak host] line in
            FileHandle.standardError.write(Data((line + "\n").utf8))
            guard let host else { return }
            host.lock.withLock { host.lines.append(line) }
        }
        Task.detached {
            try? await Rutis.serveShelf(channel: channel, endpoint: Endpoint(local: "test", declare: ["runtime"]),
                                       project: FileManager.default.currentDirectoryPath, shelf: host.shelf, log: log)
        }
        if let error = try await ready.wait(seconds: 30, "rutis-host test did not start") { throw error }
        return host
    }

    /// Puts `definition` on the shelf, once: as its name when no other
    /// plugin took it, else as `<name>-2`, `-3`…
    func put(_ definition: Rutis.Definition) -> String {
        lock.withLock {
            if let entry = shelved[ObjectIdentifier(definition)] { return entry }
            var entry = definition.name
            var next = 2
            while shelf.get(entry) != nil {
                entry = "\(definition.name)-\(next)"
                next += 1
            }
            shelf.put(entry, definition)
            shelved[ObjectIdentifier(definition)] = entry
            return entry
        }
    }

    /// A new name for one of the test's own plugins.
    func entry(_ prefix: String) -> String {
        lock.withLock {
            entries += 1
            return "\(prefix)-\(entries)"
        }
    }

    /// Calls `rutis-host/test`.
    func call<R>(_ method: String, _ args: [Any?], as type: R.Type = R.self) async throws -> R {
        let harness = lock.withLock { driver!.harness }
        return try await harness.call(method, values: args, as: type)
    }

    /// Waits up to `seconds` until `free` holds: a test that ended just
    /// before (closing in a Task of its own) lets go of its rows and
    /// services.
    func settle(_ seconds: Double, _ free: () -> Bool) async -> Bool {
        let deadline = Date().addingTimeInterval(seconds)
        while !lock.withLock(free) {
            if Date() > deadline { return false }
            try? await Task.sleep(nanoseconds: 1_000_000)
        }
        return true
    }

    /// Gives the services of a test, unless another test gives one of them.
    func provide(_ services: [String: RutisExportable]) async throws -> [() -> Void] {
        let names = Set(services.keys)
        guard await settle(RutisTest.wait, { self.services.isDisjoint(with: names) }) else {
            let taken = lock.withLock { self.services.intersection(names) }.sorted().joined(separator: ", ")
            throw RutisError(name: "Error", message: "RutisTest: another test gives \(taken)")
        }
        lock.withLock { self.services.formUnion(names) }
        return services.sorted(by: { $0.key < $1.key }).map { name, service in
            let withdraw = ctx.provide(name, service)
            return { [self] in
                withdraw()
                lock.withLock { _ = self.services.remove(name) }
            }
        }
    }

    /// Loads `entry` as a row, its id `id` (else `name`, once no row has
    /// it); its id, once its plugin applied.
    func load(_ entry: String, name: String, config: Any?, id: String?, isolate: [String: Any]) async throws -> String {
        var options: [String: Any] = [:]
        if let id {
            options["id"] = id
        } else if await settle(2, { !self.live.contains(name) }) {
            options["id"] = name
        }
        if !isolate.isEmpty { options["isolate"] = isolate }
        let row = try await call("load", [entry, config, options.isEmpty ? nil : options], as: String.self)
        lock.withLock { _ = live.insert(row) }
        do {
            try await started(row)
        } catch {
            try? await unload(row)
            throw error
        }
        return row
    }

    func unload(_ row: String) async throws {
        _ = try await call("unload", [row], as: Void.self)
        lock.withLock { _ = live.remove(row) }
    }

    /// Waits until the row `row` applied.
    func started(_ row: String) async throws {
        let deadline = Date().addingTimeInterval(RutisTest.wait)
        while !shelf.loaded(row) {
            if Date() > deadline {
                throw RutisError(name: "Error", message: "RutisTest: row \(row) did not start within \(Int(RutisTest.wait)) s")
            }
            try await Task.sleep(nanoseconds: 1_000_000)
        }
    }
}

/// Fires once, with an error or none.
final class Ready: @unchecked Sendable {
    private let lock = NSLock()
    private var result: Error??

    func fire(_ error: Error?) {
        lock.withLock { if result == nil { result = .some(error) } }
    }

    /// The error it fired with; throws when it did not fire within
    /// `seconds`.
    func wait(seconds: Double, _ late: String) async throws -> Error? {
        let deadline = Date().addingTimeInterval(seconds)
        while true {
            if let result = lock.withLock({ result }) { return result }
            if Date() > deadline { throw RutisError(name: "Error", message: "RutisTest: \(late) within \(Int(seconds)) s") }
            try await Task.sleep(nanoseconds: 1_000_000)
        }
    }
}

/// What one `RutisTest.load` started: its rows, its services, its
/// listeners; all go when it closes.
final class TestRun: @unchecked Sendable {
    let host: TestHost
    let entry: String
    let name: String
    let injects: [String]
    let lock = NSLock()
    /// The rows it loaded, in order, and the entries of its own plugins.
    var rows: [String] = []
    var entries: [String] = []
    var withdraws: [() -> Void] = []
    var listeners: [() -> Void] = []
    /// The runtime's lines before it started.
    let mark: Int
    var closed = false

    init(host: TestHost, plugin: Rutis.Definition) throws {
        self.host = host
        let manifest = try JSONSerialization.jsonObject(with: Data(Rutis.Advanced.manifest([plugin]).utf8)) as? [String: Any]
        let declared = (manifest?["plugins"] as? [String: Any])?[plugin.name] as? [String: Any]
        injects = (declared?["inject"] as? [String]) ?? []
        entry = host.put(plugin)
        name = plugin.name
        mark = host.lock.withLock { host.lines.count }
    }

    func load(config: Any?, row: String?, isolate: [String: Any]) async throws -> Loaded {
        let id = try await host.load(entry, name: name, config: config, id: row, isolate: isolate)
        lock.withLock { rows.append(id) }
        return Loaded(run: self, row: id)
    }

    /// Unloads the row `id`.
    func unload(_ id: String) async throws {
        guard lock.withLock({ rows.contains(id) }) else { return }
        try await host.unload(id)
        lock.withLock { rows.removeAll { $0 == id } }
    }

    func close() async {
        let (rows, listeners, withdraws, entries): ([String], [() -> Void], [() -> Void], [String]) = lock.withLock {
            if closed { return ([], [], [], []) }
            closed = true
            return (self.rows, self.listeners, self.withdraws, self.entries)
        }
        for row in rows.reversed() { try? await unload(row) }
        for remove in listeners { remove() }
        for withdraw in withdraws.reversed() { withdraw() }
        for entry in entries { host.shelf.remove(entry) }
    }
}

/// A row of a plugin loaded for a test.
public final class Loaded: @unchecked Sendable {
    let run: TestRun
    /// The row's id.
    public let row: String

    init(run: TestRun, row: String) {
        self.run = run
        self.row = row
    }

    /// Loads another row of the same plugin into the same host.
    public func load(config: Any? = nil, row: String? = nil, isolate: [String: Any] = [:]) async throws -> Loaded {
        try await run.load(config: config, row: row, isolate: isolate)
    }

    /// The service `name`, as a plugin of the same runtime that injects it
    /// uses it: its declared methods, or, undeclared, the methods of its
    /// type, all `async`. Waits for it (the announcement may be on its
    /// way).
    public func service(_ name: String) async throws -> Service {
        let host = run.host
        let found = Ready()
        let box = ClientBox()
        let entry = host.entry("rutistest-use")
        host.shelf.put(entry, Rutis.define(Plugin(name: "rutistest", inject: [name]) { ctx in
            box.set(try ctx.use(name))
            found.fire(nil)
        }))
        run.lock.withLock { run.entries.append(entry) }
        let user = try await host.call("load", [entry, nil, nil], as: String.self)
        host.lock.withLock { _ = host.live.insert(user) }
        run.lock.withLock { run.rows.append(user) }
        do {
            _ = try await found.wait(seconds: RutisTest.wait, "the plugin did not provide \(name)")
        } catch {
            try? await run.unload(user)
            throw RutisError(name: "Error", message: "RutisTest: the plugin does not provide \(name)")
        }
        return Service(client: box.get()!, name: name)
    }

    /// The services the row provides now.
    public var provided: [String] { run.host.shelf.provided(row) }

    /// What was reported since the test started: each line the runtime
    /// wrote (a listener of a plugin here failing in an `emit`), then each
    /// line the host wrote of the test's rows.
    public var reported: [String] {
        get async {
            let own = run.host.lock.withLock { Array(run.host.lines.dropFirst(run.mark)) }
            var host: [String] = []
            for row in run.lock.withLock({ run.rows }) {
                host += (try? await run.host.call("reported", [row], as: [String].self)) ?? []
            }
            return own + host
        }
    }

    /// Unloads the row: the runtime cancels its running listeners and waits
    /// for them first, up to RUTIS_SHUTDOWN_TIMEOUT.
    public func unload() async throws { try await run.unload(row) }

    /// Changes the row's config as a host does: the host checks it, and the
    /// plugin applies again.
    public func update(config: Any?) async throws {
        _ = try await run.host.call("update", [row, config], as: Void.self)
        try await run.host.started(row)
    }

    /// Unloads every row of the test, latest first, and withdraws its
    /// services and listeners. The host goes on, for the next test.
    public func close() async { await run.close() }

    // ── Events ───────────────────────────────────────────────────────

    /// Listens to `name` until the returned function is called or the test
    /// closes, as a plugin of the runtime does (`ctx.on`).
    @discardableResult
    public func on<each A, R>(_ name: String, prepend: Bool = false, global: Bool = false,
                              _ listener: @escaping (repeat each A) async throws -> R) async -> () async -> Void
    {
        let remove = run.host.ctx.on(name, prepend: prepend, global: global, listener)
        run.lock.withLock { run.listeners.append(remove) }
        return { remove() }
    }

    var ctx: Rutis.Ctx { run.host.ctx }

    /// Starts every listener of `name`.
    public func emit(_ name: String, _ args: Any?...) async throws { try ctx.emit(name, arguments: args) }

    /// Runs every listener of `name` and waits for all; failures are an
    /// AggregateError in registration order.
    public func parallel(_ name: String, _ args: Any?...) async throws { try await ctx.parallel(name, arguments: args) }

    /// The first result of the listeners of `name`, one at a time.
    public func serial<R>(_ name: String, _ args: Any?..., as type: R.Type) async throws -> R? {
        try await ctx.serial(name, arguments: args, as: type)
    }

    /// The outermost listener's value; `terminal` gets the arguments as an
    /// array.
    public func waterfall<R>(_ name: String, _ args: Any?..., as type: R.Type,
                             terminal: (([Any?]) async throws -> Any?)? = nil) async throws -> R?
    {
        try await ctx.waterfall(name, arguments: args, as: type, terminal: terminal)
    }

    /// `serial` as a synchronous call.
    public func bail<R>(_ name: String, _ args: Any?..., as type: R.Type) async throws -> R? {
        try await ctx.bail(name, arguments: args, as: type)
    }
}

final class ClientBox: @unchecked Sendable {
    private let lock = NSLock()
    private var client: Rutis.Advanced.Client?
    func set(_ value: Rutis.Advanced.Client) { lock.withLock { client = value } }
    func get() -> Rutis.Advanced.Client? { lock.withLock { client } }
}

/// A service a plugin provides, as a plugin of the same runtime calls it.
public final class Service: @unchecked Sendable {
    let client: Rutis.Advanced.Client
    public let name: String
    /// Its methods, each "sync" or "async".
    public var methods: [String: String] { client.methods }

    init(client: Rutis.Advanced.Client, name: String) {
        self.client = client
        self.name = name
    }

    /// The result as the session carries it.
    package func callValue(_ method: String, _ args: [Value]) async throws -> Value {
        try await client.callValue(method, args)
    }

    /// Calls `method` with `args` and decodes its result into `R`.
    public func call<R>(_ method: String, _ args: Any?..., as _: R.Type = R.self) async throws -> R {
        try await client.call(method, values: args, as: R.self)
    }
}
#endif
