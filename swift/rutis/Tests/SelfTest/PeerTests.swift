import Foundation
@testable import Rutis

/// A session in this process: `host` drives `runtime`, which serves
/// `dispatch`.
func pairOfPeers(_ dispatch: @escaping (String, String, [Value]) async throws -> Value) async throws -> (host: Peer, runtime: Peer) {
    let (a, b) = try Channels.pair()
    var runtimeOptions = PeerOptions(endpoint: Endpoint(local: "swift", expected: "main"))
    runtimeOptions.dispatch = dispatch
    let runtime = try Peer(channel: a, options: runtimeOptions)
    var hostOptions = PeerOptions(endpoint: Endpoint(local: "main", expected: "swift"))
    hostOptions.host = true
    let host = try Peer(channel: b, options: hostOptions)
    try runtime.start()
    try host.start()
    await host.ready()
    await runtime.ready()
    return (host, runtime)
}

struct PeerTests {
    func testCallsAndCallbacks() async throws {
        let (host, runtime) = try await pairOfPeers { _, method, args in
            switch method {
            case "echo": return args.first ?? .undefined
            case "apply":
                guard case let .remoteFunction(f) = args[0] else { throw RutisError(name: "Error", message: "no function") }
                return try await f.call([args[1]], sync: true)
            case "chain": return .data(.array(CallChain.path.map(JSON.string)))
            default: throw RutisError(name: "Nope", message: "no \(method)")
            }
        }
        defer {
            host.close()
            runtime.close()
        }
        XCTAssertTrue(runtime.supports("reentrant-sync"))
        XCTAssertTrue(host.supports("reentrant-sync"))
        let echoed = try await host.call(target: "t", method: "echo", args: [.instant(Instant("2026-10-15T03:22:15.123456789Z")!)], sync: true)
        guard case let .instant(instant) = echoed else { return XCTFail("\(echoed)") }
        XCTAssertEqual(instant.description, "2026-10-15T03:22:15.123456789Z")
        let doubled = try await host.call(target: "t", method: "apply", args: [
            .function(SessionFunction { args in .data(.int((args[0].plainJSON?.int64Value ?? 0) * 2)) }), .data(.int(21)),
        ], sync: true)
        XCTAssertEqual(doubled.plainJSON, .number("42"))
        do {
            _ = try await host.call(target: "t", method: "x", args: [], sync: false)
            XCTFail("expected an error")
        } catch let error as RutisError {
            XCTAssertEqual(error.name, "Nope")
        }
        // The callee runs with the caller's chain and its own call id.
        let chain = try await CallChain.$path.withValue(["main:9"]) {
            try await host.call(target: "t", method: "chain", args: [], sync: false)
        }
        XCTAssertEqual(chain.plainJSON?.arrayValue?.first, .string("main:9"))
    }

    func testCancellingAnAsyncCallAbortsTheCallee() async throws {
        let started = Event()
        let aborted = Event()
        let (host, runtime) = try await pairOfPeers { _, _, _ in
            started.fire()
            do {
                try await Task.sleep(nanoseconds: 30_000_000_000)
            } catch {
                aborted.fire()
            }
            return .undefined
        }
        defer {
            host.close()
            runtime.close()
        }
        let call = Task { try await host.call(target: "t", method: "wait", args: [], sync: false) }
        await started.wait()
        call.cancel()
        let result = await call.result
        XCTAssertThrowsError(try result.get()) { XCTAssertTrue($0 is CancellationError) }
        let heard = await aborted.wait(seconds: 5)
        XCTAssertTrue(heard, "the callee's Task is cancelled")
    }

    func testAValueTooDeepFailsOnlyItsCall() async throws {
        let (host, runtime) = try await pairOfPeers { _, _, args in args.first ?? .undefined }
        defer {
            host.close()
            runtime.close()
        }
        var deep = JSON.null
        for _ in 0 ..< 65 { deep = .array([deep]) }
        do {
            _ = try await host.call(target: "t", method: "echo", args: [.data(deep)], sync: true)
            XCTFail("expected an error")
        } catch {
            XCTAssertTrue("\(error)".contains("nested deeper than 64"), "\(error)")
        }
        let fine = try await host.call(target: "t", method: "echo", args: [.data(.string("ok"))], sync: true)
        XCTAssertEqual(fine.plainJSON, .string("ok"))
    }

    func testTheSessionEndsWhenTheFarEndGoes() async throws {
        let (host, runtime) = try await pairOfPeers { _, _, _ in .undefined }
        host.close()
        host.channel.close()
        await runtime.closed()
        XCTAssertNotNil(runtime.error)
    }

    /// A far end speaking protocol 3 (rutis 0.18) is refused at its hello,
    /// with which end to upgrade.
    func testAFarEndOfProtocol3IsRefused() async throws {
        let (a, far) = try Channels.pair()
        let runtime = try Peer(channel: a, options: PeerOptions(endpoint: Endpoint(local: "swift", expected: "main")))
        try runtime.start()
        defer { far.close() }
        try far.send(Array(#"{"op":"hello","version":3,"endpoint":"main","implementation":{"name":"rutis","version":"0.18.0"},"capabilities":[]}"#.utf8))
        await runtime.closed()
        XCTAssertEqual("\(runtime.error.map { "\($0)" } ?? "")",
                       "incompatible session: the far end speaks protocol 3, this side 4: upgrade the far end (rutis 0.18.0)")
    }

    /// A read that fails ends the session and closes its channel: nothing
    /// reads it any more, and its descriptor would stay open (#542).
    func testAReadErrorClosesTheChannel() async throws {
        let channel = FailingChannel()
        let peer = try Peer(channel: channel, options: PeerOptions(endpoint: Endpoint(local: "swift", expected: "main")))
        try peer.start()
        await peer.closed()
        XCTAssertTrue("\(peer.error.map { "\($0)" } ?? "")".contains("read failed"), "\(String(describing: peer.error))")
        for _ in 0 ..< 200 where !channel.isClosed { try await Task.sleep(nanoseconds: 10_000_000) }
        XCTAssertTrue(channel.isClosed, "the channel is still open after a read error")
    }
}

/// A channel whose every read fails.
final class FailingChannel: Channel, @unchecked Sendable {
    private let lock = NSLock()
    private var closed = false
    var isClosed: Bool { lock.withLock { closed } }
    func send(_ message: [UInt8]) throws {}
    func receive() throws -> [UInt8]? { throw ChannelError("read failed") }
    func close() { lock.withLock { closed = true } }
}
