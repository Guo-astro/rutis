import Foundation
@testable import Rutis

/// A session in this process: `host` drives `runtime`, which serves
/// `dispatch`.
func pairOfPeers(maxTransfer: Int? = nil, _ dispatch: @escaping (String, String, [Value]) async throws -> Value) async throws -> (host: Peer, runtime: Peer) {
    let (a, b) = try Channels.pair()
    var runtimeOptions = PeerOptions(endpoint: Endpoint(local: "swift", expected: "main"))
    runtimeOptions.dispatch = dispatch
    runtimeOptions.maxTransfer = maxTransfer
    let runtime = try Peer(channel: a, options: runtimeOptions)
    var hostOptions = PeerOptions(endpoint: Endpoint(local: "main", expected: "swift"))
    hostOptions.host = true
    hostOptions.maxTransfer = maxTransfer
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

    /// The session ending cancels the calls running then, as a `cancel` frame
    /// does one call: the reader keeps its hands off them and the far end's
    /// handlers stop.
    func testTheSessionEndingCancelsTheCallsRunning() async throws {
        let started = Event()
        let cancelled = Event()
        let (host, runtime) = try await pairOfPeers { _, _, _ in
            started.fire()
            do { try await Task.sleep(nanoseconds: 30_000_000_000) } catch { cancelled.fire() }
            return .undefined
        }
        defer {
            host.close()
            runtime.close()
        }
        let call = Task { try await host.call(target: "t", method: "wait", args: [], sync: false) }
        let began = await started.wait(seconds: 5)
        XCTAssertTrue(began, "the call reached its handler")
        // Its Task is attached right after the call starts; a moment is enough
        // for the attach to be behind us.
        try await Task.sleep(nanoseconds: 50_000_000)
        runtime.close()
        let heard = await cancelled.wait(seconds: 5)
        XCTAssertTrue(heard, "the call running when the session ended is cancelled")
        call.cancel()
        _ = await call.result
    }

    /// A call that takes its id just as the session ends is cancelled too
    /// (#630): `close()` cancels the calls it saw, and this one's Task is
    /// attached after that snapshot — it must not run on as a call nobody can
    /// hear.
    func testACallAttachingItsTaskAsTheSessionClosesIsCancelled() async throws {
        let (a, b) = try Channels.pair()
        let arrived = Event()
        let attaching = DispatchSemaphore(value: 0)
        let cancelled = Event()
        let ran = Event()
        var runtimeOptions = PeerOptions(endpoint: Endpoint(local: "swift", expected: "main"))
        // The reading thread between a call's id and its Task: where the close
        // lands, and the call has no Task to cancel yet (#630).
        runtimeOptions.arrive = { method, _ in
            guard method == "hold" else { return nil }
            arrived.fire()
            attaching.wait()
            return {
                do {
                    try await Task.sleep(nanoseconds: 200_000_000)
                } catch {
                    cancelled.fire()
                    throw error
                }
                ran.fire()
                return .undefined
            }
        }
        let runtime = try Peer(channel: a, options: runtimeOptions)
        var hostOptions = PeerOptions(endpoint: Endpoint(local: "main", expected: "swift"))
        hostOptions.host = true
        let host = try Peer(channel: b, options: hostOptions)
        try runtime.start()
        try host.start()
        await host.ready()
        await runtime.ready()
        defer {
            host.close()
            runtime.channel.close()
            host.channel.close()
        }
        let call = Task { try await host.call(target: "", method: "hold", args: [], sync: false) }
        let reached = await arrived.wait(seconds: 5)
        XCTAssertTrue(reached, "the call reached the point before its Task is made")
        runtime.close()
        attaching.signal()
        _ = await cancelled.wait(seconds: 5)
        XCTAssertTrue(cancelled.isSet, "the call whose Task is attached as the session closed is cancelled")
        XCTAssertFalse(ran.isSet, "it does not run after the session closed")
        call.cancel()
        _ = await call.result
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
            XCTAssertTrue("\(error)".contains("the value nests deeper than 64 levels"), "\(error)")
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
                       "incompatible session: the far end speaks protocol 3, this side 5: upgrade the far end (rutis 0.18.0)")
    }

    /// A read that fails ends the session and closes its channel: nothing
    /// reads it any more, and its descriptor would stay open (#542).
    func testAReadErrorClosesTheChannel() async throws {
        let channel = FailingChannel()
        let peer = try Peer(channel: channel, options: PeerOptions(endpoint: Endpoint(local: "swift", expected: "main")))
        // The reading thread can meet the error before the hello goes out, and
        // the session is then already closed: `start()` throws, and what this
        // test is about is the channel below (TSan, #630).
        try? peer.start()
        await peer.closed()
        XCTAssertTrue("\(peer.error.map { "\($0)" } ?? "")".contains("read failed"), "\(String(describing: peer.error))")
        for _ in 0 ..< 200 where !channel.isClosed { try await Task.sleep(nanoseconds: 10_000_000) }
        XCTAssertTrue(channel.isClosed, "the channel is still open after a read error")
    }

    /// A plugin emits inside a Task: with a far end that stopped reading, the
    /// emit queues its frames and returns, so no write is made on the thread
    /// the tasks run on (#630). One emitter: more blocked emits than the pool
    /// has threads would hold up this very wait, and a test that hangs says
    /// less than one that fails.
    func testAnEmitDoesNotWaitForTheWrite() async throws {
        let seen = Recorder()
        let (host, runtime, channel) = try await stallingPair { _, method, _ in
            seen.add(method)
            return .undefined
        }
        defer {
            host.close()
            runtime.close()
            channel.close()
        }
        channel.hold()
        let queued = Event()
        let emitter = Task {
            for index in 0 ..< 8 { host.notify(target: "t", method: "m\(index)", args: []) }
            queued.fire()
        }
        let returned = await queued.wait(seconds: 10)
        XCTAssertTrue(returned, "an emit waited for its write")
        XCTAssertEqual(seen.methods, [], "a frame was written while the far end read nothing")
        // A write is really held: the emits returned while it waits.
        let held = await channel.held.wait(seconds: 5)
        XCTAssertTrue(held, "no write was held")
        // Let go whatever the emits did: a test that hung could not say what
        // went wrong.
        channel.unhold()
        _ = await emitter.value
        for _ in 0 ..< 500 where seen.methods.count < 8 { try await Task.sleep(nanoseconds: 10_000_000) }
        XCTAssertEqual(seen.methods.count, 8, "queued frames went missing: \(seen.methods)")
    }

    /// The frames a session took while the far end read nothing go out in the
    /// order they were taken (#630).
    func testQueuedFramesGoOutInOrder() async throws {
        let seen = Recorder()
        let (host, runtime, channel) = try await stallingPair { _, method, _ in
            seen.add(method)
            return .undefined
        }
        defer {
            host.close()
            runtime.close()
            channel.close()
        }
        channel.hold()
        let queued = Event()
        let emitter = Task {
            for method in ["one", "two", "three"] { host.notify(target: "t", method: method, args: []) }
            queued.fire()
        }
        let returned = await queued.wait(seconds: 10)
        XCTAssertTrue(returned, "emitting waited for its write")
        XCTAssertEqual(seen.methods, [])
        channel.unhold()
        _ = await emitter.value
        for _ in 0 ..< 500 where seen.methods.count < 3 { try await Task.sleep(nanoseconds: 10_000_000) }
        XCTAssertEqual(seen.methods, ["one", "two", "three"])
    }

    /// `close()` writes the frames it had taken before it returns: a process
    /// that ends there, as a runtime's does, loses none (#630).
    func testClosingWritesWhatWasQueuedBeforeItReturns() async throws {
        let seen = Recorder()
        let (host, runtime, channel) = try await stallingPair { _, method, _ in
            seen.add(method)
            return .undefined
        }
        defer {
            host.close()
            runtime.close()
            channel.close()
        }
        let before = channel.written.count
        // A far end that reads, but slowly: each write takes a while.
        channel.slow(100)
        let queued = Event()
        let emitter = Task {
            for method in ["one", "two", "three"] { host.notify(target: "t", method: method, args: []) }
            queued.fire()
        }
        let returned = await queued.wait(seconds: 10)
        XCTAssertTrue(returned, "emitting waited for its write")
        host.close()
        XCTAssertEqual(channel.written.count - before, 3, "close returned with frames still queued")
        _ = await emitter.value
        for _ in 0 ..< 500 where seen.methods.count < 3 { try await Task.sleep(nanoseconds: 10_000_000) }
        XCTAssertEqual(seen.methods, ["one", "two", "three"])
    }

    /// A write that fails ends the session: its frame was taken already, and
    /// the emitter only queued it (#630).
    func testAWriteThatFailsEndsTheSession() async throws {
        let seen = Recorder()
        let (host, runtime, channel) = try await stallingPair { _, method, _ in
            seen.add(method)
            return .undefined
        }
        defer {
            host.close()
            runtime.close()
            channel.close()
        }
        channel.hold()
        channel.failWrites()
        let queued = Event()
        let emitter = Task {
            host.notify(target: "t", method: "x", args: [])
            queued.fire()
        }
        let held = await channel.held.wait(seconds: 5)
        XCTAssertTrue(held, "no write reached the channel")
        let returned = await queued.wait(seconds: 10)
        XCTAssertTrue(returned, "emitting waited for its write")
        channel.unhold()
        _ = await emitter.value
        for _ in 0 ..< 500 where !host.isClosed { try await Task.sleep(nanoseconds: 10_000_000) }
        XCTAssertTrue(host.isClosed, "the session went on after a failed write")
        XCTAssertTrue("\(host.error.map { "\($0)" } ?? "")".contains("the far end stopped reading"),
                      "\(String(describing: host.error))")
        XCTAssertEqual(seen.methods, [])
    }
}

/// A session whose host writes through `channel`, which the test can hold: a
/// far end that stopped reading (#630). `dispatch` serves the runtime side,
/// where the frames land once they are let through.
func stallingPair(_ dispatch: @escaping (String, String, [Value]) async throws -> Value)
    async throws -> (host: Peer, runtime: Peer, channel: StallingChannel)
{
    let (a, b) = try Channels.pair()
    let channel = StallingChannel(b)
    var runtimeOptions = PeerOptions(endpoint: Endpoint(local: "swift", expected: "main"))
    runtimeOptions.dispatch = dispatch
    runtimeOptions.maxTransfer = maxTransfer
    let runtime = try Peer(channel: a, options: runtimeOptions)
    var hostOptions = PeerOptions(endpoint: Endpoint(local: "main", expected: "swift"))
    hostOptions.host = true
    hostOptions.maxTransfer = maxTransfer
    let host = try Peer(channel: channel, options: hostOptions)
    try runtime.start()
    try host.start()
    await host.ready()
    await runtime.ready()
    return (host, runtime, channel)
}

/// The methods the runtime side was called with, in order.
final class Recorder: @unchecked Sendable {
    private let lock = Lock()
    private var seen: [String] = []
    func add(_ method: String) { lock.withLock { seen.append(method) } }
    var methods: [String] { lock.withLock { seen } }
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
