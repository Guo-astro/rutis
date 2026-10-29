// Objects with methods across processes, Swift's side (#687): a received
// object is the proxy of a @RutisClient protocol whose methods call the
// original; one proxy per object, the original again back home; a method
// the object lacks is no-method; a data parameter takes no object; data is
// copied; a call given up is cancelled where it runs; an object is let go
// of once its holder releases it or goes. The cross-process cases:
// crates/rutis-loader/tests/objects_cross.rs.
import Foundation
@testable import Rutis

/// An object with methods of the test's side.
@RutisService
final class Cache: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String: Int] = [:]
    func get(_ key: String) -> Int? { lock.withLock { values[key] } }
    func set(_ key: String, _ value: Int) { lock.withLock { values[key] = value } }
    func size() -> Int { lock.withLock { values.count } }
}

/// How the service uses an object of another process.
@RutisClient
protocol CacheClient {
    func get(_ key: String) async throws -> Int?
    func set(_ key: String, _ value: Int) async throws
}

struct Spot: Codable, Equatable {
    var x: Int
    var y: Int
}

/// Takes objects as a @RutisClient protocol, and data (which takes none).
@RutisService
final class Warmer: @unchecked Sendable {
    private let lock = NSLock()
    private var held: (any CacheClient)?
    private var spot: Spot?
    var kept: (any CacheClient)? { lock.withLock { held } }
    var seen: Spot? { lock.withLock { spot } }

    func warm(_ cache: any CacheClient) async throws -> Int? {
        try await cache.set("a", 1)
        lock.withLock { held = cache }
        return try await cache.get("a")
    }

    /// Whether `cache` is the proxy it keeps (its client, one per object).
    func same(_ cache: any CacheClient) -> Bool {
        guard let given = (cache as? any RutisProxy)?.rutisClient, let held = (kept as? any RutisProxy)?.rutisClient else { return false }
        return given === held
    }

    func back() -> any CacheClient { kept! }

    func place(_ spot: Spot) -> Int { spot.x }

    func keep(_ spot: Spot) -> Int {
        lock.withLock { self.spot = spot }
        return spot.y
    }

    func missing() async -> String {
        do {
            _ = try await (kept as! any RutisProxy).rutisClient.call("gett", [], as: Int?.self)
            return "answered"
        } catch let error as RutisError {
            return "\(error.code ?? ""): \(error.message)"
        } catch {
            return "\(error)"
        }
    }
}

/// Hands out its cache, an object with methods.
@RutisService
final class Source: @unchecked Sendable {
    let cache = Cache()
    func make() -> Cache { cache }
}

/// How a plugin uses `Source`: its result is an object of another process.
@RutisClient
protocol SourceClient {
    func make() async throws -> any CacheClient
}

/// Uses the cache `SourceClient.make` returns, received twice.
@RutisService
final class Reader: @unchecked Sendable {
    let source: any SourceClient
    init(source: any SourceClient) { self.source = source }

    func read() async throws -> String {
        let first = try await source.make()
        let second = try await source.make()
        try await first.set("k", 5)
        let same = (first as? any RutisProxy)?.rutisClient
        let again = (second as? any RutisProxy)?.rutisClient
        let value = try await second.get("k") ?? -1
        return "\(same != nil && same === again) \(value)"
    }
}

/// A class that is neither marked nor Codable.
final class Unmarked {}

/// An object whose method runs until its call is cancelled.
@RutisService
final class SlowObject: @unchecked Sendable {
    let cancelled = Lines()
    func run() async throws -> String {
        do {
            try await Task.sleep(nanoseconds: 60_000_000_000)
        } catch {
            cancelled.add("cancelled")
            throw error
        }
        return "done"
    }
}

/// A plugin served by the real runtime across a session of its own, the
/// test on the host's end: what crosses it crosses as between processes.
final class CrossSession: @unchecked Sendable {
    let host: Peer
    let served: Task<Void, Error>
    private let lock = NSLock()
    private var handles: [String: String] = [:]

    init(_ plugin: Rutis.Definition, services: [String: RutisExportable] = [:]) async throws {
        let (runtimeSide, hostSide) = try Channels.pair()
        served = Task.detached {
            try await Rutis.serveSession(channel: runtimeSide, endpoint: Endpoint(local: "swift", expected: "main", declare: ["runtime"]),
                                         project: NSTemporaryDirectory(), [plugin])
        }
        var options = PeerOptions(endpoint: Endpoint(local: "main", expected: "swift"))
        options.host = true
        let lock = self.lock
        let handles = Box()
        options.dispatch = { target, method, args in
            if target.isEmpty, method == "service", let id = args.first?.string {
                lock.withLock { handles.value[id] = args.count > 1 ? args[1].string : nil }
                return .undefined
            }
            guard target.hasPrefix("host:"), let service = services[String(target.dropFirst(5))] else {
                throw RutisError(name: "Error", message: "not served: \(target) \(method)")
            }
            let callee = Refusal.Callee(service: String(target.dropFirst(5)), method: method)
            guard let body = service.rutisMethods()[method] else { throw Refusal.noMethod(callee, service.rutisMethods().keys) }
            return try await body(Rutis._Generated.Arguments(args, callee: callee)).value
        }
        host = try Peer(channel: hostSide, options: options)
        self.box = handles
        try host.start()
        await host.ready()
        _ = try await host.call(target: "", method: "mount", args: [.data(.object([("runtime", .string("swift"))]))], sync: false)
        for (name, service) in services {
            let methods = JSON.object(service.rutisMethods().keys.sorted().map { ($0, .string("async")) })
            _ = try await host.call(target: "", method: "hosts.provide", args: [.data(.string(name)), .data(methods)], sync: false)
        }
        _ = try await host.call(target: "", method: "rows.load", args: [
            .data(.string("row")), .data(.string(plugin.name)), .null, .data(.array([])), .data(.array([])), .data(.object([])),
            .data(.object([])),
        ], sync: false)
    }

    private final class Box: @unchecked Sendable { var value: [String: String] = [:] }
    private let box: Box

    /// Calls `method` of the service `name` the plugin provides.
    func call<R>(_ name: String, _ method: String, _ args: Any?..., as _: R.Type) async throws -> R {
        guard let handle = lock.withLock({ box.value[name] }) else { throw RutisError(name: "Error", message: "no service \(name)") }
        let values = try args.map { try ValueCodec.encodeAny($0 as Any) }
        return try ValueCodec.decode(R.self, from: try await settle(try await host.call(target: handle, method: method, args: values, sync: false)))
    }

    func close() async {
        host.close()
        host.channel.close()
        _ = try? await served.value
    }
}

struct ObjectTests {
    // objects-case(swift): send-call-state
    // objects-case(swift): same-proxy-home
    // objects-case(swift): no-method
    // objects-case(swift): declared-type-refuses
    // objects-case(swift): data-stays-data
    /// Across a session, as between processes:
    /// the plugin's proxy calls the original, which changes; given twice it
    /// is the same proxy; sent back it is the original; a method the object
    /// lacks is its process's no-method; a data parameter refuses an
    /// object; data is copied.
    func anObjectIsTakenAsAClientProtocolAndGoesHome() async throws {
        let warmer = Warmer()
        let plugin = Rutis.define(Plugin(name: "warmer") { ctx in ctx.provide("warmer", warmer) })
        let session = try await CrossSession(plugin)
        defer { Task { await session.close() } }
        let cache = Cache()
        let warmed = try await session.call("warmer", "warm", cache, as: Int?.self)
        XCTAssertEqual(warmed, 1)
        XCTAssertEqual(cache.size(), 1, "the original object changed")
        let same = try await session.call("warmer", "same", cache, as: Bool.self)
        XCTAssertEqual(same, true, "one object, one proxy")
        let back = try await session.call("warmer", "back", as: Any?.self)
        XCTAssertTrue((back as AnyObject?) === cache, "back home, the original")
        let missing = try await session.call("warmer", "missing", as: String.self)
        XCTAssertEqual(missing, "no-method: Cache has no method gett (methods: get, set, size)")
        do {
            _ = try await session.call("warmer", "place", cache, as: Int.self)
            XCTFail("a data parameter takes no object")
        } catch let error as RutisError {
            XCTAssertEqual(error.code, "bad-argument")
            XCTAssertTrue(error.message.hasSuffix(
                "argument 1 should be a Spot, got an object (an object is received only as a @RutisClient protocol or Any)"),
                error.message)
        }
        let spot = Spot(x: 1, y: 2)
        let y = try await session.call("warmer", "keep", spot, as: Int.self)
        XCTAssertEqual(y, 2)
        XCTAssertEqual(warmer.seen, spot, "data arrives as an equal copy")
    }

    // objects-case(swift): return-array-event
    /// As the result of a @RutisClient protocol method, an object of
    /// another process is that protocol's proxy, of the same client each
    /// time.
    func anObjectAsAResultIsAClientProtocol() async throws {
        let plugin = Rutis.define(Plugin(name: "reader", inject: ["source"]) { ctx in
            ctx.provide("reader", Reader(source: try ctx.use("source", SourceClientProxy.self)))
        })
        let source = Source()
        let session = try await CrossSession(plugin, services: ["source": source])
        defer { Task { await session.close() } }
        let read = try await session.call("reader", "read", as: String.self)
        XCTAssertEqual(read, "true 5")
        XCTAssertEqual(source.cache.get("k"), 5, "the original object changed")
    }

    /// A class neither marked @RutisService nor Codable says how to make it
    /// cross (#687 §八).
    func anUnmarkedClassSaysHowToCross() throws {
        do {
            _ = try ValueCodec.encodeAny(Unmarked())
            XCTFail("an unmarked class does not cross")
        } catch {
            XCTAssertTrue("\(error)".contains("a Unmarked cannot cross: mark it @RutisService to pass it by reference, or make it Codable to copy it"), "\(error)")
        }
    }

    // objects-case(swift): cancel
    // objects-case(swift): holder-killed-releases
    /// A call of an object's method given up by its caller is cancelled
    /// where it runs; once its holder's session ends, the object is let go
    /// of.
    func callsOnObjectsAreCancelledAndHoldersRelease() async throws {
        let slow = SlowObject()
        let (a, b) = try Channels.pair()
        var options = PeerOptions(endpoint: Endpoint(local: "swift", expected: "main"))
        options.dispatch = { _, method, _ in method == "slow" ? .object(ServiceObject(slow)) : .undefined }
        let runtime = try Peer(channel: a, options: options)
        var hostOptions = PeerOptions(endpoint: Endpoint(local: "main", expected: "swift"))
        hostOptions.host = true
        let host = try Peer(channel: b, options: hostOptions)
        try runtime.start()
        try host.start()
        await host.ready()
        await runtime.ready()
        defer { runtime.close() }
        let given = try await host.call(target: "t", method: "slow", args: [], sync: false)
        guard case let .remoteObject(object) = given else { return XCTFail("\(given)") }
        XCTAssertEqual(runtime.referenceTables.exports, 1)
        let call = Task { try await object.call("run", [], sync: false) }
        try await Task.sleep(nanoseconds: 100_000_000)
        call.cancel()
        for _ in 0 ..< 100 where slow.cancelled.all.isEmpty {
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        XCTAssertEqual(slow.cancelled.all, ["cancelled"])
        // The holder goes without releasing it: its end of the channel
        // closes.
        host.close()
        b.close()
        for _ in 0 ..< 100 where runtime.referenceTables.exports != 0 {
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        XCTAssertEqual(runtime.referenceTables.exports, 0, "the object is let go of")
    }
}
