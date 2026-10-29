// RutisTest on a device and in the iOS simulator, where no `rutis-host`
// process runs: the test is an app that installs its own plugins
// (`RutisApp`, docs/guide/swift-app.md), connected to a rutis host on the
// development machine. Each `RutisTest.load` starts one: the test's
// services (`rutistest-services`), the plugin with its config, then the
// test's side (`rutistest`, which injects the services the plugin declares
// in `provides`). The host dispatches the events; it neither checks the
// plugin's config nor runs more rows of it: the plugin decodes its config
// itself (a missing key takes its type's default), and `Loaded.load`,
// `update` and `isolate` are for the macOS test tool only.
//
// The host: a `rutis-host/listen` row on the development machine (see the
// Swift app guide); `RUTIS_TEST_CONNECT` is its address
// (`ws://127.0.0.1:<port>/rutis` from the simulator), `RUTIS_TEST_TOKEN` its
// token. With `xcrun simctl spawn`, set them as `SIMCTL_CHILD_RUTIS_TEST_…`.
#if !os(macOS) && canImport(Network)
import Foundation
import Rutis

public enum RutisTest {
    /// Loads `plugin` with `config` (an Encodable value; nil for none) and
    /// the services in `services` in an app of its own, connected to the
    /// host at RUTIS_TEST_CONNECT. `row` and `isolate` are the macOS test
    /// tool's: here `row` must be nil or the plugin's name, `isolate` empty.
    public static func load(_ plugin: Rutis.Definition, config: Any? = nil, services: [String: RutisExportable] = [:],
                            row: String? = nil, isolate: [String: Any] = [:]) async throws -> Loaded
    {
        let environment = ProcessInfo.processInfo.environment
        guard let address = environment["RUTIS_TEST_CONNECT"], !address.isEmpty else {
            throw RutisError(name: "Error", message: "RutisTest on a device connects to a rutis host: set RUTIS_TEST_CONNECT (and RUTIS_TEST_TOKEN)")
        }
        guard row == nil || row == plugin.name, isolate.isEmpty else {
            throw RutisError(name: "Error", message: "RutisTest on a device loads the plugin once, as an app installs it: no row id, no isolate")
        }
        var configs: [String: any Encodable] = [:]
        if let config {
            guard let encodable = config as? any Encodable else {
                throw RutisError(name: "Error", message: "RutisTest on a device takes an Encodable config")
            }
            configs[plugin.name] = encodable
        }
        let manifest = try JSONSerialization.jsonObject(with: Data(Rutis.Advanced.manifest([plugin]).utf8)) as? [String: Any]
        let declared = (manifest?["plugins"] as? [String: Any])?[plugin.name] as? [String: Any]
        for name in (declared?["inject"] as? [String]) ?? [] where services[name] == nil {
            throw RutisError(name: "Error", message: "RutisTest: the plugin injects \(name): give the test a service \(name)")
        }
        let provides = ((declared?["provides"] as? [String: Any]) ?? [:]).keys.sorted()
        let run = DeviceRun(provides: provides)
        let given = Rutis.define(Plugin(name: "rutistest-services") { ctx in
            for (name, service) in services.sorted(by: { $0.key < $1.key }) { ctx.provide(name, service) }
        })
        let driver = Rutis.define(Plugin(name: "rutistest", inject: provides) { ctx in run.started(ctx) })
        let app = RutisApp(plugins: [given, plugin, driver], configs: configs, connect: address, id: "rutistest",
                           token: environment["RUTIS_TEST_TOKEN"] ?? "", log: { line in run.log(line, plugin: plugin.name) })
        run.app = app
        try await app.start()
        do {
            try await run.wait(seconds: 30)
        } catch {
            await app.stop()
            throw error
        }
        return Loaded(run: run, row: plugin.name)
    }
}

final class DeviceRun: @unchecked Sendable {
    let provides: [String]
    var app: RutisApp?
    let lock = NSLock()
    private var ctx: Rutis.Ctx?
    private var failed: String?
    var lines: [String] = []

    init(provides: [String]) { self.provides = provides }

    func started(_ ctx: Rutis.Ctx) { lock.withLock { self.ctx = ctx } }

    func log(_ line: String, plugin: String) {
        FileHandle.standardError.write(Data((line + "\n").utf8))
        lock.withLock {
            lines.append(line)
            let prefix = "rutis: plugin \(plugin): "
            if line.hasPrefix(prefix), failed == nil { failed = String(line.dropFirst(prefix.count)) }
        }
    }

    var driver: Rutis.Ctx { lock.withLock { ctx! } }

    func wait(seconds: Double) async throws {
        let deadline = Date().addingTimeInterval(seconds)
        while true {
            // The plugin's failure is written before the test's side installs.
            let (ready, failure) = lock.withLock { (ctx != nil, failed) }
            if let failure { throw RutisError(name: "Error", message: failure) }
            if ready { return }
            if Date() > deadline { throw RutisError(name: "Error", message: "RutisTest: the plugin did not start within \(Int(seconds)) s") }
            try await Task.sleep(nanoseconds: 2_000_000)
        }
    }
}

/// The plugin, installed by the test's app.
public final class Loaded: @unchecked Sendable {
    let run: DeviceRun
    /// The plugin's name.
    public let row: String

    init(run: DeviceRun, row: String) {
        self.run = run
        self.row = row
    }

    private static func macOSOnly(_ what: String) -> RutisError {
        RutisError(name: "Error", message: "RutisTest: \(what) is for the macOS test tool (rutis-host test): on a device the app installs the plugin once")
    }

    public func load(config _: Any? = nil, row _: String? = nil, isolate _: [String: Any] = [:]) async throws -> Loaded {
        throw Self.macOSOnly("Loaded.load")
    }

    /// The service `name` the plugin declares in `provides`.
    public func service(_ name: String) async throws -> Service {
        guard run.provides.contains(name) else {
            throw RutisError(name: "Error", message: "RutisTest on a device calls the services the plugin declares in provides: \(name) is not one")
        }
        return Service(client: try run.driver.use(name), name: name)
    }

    var ctx: Rutis.Ctx { run.driver }

    /// The services the plugin declares.
    public var provided: [String] { run.provides }

    /// The lines the app's runtime wrote.
    public var reported: [String] { get async { run.lock.withLock { run.lines } } }

    public func unload() async throws { await close() }

    public func update(config _: Any?) async throws { throw Self.macOSOnly("Loaded.update") }

    /// Stops the test's app: its plugins clean up, latest first.
    public func close() async { await run.app?.stop() }

    @discardableResult
    public func on<each A, R>(_ name: String, prepend: Bool = false, global: Bool = false,
                              _ listener: @escaping (repeat each A) async throws -> R) async -> () async -> Void
    {
        let remove = run.driver.on(name, prepend: prepend, global: global, listener)
        return { remove() }
    }

    public func emit(_ name: String, _ args: Any?...) async throws { try run.driver.emit(name, arguments: args) }

    public func parallel(_ name: String, _ args: Any?...) async throws { try await run.driver.parallel(name, arguments: args) }

    public func serial<R>(_ name: String, _ args: Any?..., as type: R.Type) async throws -> R? {
        try await run.driver.serial(name, arguments: args, as: type)
    }

    public func waterfall<R>(_ name: String, _ args: Any?..., as type: R.Type,
                             terminal: (([Any?]) async throws -> Any?)? = nil) async throws -> R?
    {
        try await run.driver.waterfall(name, arguments: args, as: type, terminal: terminal)
    }

    public func bail<R>(_ name: String, _ args: Any?..., as type: R.Type) async throws -> R? {
        try await run.driver.bail(name, arguments: args, as: type)
    }
}

/// A service the plugin provides, as a plugin of the same app calls it.
public final class Service: @unchecked Sendable {
    let client: Rutis.Advanced.Client
    public let name: String
    public var methods: [String: String] { client.methods }

    init(client: Rutis.Advanced.Client, name: String) {
        self.client = client
        self.name = name
    }

    package func callValue(_ method: String, _ args: [Value]) async throws -> Value {
        try await client.callValue(method, args)
    }

    public func call<R>(_ method: String, _ args: Any?..., as _: R.Type = R.self) async throws -> R {
        try await client.call(method, values: args, as: R.self)
    }
}
#endif
