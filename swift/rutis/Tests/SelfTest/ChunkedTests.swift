// guarantee: bridge/large-and-deep-values
import Foundation
@testable import Rutis

/// A count the dispatch of a test may bump from several calls.
private final class Tally: @unchecked Sendable {
    private let lock = Lock()
    private var count = 0

    func increment() -> Int {
        lock.lock()
        defer { lock.unlock() }
        count += 1
        return count
    }

    var value: Int {
        lock.lock()
        defer { lock.unlock() }
        return count
    }
}

/// The next message of `channel`, read off the cooperative threads.
private func next(_ channel: Channel) async throws -> JSON {
    let message: [UInt8] = try await withCheckedThrowingContinuation { continuation in
        DispatchQueue.global().async {
            do {
                guard let message = try channel.receive() else { throw ChannelError("the channel ended") }
                continuation.resume(returning: message)
            } catch {
                continuation.resume(throwing: error)
            }
        }
    }
    return try JSONReader.parse(message)
}

private func send(_ channel: Channel, _ frame: JSON) throws {
    try channel.send(JSONWriter.write(frame))
}

/// One `chunk` frame around `data`.
private func chunk(_ transfer: String, seq: Int, last: Bool, _ data: [UInt8]) -> JSON {
    .object([
        ("op", .string("chunk")),
        ("transfer", .string(transfer)),
        ("seq", .int(seq)),
        ("last", .bool(last)),
        ("data", .string(Data(data).base64EncodedString())),
    ])
}

/// A session whose far end is written frame by frame here: the runtime side
/// is `Peer`, the far end is the other half of a socket pair. `dispatch`
/// serves the runtime's incoming calls; the far end's greeting declares
/// `capabilities`.
private func scriptedPeer(maxTransfer: Int? = nil, capabilities: [String] = ["chunked"],
                          dispatch: @escaping (String, String, [Value]) async throws -> Value = { _, _, _ in .undefined })
    async throws -> (peer: Peer, far: Channel)
{
    let (a, b) = try Channels.pair()
    var options = PeerOptions(endpoint: Endpoint(local: "swift", expected: "main"))
    options.maxTransfer = maxTransfer
    options.dispatch = dispatch
    let peer = try Peer(channel: a, options: options)
    try peer.start()
    let greeting = try await next(b)
    XCTAssertEqual(greeting["op"]?.stringValue, "hello")
    XCTAssertTrue(greeting["capabilities"]?.arrayValue?.compactMap(\.stringValue).contains("chunked") == true)
    try send(b, .object([
        ("op", .string("hello")),
        ("version", .int(endpointProtocol)),
        ("endpoint", .string("main")),
        ("capabilities", .array(capabilities.map(JSON.string))),
    ]))
    await peer.ready()
    return (peer, b)
}

/// An `invoke` frame of `method` with `args`.
private func invoke(_ id: String, _ method: String, _ args: [JSON] = []) -> JSON {
    .object([
        ("op", .string("invoke")),
        ("id", .string(id)),
        ("path", .array([])),
        ("target", .string("t")),
        ("method", .string(method)),
        ("args", .object([("type", .string("list")), ("value", .array(args))])),
    ])
}

private func text(_ value: String) -> JSON {
    .object([("type", .string("data")), ("value", .string(value))])
}

private func integer(_ value: Int64) -> JSON {
    .object([("type", .string("data")), ("value", .int(value))])
}

struct ChunkedTests {
    /// A parameter and a result over the channel's limit cross whole, and
    /// each call runs once (#1363, as Rust and Node).
    func testValuesOverTheLimitCrossInChunks() async throws {
        let tally = Tally()
        let (host, runtime) = try await pairOfPeers { _, method, args in
            switch method {
            case "tally":
                guard case let .data(.string(text)) = args[0] else {
                    throw RutisError(name: "Error", message: "tally needs a string")
                }
                return .data(.object([("calls", .int(tally.increment())), ("size", .int(text.utf8.count))]))
            case "big":
                guard case let .data(json) = args[0], let size = json.int64Value else {
                    throw RutisError(name: "Error", message: "big needs an integer")
                }
                return .data(.string(String(repeating: "x", count: Int(size))))
            default:
                return args.first ?? .undefined
            }
        }
        defer {
            host.close()
            runtime.close()
        }
        XCTAssertTrue(host.supports("chunked"))
        XCTAssertTrue(runtime.supports("chunked"))
        let size = maxMessage + 4096
        let big = String(repeating: "x", count: size)
        let first = try await host.call(target: "t", method: "tally", args: [.data(.string(big))], sync: true)
        XCTAssertEqual(first.plainJSON?["calls"]?.int64Value, 1, "the argument arrived whole, once")
        XCTAssertEqual(first.plainJSON?["size"]?.int64Value, Int64(size))
        let second = try await host.call(target: "t", method: "tally", args: [.data(.string(big))], sync: true)
        XCTAssertEqual(second.plainJSON?["calls"]?.int64Value, 2, "the second call ran once, whole")
        let back = try await host.call(target: "t", method: "big", args: [.data(.int(size))], sync: true)
        guard case let .data(.string(result)) = back else { return XCTFail("a result, not \(back)") }
        XCTAssertEqual(result.utf8.count, size, "the result came back whole")
        let after = try await host.call(target: "t", method: "tally", args: [.data(.string("after"))], sync: true)
        XCTAssertEqual(after.plainJSON?["calls"]?.int64Value, 3)
        XCTAssertNil(host.error)
        XCTAssertNil(runtime.error)
    }

    /// The receiver buffers a transfer and dispatches only after its last
    /// chunk (#1363): no reply, and no call, before it.
    func testAChunkedCallIsDispatchedOnlyAfterItsLastChunk() async throws {
        let tally = Tally()
        let (peer, far) = try await scriptedPeer { _, method, args in
            guard method == "tally", case let .data(.string(text)) = args[0] else { return .undefined }
            return .data(.object([("calls", .int(tally.increment())), ("size", .int(text.utf8.count))]))
        }
        defer {
            peer.close()
            far.close()
        }
        let bytes = JSONWriter.write(invoke("main:1", "tally", [text("a value in one frame")]))
        let size = max(1, bytes.count / 3)
        var at = 0
        var seq = 0
        while at + size < bytes.count {
            let end = at + size
            try send(far, chunk("main:1", seq: seq, last: false, Array(bytes[at ..< end])))
            at = end
            seq += 1
        }
        // Nothing runs before the last chunk arrives.
        try await Task.sleep(nanoseconds: 100_000_000)
        XCTAssertEqual(tally.value, 0, "a call ran before its last chunk")
        try send(far, chunk("main:1", seq: seq, last: true, Array(bytes[at...])))
        let reply = try await next(far)
        XCTAssertEqual(reply["op"]?.stringValue, "return")
        XCTAssertEqual(reply["value"]?["value"]?["calls"]?.int64Value, 1)
        XCTAssertEqual(tally.value, 1, "the call ran once")
        XCTAssertNil(peer.error)
    }

    /// A `seq` out of order ends the session, and the call never runs.
    func testAChunkOutOfOrderEndsTheSession() async throws {
        let (peer, far) = try await scriptedPeer()
        defer {
            peer.close()
            far.close()
        }
        let bytes = JSONWriter.write(invoke("main:1", "echo", [text("x")]))
        try send(far, chunk("main:1", seq: 1, last: true, bytes))
        await peer.closed()
        XCTAssertTrue("\(peer.error.map { "\($0)" } ?? "")".contains("chunked transfer main:1: expected chunk 0, got 1"),
                      "\(String(describing: peer.error))")
    }

    /// A second transfer before the first ends ends the session: one
    /// unfinished transfer at a time (#1363).
    func testASecondTransferBeforeTheFirstEndsEndsTheSession() async throws {
        let (peer, far) = try await scriptedPeer()
        defer {
            peer.close()
            far.close()
        }
        let bytes = JSONWriter.write(invoke("main:1", "echo", [text("x")]))
        try send(far, chunk("main:1", seq: 0, last: false, bytes))
        try send(far, chunk("main:2", seq: 0, last: true, bytes))
        await peer.closed()
        XCTAssertTrue("\(peer.error.map { "\($0)" } ?? "")".contains("chunked transfer main:1: transfer main:2 started before it ended"),
                      "\(String(describing: peer.error))")
    }

    /// A transfer over the session's cap ends the session, naming the cap.
    func testATransferOverTheSessionsCapEndsTheSession() async throws {
        let (peer, far) = try await scriptedPeer(maxTransfer: 1024)
        defer {
            peer.close()
            far.close()
        }
        try send(far, chunk("main:1", seq: 0, last: true, [UInt8](repeating: 0, count: 2048)))
        await peer.closed()
        XCTAssertTrue("\(peer.error.map { "\($0)" } ?? "")".contains("chunked transfer main:1: over the limit of 1024 bytes"),
                      "\(String(describing: peer.error))")
    }

    /// A transfer cut off by the channel is discarded: the call never runs.
    func testAnIncompleteTransferIsNeverDispatched() async throws {
        let tally = Tally()
        let (peer, far) = try await scriptedPeer { _, _, _ in
            _ = tally.increment()
            return .undefined
        }
        let bytes = JSONWriter.write(invoke("main:1", "tally", [text(String(repeating: "x", count: 64))]))
        try send(far, chunk("main:1", seq: 0, last: false, Array(bytes.prefix(bytes.count / 2))))
        try await Task.sleep(nanoseconds: 100_000_000)
        far.close()
        await peer.closed()
        XCTAssertEqual(tally.value, 0, "an unfinished transfer was dispatched")
        XCTAssertNotNil(peer.error)
    }

    /// A far end that did not declare `chunked` may not send `chunk` frames.
    func testAChunkWithoutTheCapabilityEndsTheSession() async throws {
        let (peer, far) = try await scriptedPeer(capabilities: [])
        defer {
            peer.close()
            far.close()
        }
        try send(far, chunk("main:1", seq: 0, last: true, Array("{}".utf8)))
        await peer.closed()
        XCTAssertTrue("\(peer.error.map { "\($0)" } ?? "")".contains("chunk frame from a far end that did not declare chunked"),
                      "\(String(describing: peer.error))")
    }

    /// A `chunk` before the greeting is refused as any other frame is.
    func testAChunkBeforeTheHandshakeEndsTheSession() async throws {
        let (a, far) = try Channels.pair()
        let peer = try Peer(channel: a, options: PeerOptions(endpoint: Endpoint(local: "swift", expected: "main")))
        try peer.start()
        defer {
            peer.close()
            far.close()
        }
        _ = try await next(far)
        try send(far, chunk("main:1", seq: 0, last: true, Array("{}".utf8)))
        await peer.closed()
        XCTAssertTrue("\(peer.error.map { "\($0)" } ?? "")".contains("request before protocol handshake"),
                      "\(String(describing: peer.error))")
    }

    /// A slice that is not canonical base64 ends the session.
    func testAChunkThatIsNotBase64EndsTheSession() async throws {
        let (peer, far) = try await scriptedPeer()
        defer {
            peer.close()
            far.close()
        }
        try send(far, .object([
            ("op", .string("chunk")), ("transfer", .string("main:1")), ("seq", .int(0)),
            ("last", .bool(true)), ("data", .string("not base64!")),
        ]))
        await peer.closed()
        XCTAssertTrue("\(peer.error.map { "\($0)" } ?? "")".contains("chunked transfer main:1: invalid bytes (11 characters)"),
                      "\(String(describing: peer.error))")
    }

    /// A chunk may not wrap another chunk.
    func testANestedChunkEndsTheSession() async throws {
        let (peer, far) = try await scriptedPeer()
        defer {
            peer.close()
            far.close()
        }
        let inner = JSONWriter.write(chunk("main:2", seq: 0, last: true, Array("{}".utf8)))
        try send(far, chunk("main:1", seq: 0, last: true, inner))
        await peer.closed()
        XCTAssertTrue("\(peer.error.map { "\($0)" } ?? "")".contains("chunk frame inside a chunked transfer"),
                      "\(String(describing: peer.error))")
    }

    /// Without `chunked` on the far end, an oversized result keeps the
    /// channel's limit and fails only its call (#1363): nothing is chunked.
    func testTheFarEndWithoutChunkedKeepsTheMessageLimit() async throws {
        let size = maxMessage + 4096
        let (peer, far) = try await scriptedPeer(capabilities: []) { _, method, args in
            guard method == "big", case let .data(json) = args[0], let bytes = json.int64Value else { return .undefined }
            return .data(.string(String(repeating: "x", count: Int(bytes))))
        }
        defer {
            peer.close()
            far.close()
        }
        try send(far, invoke("main:1", "big", [integer(Int64(size))]))
        let reply = try await next(far)
        XCTAssertEqual(reply["op"]?.stringValue, "throw", "a chunk frame went to a far end that did not declare chunked")
        let message = reply["error"]?["message"]?.stringValue ?? ""
        XCTAssertTrue(message.contains("over the limit of 16777216 bytes (16 MiB)"), message)
        // The session goes on.
        XCTAssertNil(peer.error)
    }

    /// The session's cap binds even when the channel would take the frame:
    /// over it only that call fails, naming the cap (#1363).
    func testAFrameOverTheConfiguredCapFailsOnlyItsCall() async throws {
        let (a, b) = try Channels.pair()
        var runtimeOptions = PeerOptions(endpoint: Endpoint(local: "swift", expected: "main"))
        runtimeOptions.maxTransfer = 4096
        runtimeOptions.dispatch = { _, _, args in args.first ?? .undefined }
        let runtime = try Peer(channel: a, options: runtimeOptions)
        var hostOptions = PeerOptions(endpoint: Endpoint(local: "main", expected: "swift"))
        hostOptions.maxTransfer = 4096
        let host = try Peer(channel: b, options: hostOptions)
        try runtime.start()
        try host.start()
        await host.ready()
        await runtime.ready()
        defer {
            host.close()
            runtime.close()
        }
        do {
            _ = try await host.call(target: "t", method: "echo",
                                    args: [.data(.string(String(repeating: "x", count: 8192)))], sync: true)
            XCTFail("expected an error")
        } catch {
            XCTAssertTrue("\(error)".contains("over the limit of 4096 bytes"), "\(error)")
            let (code, data, _) = codeAndData(error)
            XCTAssertEqual(code, "bad-value")
            let fields = data as? [String: Any]
            XCTAssertEqual(fields?["limit"] as? Int, 4096)
            XCTAssertEqual(fields?["part"] as? String, "argument")
        }
        let fine = try await host.call(target: "t", method: "echo", args: [.data(.string("after"))], sync: true)
        XCTAssertEqual(fine.plainJSON, .string("after"))
        XCTAssertNil(host.error)
        XCTAssertNil(runtime.error)
    }

    /// `RUTIS_MAX_TRANSFER` sets the session's cap; a value that is not a
    /// positive number is refused naming the variable.
    func testMaxTransferFromTheEnvironment() {
        XCTAssertEqual(try maxTransferFromEnvironment(nil), defaultMaxTransfer)
        XCTAssertEqual(try maxTransferFromEnvironment("1024"), 1024)
        for bad in ["abc", "0", "-1", "1.5"] {
            XCTAssertThrowsError(try maxTransferFromEnvironment(bad)) { error in
                XCTAssertEqual("\(error)", "RUTIS_MAX_TRANSFER \"\(bad)\": expected a positive number of bytes")
            }
        }
    }
}
