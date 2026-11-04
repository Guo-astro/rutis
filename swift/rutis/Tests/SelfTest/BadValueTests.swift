// guarantee: bridge/bad-values
import Foundation
@testable import Rutis

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
/// result and why; an object received is a proxy of its methods; a
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
            XCTAssertTrue(message.hasPrefix("t.echo: argument 2 cannot be sent: the message would be ") && message.hasSuffix(limit), message)
            // guarantee: sdk/error-texts (code bad-value, data {argument, part, limit}: #677)
            let (code, data, _) = codeAndData(error)
            XCTAssertEqual(code, "bad-value")
            XCTAssertEqual((data as? [String: Any])?["argument"] as? Int, 2)
            XCTAssertEqual((data as? [String: Any])?["part"] as? String, "argument")
            XCTAssertEqual((data as? [String: Any])?["limit"] as? Int, maxMessage)
        }
        // A result the far end would send.
        do {
            _ = try await host.call(target: "t", method: "big", args: [.data(.int(Int64(over)))], sync: true)
            XCTFail("expected an error")
        } catch let error as RutisError {
            XCTAssertTrue(error.message.hasPrefix("t.big: the result cannot be sent: the message would be ") && error.message.hasSuffix(limit), error.message)
            XCTAssertEqual(error.code, "bad-value")
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
            XCTAssertTrue(error.message.hasPrefix("a function: argument 1 cannot be sent: the message would be ") && error.message.hasSuffix(limit), error.message)
        }
        XCTAssertFalse(called.isSet, "the function ran")
        let fine = try await host.call(target: "t", method: "echo", args: [.data(.string("after"))], sync: true)
        XCTAssertEqual(fine.plainJSON, .string("after"))
        XCTAssertNil(host.error)
        XCTAssertNil(runtime.error)
    }

    // guarantee: bridge/bad-values
    // A value that holds itself says so (design D3): found before the depth,
    // and an instance in two places is not a cycle.
    func testAValueThatHoldsItselfSaysSo() throws {
        final class Node: Codable {
            var name: String
            var next: Node?
            init(name: String, next: Node? = nil) {
                self.name = name
                self.next = next
            }
        }
        let looped = Node(name: "a")
        looped.next = looped
        do {
            _ = try ValueCodec.encodeAny(looped)
            XCTFail("a value that holds itself crossed")
        } catch let error as ValueError {
            XCTAssertEqual(error.description, "the value holds itself (a cycle)")
            XCTAssertEqual(error.reason, "cycle")
        }
        // The same instance in two places is not a cycle.
        let shared = Node(name: "b")
        let both = [shared, shared]
        let encoded = try ValueCodec.encodeAny(both)
        guard case let .list(items) = encoded, items.count == 2 else {
            return XCTFail("a list of two expected, got \(encoded)")
        }
        for item in items {
            guard case let .record(fields) = item else { return XCTFail("\(item)") }
            XCTAssertEqual(fields.map(\.0), ["name", "next"], "\(item)")
        }
        // The path is left behind: a second value encodes as the first did.
        _ = try ValueCodec.encodeAny(shared)
    }

    // guarantee: bridge/bad-values
    // What a value that cannot cross says (design §5.5, docs/design-value-
    // errors-2026-11-03.md): one sentence of reason, `bad-value`, and
    // `data.reason` for a program. The sentences are rutis's own; the reasons
    // are the closed names.
    func testAValueThatCannotCrossSaysWhy() throws {
        func refused(_ value: Any) -> ValueError? {
            do {
                _ = try ValueCodec.encodeAny(value)
                return nil
            } catch let error as ValueError {
                return error
            } catch {
                XCTFail("a ValueError expected, got \(error)")
                return nil
            }
        }
        for (value, says, reason) in [
            (Double.nan as Any, "the number NaN is not finite", "non-finite"),
            (-Double.infinity as Any, "the number -Infinity is not finite", "non-finite"),
        ] {
            guard let error = refused(value) else { return XCTFail("a number that is not finite crossed") }
            XCTAssertEqual(error.description, says)
            XCTAssertEqual(error.reason, reason)
        }
        var deep: Any = 1
        for _ in 0..<(maxDepth + 1) { deep = [deep] }
        guard let error = refused(deep) else { return XCTFail("a deep value crossed") }
        XCTAssertEqual(error.description, "the value nests deeper than \(maxDepth) levels")
        XCTAssertEqual(error.reason, "too-deep")
        // The reason reaches `data` when the value is an argument or a result
        // (design §4.3).
        let argument = ofArgument(error.description, 0, "t.echo", why: error.reason)
        XCTAssertEqual(argument.description, "t.echo: argument 1 cannot be sent: the value nests deeper than \(maxDepth) levels")
        XCTAssertEqual(argument.code, "bad-value")
        XCTAssertEqual(argument.data?["reason"] as? String, "too-deep")
        XCTAssertEqual(argument.data?["part"] as? String, "argument")
        XCTAssertEqual(argument.data?["argument"] as? Int, 1)
        let result = ofResult(error.description, "t.echo", why: error.reason)
        XCTAssertEqual(result.description, "t.echo: the result cannot be sent: the value nests deeper than \(maxDepth) levels")
        XCTAssertEqual(result.data?["reason"] as? String, "too-deep")
        XCTAssertEqual(result.data?["part"] as? String, "result")
    }

    // guarantee: bridge/object-refs
    // objects-case(swift): receiver-releases
    // objects-case(swift): undefined-nested-null
    func testAReceivedObjectIsAProxyOfItsMethods() async throws {
        func reference(_ id: Int) -> String {
            #"{"type":"reference","value":{"id":\#(id),"kind":"object","home":false,"origin":[]}}"#
        }
        let (a, far) = try Channels.pair()
        var options = PeerOptions(endpoint: Endpoint(local: "swift", expected: "main"))
        let kept = SavedValue()
        options.dispatch = { _, method, args in
            if method == "keep" {
                kept.value = .list(args)
                return .undefined
            }
            if method == "poke", case let .remoteObject(object)? = args.first {
                return try await object.call("get", [], sync: false)
            }
            return args.first ?? .undefined
        }
        let runtime = try Peer(channel: a, options: options)
        try runtime.start()
        defer {
            runtime.close()
            far.close()
        }
        let hello = try await next(far)
        XCTAssertEqual(hello["op"]?.stringValue, "hello")
        try send(far, #"{"op":"hello","version":4,"endpoint":"main","capabilities":[]}"#)
        await runtime.ready()

        // Sent back, the object goes home as itself; the runtime's hold on
        // it is given back.
        try send(far, #"{"op":"invoke","id":"main:1","path":[],"target":"t","method":"echo","args":{"type":"list","value":["# + reference(7) + "]}}")
        var frames: [String: JSON] = [:]
        for _ in 0 ..< 2 {
            let frame = try await next(far)
            frames[frame["op"]?.stringValue ?? ""] = frame
        }
        XCTAssertEqual(frames["return"]?["value"]?["value"]?["id"]?.int64Value, 7)
        XCTAssertEqual(frames["return"]?["value"]?["value"]?["home"], .bool(true))
        XCTAssertEqual(frames["release"]?["reference"]?.int64Value, 7)

        // A method call is a call frame naming the method.
        try send(far, #"{"op":"invoke","id":"main:2","path":[],"target":"t","method":"poke","args":{"type":"list","value":["# + reference(8) + "]}}")
        let call = try await next(far)
        XCTAssertEqual(call["op"]?.stringValue, "call")
        XCTAssertEqual(call["reference"]?.int64Value, 8)
        XCTAssertEqual(call["method"]?.stringValue, "get")
        let id = call["id"]?.stringValue ?? ""
        try send(far, #"{"op":"return","id":"\#(id)","value":{"type":"data","value":5}}"#)
        frames = [:]
        for _ in 0 ..< 2 {
            let frame = try await next(far)
            frames[frame["op"]?.stringValue ?? ""] = frame
        }
        XCTAssertEqual(frames["return"]?["value"]?["value"]?.int64Value, 5)
        XCTAssertEqual(frames["release"]?["reference"]?.int64Value, 8)

        // Undefined: kept as a whole argument, null inside a value.
        try send(far, #"{"op":"invoke","id":"main:3","path":[],"target":"t","method":"keep","args":{"type":"list","value":[{"type":"undefined"},{"type":"list","value":[{"type":"undefined"}]}]}}"#)
        _ = try await next(far)
        guard case let .list(args)? = kept.value, args.count == 2 else { return XCTFail("\(String(describing: kept.value))") }
        guard case .undefined = args[0] else { return XCTFail("a whole argument keeps undefined: \(args[0])") }
        XCTAssertEqual(args[1].plainJSON, .array([.null]), "inside a value, null")
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
