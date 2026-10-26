import Foundation
@testable import Rutis

/// An object with methods: it crosses only to a far end that takes objects.
final class Gadget: SessionObject, @unchecked Sendable {
    var identity: ObjectIdentifier { ObjectIdentifier(self) }
    func call(method: String, args: [Value]) async throws -> Value { .data(.int(1)) }
    func get(property: String) async throws -> Value { .undefined }
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

private func send(_ channel: Channel, _ text: String) throws {
    try channel.send(Array(text.utf8))
}

/// #675: what a call cannot send (a message over 16 MiB, an object with
/// methods to a far end that does not take objects) fails only that call,
/// on the side that would send it, naming the call, the argument or the
/// result and why; an object reference received fails only its call; a
/// relayed thrown value that is not an error crosses whole. The session
/// goes on.
struct BadValueTests {
    func testAMessageOverTheLimitFailsOnlyItsCall() async throws {
        let over = maxMessage + 10
        let limit = "over the limit of 16777216 bytes (16 MiB)"
        let (host, runtime) = try await pairOfPeers { _, method, args in
            switch method {
            case "big":
                return .data(.string(String(repeating: "x", count: Int(args[0].plainJSON?.int64Value ?? 0))))
            case "applyBig":
                guard case let .remoteFunction(f) = args[0] else { throw RutisError(name: "Error", message: "no function") }
                let text = String(repeating: "x", count: Int(args[1].plainJSON?.int64Value ?? 0))
                return try await f.call([.data(.string(text))], sync: true)
            default:
                return args.first ?? .undefined
            }
        }
        defer {
            host.close()
            runtime.close()
        }
        // An argument this side would send.
        do {
            _ = try await host.call(target: "host:t\0scope", method: "echo",
                                    args: [.data(.int(1)), .data(.string(String(repeating: "x", count: over)))], sync: true)
            XCTFail("expected an error")
        } catch {
            let message = "\(error)"
            XCTAssertTrue(message.hasPrefix("argument 2 of t.echo: the message would be ") && message.hasSuffix(limit), message)
        }
        // A result the far end would send.
        do {
            _ = try await host.call(target: "t", method: "big", args: [.data(.int(Int64(over)))], sync: true)
            XCTFail("expected an error")
        } catch let error as RutisError {
            XCTAssertTrue(error.message.hasPrefix("the result of t.big: the message would be ") && error.message.hasSuffix(limit), error.message)
        }
        // An argument the far end would send to a function of this side,
        // which never runs.
        let called = Event()
        let callback = SessionFunction { _ in
            called.fire()
            return .undefined
        }
        do {
            _ = try await host.call(target: "t", method: "applyBig", args: [.function(callback), .data(.int(Int64(over)))], sync: true)
            XCTFail("expected an error")
        } catch let error as RutisError {
            XCTAssertTrue(error.message.hasPrefix("argument 1 of a function: the message would be ") && error.message.hasSuffix(limit), error.message)
        }
        XCTAssertFalse(called.isSet, "the function ran")
        let fine = try await host.call(target: "t", method: "echo", args: [.data(.string("after"))], sync: true)
        XCTAssertEqual(fine.plainJSON, .string("after"))
        XCTAssertNil(host.error)
        XCTAssertNil(runtime.error)
    }

    func testAnObjectGoesOnlyToAFarEndThatTakesObjects() async throws {
        let (a, b) = try Channels.pair()
        var runtimeOptions = PeerOptions()
        runtimeOptions.endpoint = Endpoint(local: "swift", expected: "main")
        runtimeOptions.dispatch = { _, method, args in method == "object" ? .object(Gadget()) : args.first ?? .undefined }
        let runtime = try Peer(channel: a, options: runtimeOptions)
        var hostOptions = PeerOptions()
        hostOptions.endpoint = Endpoint(local: "main", expected: "swift")
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
            _ = try await host.call(target: "t", method: "object", args: [], sync: true)
            XCTFail("expected an error")
        } catch let error as RutisError {
            XCTAssertEqual(error.message, "the result of t.object: an object with methods (Gadget) cannot cross to main; pass plain data or a function instead")
        }
        let fine = try await host.call(target: "t", method: "echo", args: [.data(.string("after"))], sync: true)
        XCTAssertEqual(fine.plainJSON, .string("after"))
        XCTAssertEqual(runtime.referenceTables.exports, 0, "the object is left exported")
    }

    func testAReceivedObjectFailsOnlyItsCall() async throws {
        let refused = "received an object with methods, which Swift cannot take; pass plain data or a function instead"
        func reference(_ id: Int) -> String {
            #"{"type":"reference","value":{"id":\#(id),"kind":"object","home":false,"origin":[]}}"#
        }
        let (a, far) = try Channels.pair()
        var options = PeerOptions()
        options.dispatch = { _, _, args in args.first ?? .undefined }
        let runtime = try Peer(channel: a, options: options)
        try runtime.start()
        defer {
            runtime.close()
            far.close()
        }
        let hello = try await next(far)
        XCTAssertEqual(hello["op"]?.stringValue, "hello")
        try send(far, #"{"op":"hello","version":2,"capabilities":["events-v2"]}"#)
        await runtime.ready()

        // As a call's argument: the call is answered with a throw, and the
        // reference is given back.
        try send(far, #"{"op":"invoke","id":"rust:1","path":[],"target":"t","method":"echo","args":{"type":"list","value":["# + reference(7) + "]}}")
        var frames: [String: JSON] = [:]
        for _ in 0 ..< 2 {
            let frame = try await next(far)
            frames[frame["op"]?.stringValue ?? ""] = frame
        }
        XCTAssertEqual(frames["release"]?["reference"]?.int64Value, 7)
        XCTAssertEqual(frames["release"]?["count"]?.int64Value, 1)
        XCTAssertEqual(frames["throw"]?["error"]?["message"]?.stringValue, refused)

        // As a reply: the call waiting for it fails.
        let call = Task { try await runtime.call(target: "t", method: "make", args: [], sync: true) }
        let invoke = try await next(far)
        XCTAssertEqual(invoke["op"]?.stringValue, "invoke")
        let id = invoke["id"]?.stringValue ?? ""
        try send(far, #"{"op":"return","id":"\#(id)","value":"# + reference(8) + "}")
        let result = await call.result
        XCTAssertThrowsError(try result.get()) { XCTAssertEqual("\($0)", refused) }
        let release = try await next(far)
        XCTAssertEqual(release["reference"]?.int64Value, 8)

        // The session goes on.
        try send(far, #"{"op":"invoke","id":"rust:2","path":[],"target":"t","method":"echo","args":{"type":"data","value":["after"]}}"#)
        let reply = try await next(far)
        XCTAssertEqual(reply["op"]?.stringValue, "return")
        XCTAssertNil(runtime.error)
    }

    func testARelayedNonErrorThrowKeepsItsGraph() async throws {
        let (host, runtime) = try await pairOfPeers { _, _, args in
            guard case let .remoteFunction(f) = args[0] else { throw RutisError(name: "Error", message: "no function") }
            return try await f.call([], sync: true)
        }
        defer {
            host.close()
            runtime.close()
        }
        // A JavaScript `throw 'a string'`, as it crosses.
        let graph = JSON.object([
            ("root", .object([("type", .string("data")), ("value", .string("a string"))])),
            ("nodes", .array([])),
        ])
        var thrown = RutisError(name: "ThrownValue", message: "a string")
        thrown.graph = graph
        let thrower = SessionFunction { [thrown] _ in throw thrown }
        do {
            _ = try await host.call(target: "t", method: "rethrow", args: [.function(thrower)], sync: true)
            XCTFail("expected an error")
        } catch let error as RutisError {
            XCTAssertEqual(error.name, "ThrownValue")
            XCTAssertEqual(error.message, "a string")
            XCTAssertEqual(error.graph, graph)
        }
        XCTAssertNil(runtime.error)
    }
}
