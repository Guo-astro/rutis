// An app's listener and a row's lifetime (the 0.15 review): a connection
// that never presents its upgrade request is dropped, and only so many
// upgrade at once; a listening app is connected once rutis greeted, not
// when a connection is upgraded; a task of an unloaded row cannot provide;
// a stopped dialing app leaves no socket open (#542); a connected app
// installs its own plugins, with their configurations, and announces their
// services (decision 2 of docs/design-remote-session-2026-10-28.md).
// `start()` and `stop()` in call order: crates/rutis-host/tests/swift_app.rs.
import Darwin
import Foundation
@testable import Rutis

@RutisService
final class LateService: @unchecked Sendable {
    func name() -> String { "late" }
}

private struct LetGo: Error {}

@RutisService
final class Greeting: @unchecked Sendable {
    let word: String
    init(word: String) { self.word = word }
    func say() -> String { word }
}

@RutisConfig
struct Units: Codable {
    var units = "metric"
}

struct Words: Codable {
    var word: String
}

@RutisConfig
struct Count: Codable {
    var count: Int
}

/// The far end of an app's connection, as rutis: what it was told. It
/// refuses `events.on` until it offers `events` (`offersEvents`), as rutis
/// does.
private final class FakeHost: @unchecked Sendable {
    private let lock = NSLock()
    private var told: [(method: String, frame: Value)] = []
    private var session: Peer?
    private var events = false

    var offersEvents: Bool {
        get { lock.withLock { events } }
        set { lock.withLock { events = newValue } }
    }

    func serve(_ socket: WebSocket) {
        var options = PeerOptions(endpoint: Endpoint(local: "test", expected: "pad"))
        options.dispatch = { [self] _, method, args in
            try lock.withLock {
                told.append((method, args.first ?? .undefined))
                if method == "events.on", !events { throw RutisError(name: "BindingError", message: "events.on is not offered here") }
            }
            return .undefined
        }
        guard let peer = try? Peer(channel: socket, options: options), (try? peer.start()) != nil else { return }
        lock.withLock { session = peer }
    }

    var peer: Peer? { lock.withLock { session } }
    var frames: [(method: String, frame: Value)] { lock.withLock { told } }
}

/// Sleeps `seconds`, cancelled or not.
private func uncancellableSleep(_ seconds: Double) async {
    await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
        DispatchQueue.global().asyncAfter(deadline: .now() + seconds) { done.resume() }
    }
}

final class SavedCtx: @unchecked Sendable {
    private let lock = NSLock()
    private var saved: Rutis.Ctx?
    var ctx: Rutis.Ctx? {
        get { lock.withLock { saved } }
        set { lock.withLock { saved = newValue } }
    }
}

final class Lines: @unchecked Sendable {
    private let lock = NSLock()
    private var lines: [String] = []
    func add(_ line: String) { lock.withLock { lines.append(line) } }
    var all: [String] { lock.withLock { lines } }
}

struct AppTests {
    /// A task (ctx.spawn) that ignores cancellation does not hold an unload
    /// past the shutdown deadline, nor does it get a deadline of its own
    /// after a listener used it up: listeners and tasks share one deadline,
    /// and what still runs past it is reported (#542).
    func aSpawnedTaskDoesNotHoldTheUnloadPastTheDeadline() async throws {
        let lines = Lines()
        let listening = Event()
        let plugin = Rutis.define(Plugin(name: "stubborn") { ctx in
            ctx.on("t/stubborn") { () async in
                listening.fire()
                await uncancellableSleep(3)
            }
            ctx.spawn { await uncancellableSleep(3) }
        })
        let runtime = Runtime([plugin], project: "", settings: Events.Settings(log: { lines.add($0) }, shutdownTimeout: 0.3))
        // Only a task.
        try await runtime.load(key: "a", name: nil, entry: "stubborn", config: .undefined, exports: [:], isolate: [:], scope: [])
        var started = Date()
        try await runtime.unload("a")
        var took = Date().timeIntervalSince(started)
        XCTAssertTrue(took < 1.5, "the unload waited \(took)s for a task")
        XCTAssertTrue(lines.all.contains("rutis: tasks of row a still running after 0.3s; unloading anyway"), "\(lines.all)")
        // A listener past the deadline, then the task: no second deadline.
        try await runtime.load(key: "b", name: nil, entry: "stubborn", config: .undefined, exports: [:], isolate: [:], scope: [])
        // As the host calls a listener here for an emit.
        let id = runtime.lock.withLock { runtime.byID.values.first { $0.row.key == "b" }?.id ?? "" }
        let call = runtime.arrive("events.call", [.record([("listener", .data(.string(id))), ("name", .data(.string("t/stubborn"))),
                                                           ("mode", .data(.string("emit"))), ("args", .list([]))])])
        Task { _ = try? await call?() }
        let heard = await listening.wait(seconds: 5)
        XCTAssertTrue(heard, "the listener runs")
        started = Date()
        try await runtime.unload("b")
        took = Date().timeIntervalSince(started)
        XCTAssertTrue(took < 1.5, "the unload waited \(took)s after the listeners' deadline")
        XCTAssertTrue(lines.all.contains("rutis: listeners of row b still running after 0.3s; unloading anyway"), "\(lines.all)")
    }

    func anUnloadedRowCannotProvide() async throws {
        let saved = SavedCtx()
        let plugin = Rutis.define(Plugin(name: "late") { ctx in
            saved.ctx = ctx
            ctx.provide("late", LateService())
        })
        let lines = Lines()
        let runtime = Runtime([plugin], project: "", settings: Events.Settings(log: { lines.add($0) }))
        try await runtime.load(key: "a", name: nil, entry: "late", config: .undefined, exports: [:], isolate: [:], scope: [])
        try await runtime.unload("a")
        let late = try XCTUnwrap(saved.ctx)
        _ = late.provide("late", LateService())
        XCTAssertTrue(lines.all.contains { $0.contains("row a is not active") }, "\(lines.all)")
        try await runtime.load(key: "a", name: nil, entry: "late", config: .undefined, exports: [:], isolate: [:], scope: [])
        try await runtime.unload("a")
    }

    /// Connections that send no upgrade request are dropped after the
    /// deadline, those beyond the cap at once; then a real upgrade works.
    func silentConnectionsAreDropped() throws {
        let listener = try WebSocketListener("ws://127.0.0.1:0/rutis", protocol: "rutis.\(endpointProtocol)", token: "secret",
                                             identity: { throw ChannelError("no TLS") }, upgradeTimeout: 0.5, maxUpgrading: 2)
        let address = try listener.start { socket in socket.close() }
        defer { listener.stop() }
        let port = try XCTUnwrap(port(of: address))
        let first = connectLoopback(port), second = connectLoopback(port)
        defer { close(first); close(second) }
        Thread.sleep(forTimeInterval: 0.1)
        XCTAssertEqual(listener.pendingUpgrades, 2)
        let third = connectLoopback(port)
        defer { close(third) }
        XCTAssertTrue(closedWithin(third, 0.3), "beyond the cap: dropped at once")
        XCTAssertTrue(closedWithin(first, 2), "silent: dropped at the deadline")
        XCTAssertTrue(closedWithin(second, 2), "silent: dropped at the deadline")
        Thread.sleep(forTimeInterval: 0.1)
        XCTAssertEqual(listener.pendingUpgrades, 0)
        let upgraded = connectLoopback(port)
        defer { close(upgraded) }
        XCTAssertTrue(sendUpgrade(upgraded).hasPrefix("HTTP/1.1 101"), "an upgrade after them")
    }

    /// A listening app is `connected` once rutis greeted, not when a
    /// connection is upgraded.
    func aListeningAppIsConnectedOnceGreeted() async throws {
        let lines = Lines()
        let plugin = Rutis.define(Plugin(name: "late") { ctx in ctx.provide("late", LateService()) })
        let app = RutisApp(plugins: [plugin], listen: "ws://127.0.0.1:0/rutis", id: "pad", token: "secret", log: { lines.add($0) })
        try await app.start()
        let address = try XCTUnwrap(lines.all.compactMap { $0.components(separatedBy: "listening on ").dropFirst().first }.first)
        let port = try XCTUnwrap(self.port(of: address))
        let socket = connectLoopback(port)
        defer { close(socket) }
        XCTAssertTrue(sendUpgrade(socket).hasPrefix("HTTP/1.1 101"))
        try await Task.sleep(nanoseconds: 500_000_000)
        XCTAssertEqual(app.state, .connecting, "upgraded, not greeted")
        await app.stop()
        XCTAssertEqual(app.state, .stopped(reason: nil))
    }

    /// A listening app started right after it stopped binds its address
    /// again: `stop()` returns once the port is free; asked for `:0`, it
    /// keeps the port it got first, as a device keeps its address.
    func aListeningAppStartsAgainRightAfterItStopped() async throws {
        let plugin = Rutis.define(Plugin(name: "late") { ctx in ctx.provide("late", LateService()) })
        let lines = Lines()
        let app = RutisApp(plugins: [plugin], listen: "ws://127.0.0.1:0/rutis", id: "pad", token: "secret", log: { lines.add($0) })
        var port: UInt16 = 0
        for round in 0..<20 {
            do {
                try await app.start()
            } catch {
                record("round \(round): \(error)")
                return
            }
            let said = lines.all.compactMap { $0.components(separatedBy: "listening on ").dropFirst().first }
            XCTAssertEqual(said.count, round + 1)
            let bound = try XCTUnwrap(said.last.flatMap(self.port(of:)))
            if round == 0 { port = bound }
            XCTAssertTrue(bound != 0 && bound == port, "round \(round): \(said)")
            // A connection the app closes as it stops: its end of it stays
            // in TIME_WAIT on the listening port.
            let socket = connectLoopback(port)
            XCTAssertTrue(sendUpgrade(socket).hasPrefix("HTTP/1.1 101"), "round \(round)")
            await app.stop()
            close(socket)
        }
    }

    /// A result the operation returns after the stop won the race goes to
    /// `discard`, as nobody else sees it (#542: a dialed socket is closed).
    func aRaceLostToTheStopDiscardsItsResult() async throws {
        let stop = StopSignal()
        let discarded = Lines()
        let raced = try await stop.race({ () -> String in
            stop.stop()
            // Returns after the stop, ignoring the cancellation.
            await Task.detached { try? await Task.sleep(nanoseconds: 50_000_000) }.value
            return "socket"
        }, discard: { discarded.add($0) })
        XCTAssertNil(raced)
        for _ in 0..<100 where discarded.all.isEmpty { try await Task.sleep(nanoseconds: 10_000_000) }
        XCTAssertEqual(discarded.all, ["socket"])
        let won = try await StopSignal().race({ "socket" }, discard: { discarded.add("won: \($0)") })
        XCTAssertEqual(won, "socket")
        try await Task.sleep(nanoseconds: 50_000_000)
        XCTAssertEqual(discarded.all, ["socket"], "the winner is not discarded")
    }

    /// Stopping a dialing app, while it dials, as the dial returns or
    /// while it waits for the greeting, leaves no URLSession, delegate
    /// cycle or heartbeat behind: repeated stop/start does not grow them
    /// (#542).
    func aDialingAppLeavesNoSocketOpenWhenStopped() async throws {
        let accepted = Accepted()
        let listener = try WebSocketListener("ws://127.0.0.1:0/rutis", protocol: "rutis.\(endpointProtocol)", token: "secret",
                                             identity: { throw ChannelError("no TLS") })
        let address = try listener.start { accepted.add($0) }
        defer { listener.stop() }
        let sessions = DialedSocket.live.sessions, heartbeats = DialedSocket.live.heartbeats
        let plugin = Rutis.define(Plugin(name: "late") { ctx in ctx.provide("late", LateService()) })
        let app = RutisApp(plugins: [plugin], connect: address, id: "pad", token: "secret", log: { _ in })
        for round in 0..<60 {
            let before = accepted.count
            try await app.start()
            switch round % 3 {
            case 0: break // while dialing
            case 1: // as the dial returns: stopped once upgraded
                let delay = Double(round % 7) * 0.0003
                accepted.then = { DispatchQueue.global().asyncAfter(deadline: .now() + delay) { Task { await app.stop() } } }
                for _ in 0..<500 where accepted.count == before { try await Task.sleep(nanoseconds: 1_000_000) }
                accepted.then = nil
            default: // waiting for the greeting
                for _ in 0..<500 where accepted.count == before { try await Task.sleep(nanoseconds: 1_000_000) }
                try await Task.sleep(nanoseconds: 50_000_000)
            }
            await app.stop()
        }
        XCTAssertTrue(accepted.count > 0, "the app dialed")
        accepted.closeAll()
        for _ in 0..<300 where DialedSocket.live.sessions != sessions || DialedSocket.live.heartbeats != heartbeats {
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        XCTAssertEqual(DialedSocket.live.sessions, sessions, "URLSessions left")
        XCTAssertEqual(DialedSocket.live.heartbeats, heartbeats, "heartbeats left")
    }

    /// A connected app offers `services` and installs its plugins in
    /// order, once rutis offers `events` (not before: until then rutis
    /// refuses `events.on`), each with its configuration (or its schema's
    /// defaults) once the services it injects are there: its own plugins' or rutis's. It
    /// announces each service its plugins provide, with the plugin's name,
    /// once rutis offers `services`, again to a new offer, never one rutis
    /// announced; rutis's rows cannot run them. A service rutis withdraws
    /// unloads the plugin injecting it, and it loads again once the service
    /// is back. Stopping withdraws the services, then the plugins clean up,
    /// latest first; the versions come from one sequence.
    func aConnectedAppInstallsItsPluginsAndAnnouncesTheirServices() async throws {
        let host = FakeHost()
        let listener = try WebSocketListener("ws://127.0.0.1:0/rutis", protocol: "rutis.\(endpointProtocol)", token: "secret",
                                             identity: { throw ChannelError("no TLS") })
        let address = try listener.start { host.serve($0) }
        defer { listener.stop() }
        let lines = Lines()
        let first = Rutis.define(Plugin(name: "first", config: Units.self) { ctx, config in
            // A listener registered while it loads: rutis must take it.
            ctx.on("first/ping") { () async in }
            lines.add("first \(config.units)")
            ctx.provide("units", Greeting(word: config.units))
            ctx.effect { lines.add("first cleans up") }
        })
        // A configuration its schema refuses: reported, and the next plugin
        // installed.
        let strict = Rutis.define(Plugin(name: "strict", config: Count.self) { _, _ in lines.add("strict ran") })
        // It uses the service of a plugin installed before it in the app.
        let second = Rutis.define(Plugin(name: "second", inject: ["units"], config: Words.self) { ctx, config in
            let units: String = try await ctx.use("units").call("say", values: [], as: String.self)
            lines.add("second \(config.word) \(units)")
            ctx.provide("greeting", Greeting(word: config.word))
            ctx.effect { lines.add("second cleans up") }
        })
        // It uses a service rutis announces.
        let third = Rutis.define(Plugin(name: "third", inject: ["clock"]) { ctx in
            let now: String = try await ctx.use("clock").call("now", values: [], as: String.self)
            lines.add("third \(now)")
            ctx.effect { lines.add("third cleans up") }
        })
        let plugins = [first, strict, second, third]
        let stray = RutisApp(plugins: plugins, configs: ["nope": Words(word: "x")], connect: address, id: "pad", token: "secret")
        do {
            try await stray.start()
            XCTFail("a configuration of no plugin was taken")
        } catch let error as RutisError {
            XCTAssertEqual(error.message, "configs names nope, which is not one of the plugins: first, second, strict, third")
        }
        let app = RutisApp(plugins: plugins, configs: ["strict": Words(word: "x"), "second": Words(word: "hello")],
                           connect: address, id: "pad", token: "secret", peer: "test", log: { lines.add($0) })
        try await app.start()
        func waitFor(_ what: String, _ condition: () -> Bool) async throws {
            for _ in 0..<500 where !condition() { try await Task.sleep(nanoseconds: 10_000_000) }
            XCTAssertTrue(condition(), what)
        }
        func told(_ method: String) -> [Value] { host.frames.filter { $0.method == method }.map(\.frame) }
        func ran(_ prefix: String) -> [String] { lines.all.filter { $0.hasPrefix(prefix) } }
        try await waitFor("the app's offer") { !told("link.offers").isEmpty }
        XCTAssertEqual(told("link.offers").first?.plainJSON?["families"], .array([.string("services")]))

        let peer = try XCTUnwrap(host.peer)
        // rutis's offers, version `version`: `families`, each since `since`.
        func offer(_ version: Int, _ families: [String], since: Int? = nil) async throws {
            _ = try await peer.call(target: "", method: "link.offers", args: [.data(.object([
                ("families", .array(families.map(JSON.string))), ("version", .int(version)),
                ("since", .object(families.map { ($0, .int(since ?? version)) })),
            ]))], sync: false)
        }
        // `services` alone: nothing installs before rutis offers `events`.
        try await offer(1, ["services"])
        try await Task.sleep(nanoseconds: 100_000_000)
        XCTAssertEqual(lines.all.filter { !$0.hasPrefix("rutis: connected") }, [], "nothing installed before rutis offers events")
        XCTAssertEqual(told("events.on").count, 0)

        host.offersEvents = true
        try await offer(2, ["services", "events"], since: 1)
        try await waitFor("first and second installed") { ran("second").count == 1 }
        XCTAssertEqual(lines.all.filter { !$0.hasPrefix("rutis:") }, ["first metric", "second hello metric"], "third waits for clock")
        XCTAssertEqual(lines.all.filter { $0.hasPrefix("rutis: plugin") }, ["rutis: plugin strict: BindingError: config: count is required"])
        XCTAssertEqual(told("events.on").map { $0["name"]?.string }, ["first/ping"])
        try await waitFor("two announcements") { told("services.announce").count == 2 }
        let announced = told("services.announce")
        guard announced.count == 2 else { return XCTFail("\(announced)") }
        XCTAssertEqual(announced.map { $0["name"]?.string }, ["units", "greeting"])
        XCTAssertEqual(announced.map { $0["plugin"]?.string }, ["first", "second"])
        XCTAssertEqual(announced.map { $0["version"]?.plainJSON }, [.number("1"), .number("2")])
        XCTAssertEqual(announced[1]["shape"]?.plainJSON, .object([("say", .string("async"))]))
        guard case let .record(methods)? = announced[1]["service"], case let .remoteFunction(say)? = methods.first(where: { $0.0 == "say" })?.1
        else { return XCTFail("\(announced[1])") }
        let said = try await say.call([], sync: false)
        XCTAssertEqual(said.plainJSON, .string("hello"))
        // A new offer of services: announced again, at the same versions.
        try await offer(3, ["services", "events"])
        try await waitFor("announced again") { told("services.announce").count == 4 }
        XCTAssertEqual(told("services.announce").dropFirst(2).map { $0["version"]?.plainJSON }, [.number("1"), .number("2")])

        // rutis's clock: third installs; withdrawn, it unloads; back, it loads again.
        func clock(_ method: String, _ version: Int, _ time: String = "") async throws {
            var fields: [(String, Value)] = [("name", .data(.string("clock"))), ("version", .data(.int(version)))]
            if method == "services.announce" {
                fields += [("service", .record([("now", .function(SessionFunction { _ in .data(.string(time)) }))])),
                           ("shape", .data(.object([("now", .string("async"))])))]
            }
            _ = try await peer.call(target: "", method: method, args: [.record(fields)], sync: false)
        }
        try await clock("services.announce", 1, "noon")
        try await waitFor("third installed") { ran("third").count == 1 }
        XCTAssertEqual(ran("third"), ["third noon"])
        try await clock("services.withdraw", 2)
        try await waitFor("third unloaded") { ran("third cleans up").count == 1 }
        // Older than the withdrawal: ignored.
        try await clock("services.announce", 1, "late")
        try await clock("services.announce", 3, "dusk")
        try await waitFor("third back") { ran("third").count == 3 }
        XCTAssertEqual(ran("third"), ["third noon", "third cleans up", "third dusk"])
        XCTAssertFalse(told("services.announce").contains { $0["name"]?.string == "clock" }, "rutis's service is not sent back")

        // rutis's rows cannot run an app's plugins.
        let mounted = try await peer.call(target: "", method: "mount", args: [.data(.object([]))], sync: false)
        XCTAssertEqual(mounted.plainJSON?["plugins"], .object([]))
        do {
            _ = try await peer.call(target: "", method: "rows.load", args: [.data(.string("again")), .data(.string("first"))], sync: false)
            XCTFail("rows.load ran an app's plugin")
        } catch let error as RutisError {
            XCTAssertEqual(error.name, "NotFound")
        }
        await app.stop()
        try await waitFor("two withdrawals") { told("services.withdraw").count == 2 }
        let withdrawn = told("services.withdraw")
        XCTAssertEqual(withdrawn.map { $0["name"]?.string }, ["greeting", "units"])
        XCTAssertEqual(withdrawn.map { $0["version"]?.plainJSON }, [.number("3"), .number("4")])
        XCTAssertEqual(lines.all.filter { $0.hasSuffix("cleans up") },
                       ["third cleans up", "third cleans up", "second cleans up", "first cleans up"])
    }

    // ── Sockets ──────────────────────────────────────────────────────

    private func port(of address: String) -> UInt16? {
        guard let url = URLComponents(string: address), let port = url.port else { return nil }
        return UInt16(port)
    }
}

/// The connections a listener accepted, kept open until `closeAll()`.
private final class Accepted: @unchecked Sendable {
    private let lock = NSLock()
    private var sockets: [WebSocket] = []
    private var next: (@Sendable () -> Void)?
    /// Run as a connection is accepted.
    var then: (@Sendable () -> Void)? {
        get { lock.withLock { next } }
        set { lock.withLock { next = newValue } }
    }
    func add(_ socket: WebSocket) {
        lock.withLock { sockets.append(socket) }
        then?()
    }
    var count: Int { lock.withLock { sockets.count } }
    func closeAll() { for socket in lock.withLock({ sockets }) { socket.close() } }
}

/// A TCP connection to 127.0.0.1:`port`.
private func connectLoopback(_ port: UInt16) -> Int32 {
    let fd = socket(AF_INET, SOCK_STREAM, 0)
    var address = sockaddr_in()
    address.sin_family = sa_family_t(AF_INET)
    address.sin_port = port.bigEndian
    address.sin_addr.s_addr = inet_addr("127.0.0.1")
    let connected = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) }
    }
    return connected == 0 ? fd : -1
}

/// Whether the far end closed `fd` within `seconds`.
private func closedWithin(_ fd: Int32, _ seconds: Double) -> Bool {
    let deadline = Date().addingTimeInterval(seconds)
    var buffer = [UInt8](repeating: 0, count: 4096)
    while Date() < deadline {
        var polled = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
        let wait = Int32(max(1, deadline.timeIntervalSinceNow * 1000))
        if poll(&polled, 1, wait) > 0, recv(fd, &buffer, buffer.count, 0) <= 0 { return true }
    }
    return false
}

/// Sends a valid upgrade request on `fd`; the response's head.
private func sendUpgrade(_ fd: Int32) -> String {
    let request = "GET /rutis HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
        "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n" +
        "Sec-WebSocket-Protocol: rutis.\(endpointProtocol)\r\nAuthorization: Bearer secret\r\n\r\n"
    _ = Array(request.utf8).withUnsafeBytes { send(fd, $0.baseAddress, $0.count, 0) }
    var head: [UInt8] = []
    var byte: UInt8 = 0
    let deadline = Date().addingTimeInterval(5)
    while Date() < deadline, !head.ends(with: Array("\r\n\r\n".utf8)) {
        var polled = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
        guard poll(&polled, 1, 100) > 0 else { continue }
        guard recv(fd, &byte, 1, 0) == 1 else { break }
        head.append(byte)
    }
    return String(decoding: head, as: UTF8.self)
}

private extension Array where Element == UInt8 {
    func ends(with suffix: [UInt8]) -> Bool { count >= suffix.count && Array(self[(count - suffix.count)...]) == suffix }
}
