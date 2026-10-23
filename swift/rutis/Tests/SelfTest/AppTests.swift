// An app's listener and a row's lifetime (the 0.15 review): a connection
// that never presents its upgrade request is dropped, and only so many
// upgrade at once; a listening app is connected once rutis greeted, not
// when a connection is upgraded; a task of an unloaded row cannot provide;
// a stopped dialing app leaves no socket open (#542).
// `start()` and `stop()` in call order: crates/rutis-host/tests/swift_app.rs.
import Darwin
import Foundation
@testable import Rutis

@RutisService
final class LateService: @unchecked Sendable {
    func name() -> String { "late" }
}

private struct LetGo: Error {}

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
    /// A filtered emit whose filter does not answer (and ignores
    /// cancellation) holds the later emits of its name only while its row
    /// is loaded: unloading the row lets the queue move on, and the emit
    /// starts nothing even once its filter answers.
    func anUnloadedRowsHeldEmitLetsTheQueueGo() async throws {
        let lines = Lines()
        let stuck = SavedCtx(), other = SavedCtx()
        let listener = Rutis.define(Plugin(name: "hears") { ctx in
            ctx.on("t/held") { (n: Int) in lines.add("got \(n)") }
        })
        let stuckPlugin = Rutis.define(Plugin(name: "stuck") { ctx in stuck.ctx = ctx })
        let otherPlugin = Rutis.define(Plugin(name: "other") { ctx in other.ctx = ctx })
        let runtime = Runtime([listener, stuckPlugin, otherPlugin], project: "", settings: Events.Settings(log: { lines.add($0) }))
        for (key, entry) in [("l", "hears"), ("s", "stuck"), ("o", "other")] {
            try await runtime.load(key: key, name: nil, entry: entry, config: .undefined, exports: [:], isolate: [:], scope: [])
        }
        let started = Date()
        // About a second, cancelled or not; then it would keep everyone.
        try XCTUnwrap(stuck.ctx).filter { _ in
            for _ in 0..<20 { try? await Task.sleep(nanoseconds: 50_000_000) }
            return true
        }.emit("t/held", 1)
        try XCTUnwrap(other.ctx).emit("t/held", 2)
        try await Task.sleep(nanoseconds: 200_000_000)
        XCTAssertFalse(lines.all.contains("got 2"), "behind the filtered emit (§5.5): \(lines.all)")
        try await runtime.unload("s")
        for _ in 0..<50 where !lines.all.contains("got 2") { try await Task.sleep(nanoseconds: 10_000_000) }
        XCTAssertTrue(lines.all.contains("got 2"), "the queue moved on: \(lines.all)")
        XCTAssertTrue(Date().timeIntervalSince(started) < 0.8, "before the filter answered")
        try await Task.sleep(nanoseconds: 1_200_000_000)
        XCTAssertFalse(lines.all.contains("got 1"), "the unloaded row's emit started nothing: \(lines.all)")
        XCTAssertFalse(lines.all.contains { $0.contains("filter of") }, "a let-go filter is no failure: \(lines.all)")
        try await runtime.unload("o")
        try await runtime.unload("l")
    }

    /// A task of a row that was unloaded provides: refused, and the row
    /// loads again (its service is not taken).
    func aLetGoFilterThatFailsIsNotReported() async throws {
        let lines = Lines()
        let saved = SavedCtx()
        let listener = Rutis.define(Plugin(name: "hears") { ctx in
            ctx.on("t/let-go") { (n: Int) in lines.add("got \(n)") }
        })
        let emitter = Rutis.define(Plugin(name: "emits") { ctx in saved.ctx = ctx })
        let runtime = Runtime([listener, emitter], project: "", settings: Events.Settings(log: { lines.add($0) }))
        for (key, entry) in [("l", "hears"), ("e", "emits")] {
            try await runtime.load(key: key, name: nil, entry: entry, config: .undefined, exports: [:], isolate: [:], scope: [])
        }
        // Ignores cancellation, then fails with an error of its own.
        try XCTUnwrap(saved.ctx).filter { _ in
            for _ in 0..<6 { try? await Task.sleep(nanoseconds: 50_000_000) }
            throw LetGo()
        }.emit("t/let-go", 1)
        try await Task.sleep(nanoseconds: 100_000_000)
        try await runtime.unload("e")
        try await Task.sleep(nanoseconds: 600_000_000)
        XCTAssertFalse(lines.all.contains { $0.contains("filter of") }, "let go: nothing reported: \(lines.all)")
        XCTAssertFalse(lines.all.contains("got 1"), "\(lines.all)")
        try await runtime.unload("l")
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
        let listener = try WebSocketListener("ws://127.0.0.1:0/rutis", protocol: "rutis.3", token: "secret",
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
        "Sec-WebSocket-Protocol: rutis.3\r\nAuthorization: Bearer secret\r\n\r\n"
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
