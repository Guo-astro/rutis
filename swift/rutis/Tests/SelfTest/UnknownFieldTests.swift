// guarantee: bridge/unknown-fields
import Foundation
@testable import Rutis

/// A field this side does not know (session protocol §9, #635): a call frame
/// fails only its call with a `bad-frame` throw, a reply fails only the call
/// waiting for it, `release`/`cancel` ignore the extras, and a greeting's
/// fails the handshake. The session goes on in every case but the greeting's.
struct UnknownFieldTests {
    /// A runtime session whose far end the test writes and reads by hand.
    private func scripted(_ dispatch: @escaping (String, String, [Value]) async throws -> Value) async throws -> (Peer, LineChannel) {
        let (a, b) = try Channels.pair()
        var options = PeerOptions(endpoint: Endpoint(local: "swift", expected: "main"))
        options.dispatch = dispatch
        let runtime = try Peer(channel: a, options: options)
        try runtime.start()
        _ = try await next(b) // its hello
        try send(b, #"{"op":"hello","version":5,"endpoint":"main","capabilities":[]}"#)
        await runtime.ready()
        return (runtime, b)
    }

    private func send(_ far: LineChannel, _ frame: String) throws { try far.send(Array(frame.utf8)) }

    /// The next frame the runtime sent, read off the test's task.
    private func next(_ far: LineChannel) async throws -> JSON {
        try await withCheckedThrowingContinuation { continuation in
            Thread {
                do {
                    guard let message = try far.receive() else { throw ChannelError("the runtime closed the session") }
                    continuation.resume(returning: try JSONReader.parse(message))
                } catch {
                    continuation.resume(throwing: error)
                }
            }.start()
        }
    }

    /// The error a throw frame carries, read as the far end reads it.
    private func thrown(_ frame: JSON) -> RutisError {
        decodeError(frame["error"] ?? .null, decodeData: { $0 })
    }

    /// The `data` of an error as JSON data. An error that crossed the wire
    /// carries a `Value`; one made on this side carries the plain
    /// `[String: Any]` its constructor was given.
    private func data(_ error: RutisError) -> JSON? {
        guard let carried = error.data else { return nil }
        if let value = carried as? Value { return value.plainJSON }
        guard let object = carried as? [String: Any] else { return nil }
        let fields = (object["fields"] as? [String] ?? []).map(JSON.string)
        return .object([("fields", .array(fields)), ("op", .string(object["op"] as? String ?? ""))])
    }

    /// `{"op": op, "fields": fields}`, as the data of a `bad-frame` error.
    private func refused(_ op: String, _ fields: String...) -> JSON {
        .object([("fields", .array(fields.map(JSON.string))), ("op", .string(op))])
    }

    // A call frame with a field this side does not know fails only that
    // call: a throw with code bad-frame, the field's path, and the op; a
    // field inside the arguments' value is named by its path.
    func aCallWithAnUnknownFieldFailsOnlyItsCall() async throws {
        let (runtime, far) = try await scripted { _, _, args in args.first ?? .undefined }
        defer {
            runtime.close()
            far.close()
        }
        try send(far, #"{"op":"invoke","id":"main:1","path":[],"target":"t","method":"echo","args":{"type":"data","value":[]},"trace":"x"}"#)
        let top = try await next(far)
        XCTAssertEqual(top["op"]?.stringValue, "throw")
        XCTAssertEqual(top["id"]?.stringValue, "main:1")
        let topError = thrown(top)
        XCTAssertEqual(topError.name, "BindingError")
        XCTAssertEqual(topError.code, "bad-frame")
        XCTAssertEqual(topError.message, #"unknown field "trace" in invoke"#)
        XCTAssertEqual(data(topError), refused("invoke", "trace"))

        try send(far, #"{"op":"invoke","id":"main:2","path":[],"target":"t","method":"echo","args":{"type":"record","value":{"x":{"type":"undefined","extra":1}}}}"#)
        let nested = try await next(far)
        XCTAssertEqual(nested["op"]?.stringValue, "throw")
        XCTAssertEqual(nested["id"]?.stringValue, "main:2")
        let nestedError = thrown(nested)
        XCTAssertEqual(nestedError.code, "bad-frame")
        XCTAssertEqual(nestedError.message, #"unknown field "args.value.x.extra" in invoke"#)
        XCTAssertEqual(data(nestedError), refused("invoke", "args.value.x.extra"))

        // Every call op says which one it is.
        try send(far, #"{"op":"await","id":"main:3","path":[],"reference":1,"extra":1}"#)
        let awaited = try await next(far)
        XCTAssertEqual(awaited["op"]?.stringValue, "throw")
        XCTAssertEqual(awaited["id"]?.stringValue, "main:3")
        XCTAssertEqual(thrown(awaited).message, #"unknown field "extra" in await"#)

        // The session goes on: a call without one is served.
        try send(far, #"{"op":"invoke","id":"main:4","path":[],"target":"t","method":"echo","args":{"type":"data","value":["after"]}}"#)
        let served = try await next(far)
        XCTAssertEqual(served["op"]?.stringValue, "return")
        XCTAssertEqual(served["id"]?.stringValue, "main:4")
        XCTAssertEqual(served["value"]?["value"]?.stringValue, "after")
        XCTAssertFalse(runtime.isClosed)
    }

    // A reply with a field this side does not know fails the call waiting
    // for it, with code bad-frame and the op of the reply; the session goes
    // on.
    func aReplyWithAnUnknownFieldFailsOnlyItsCall() async throws {
        let (runtime, far) = try await scripted { _, _, args in args.first ?? .undefined }
        defer {
            runtime.close()
            far.close()
        }
        // A return with an extra field: the call fails, the value is not read.
        let first = Task { try await runtime.call(target: "t", method: "echo", args: [.data(.string("one"))], sync: false) }
        let firstInvoke = try await next(far)
        let firstID = firstInvoke["id"]?.stringValue ?? ""
        try send(far, #"{"op":"return","id":"\#(firstID)","value":{"type":"data","value":"one"},"extra":1}"#)
        let firstResult = await first.result
        guard case let .failure(failure) = firstResult, let error = failure as? RutisError else {
            return XCTFail("expected a refusal, got \(String(describing: try? firstResult.get()))")
        }
        XCTAssertEqual(error.code, "bad-frame")
        XCTAssertEqual(error.message, #"unknown field "extra" in return"#)
        XCTAssertEqual(data(error), refused("return", "extra"))

        // A throw with an extra field: the reply's own error is not read.
        let second = Task { try await runtime.call(target: "t", method: "echo", args: [], sync: false) }
        let secondInvoke = try await next(far)
        let secondID = secondInvoke["id"]?.stringValue ?? ""
        try send(far, #"{"op":"throw","id":"\#(secondID)","error":{"name":"E","message":"boom"},"extra":1}"#)
        let secondResult = await second.result
        guard case let .failure(failure) = secondResult, let refusal = failure as? RutisError else {
            return XCTFail("expected a refusal, got \(String(describing: try? secondResult.get()))")
        }
        XCTAssertEqual(refusal.name, "BindingError")
        XCTAssertEqual(refusal.code, "bad-frame")
        XCTAssertEqual(refusal.message, #"unknown field "extra" in throw"#)
        XCTAssertEqual(data(refusal), refused("throw", "extra"))

        // The next call is served as usual.
        let third = Task { try await runtime.call(target: "t", method: "echo", args: [.data(.string("after"))], sync: false) }
        let thirdInvoke = try await next(far)
        let thirdID = thirdInvoke["id"]?.stringValue ?? ""
        try send(far, #"{"op":"return","id":"\#(thirdID)","value":{"type":"data","value":"after"}}"#)
        let echoed = try await third.value
        XCTAssertEqual(echoed.plainJSON, .string("after"))
        XCTAssertFalse(runtime.isClosed)
    }

    // `release` and `cancel` have no call to fail: their extra fields are
    // ignored and the frame is handled by its known fields.
    func cancelAndReleaseIgnoreUnknownFields() async throws {
        let started = Event()
        let aborted = Event()
        let (runtime, far) = try await scripted { _, method, args in
            switch method {
            case "make": return .function(SessionFunction { _ in .undefined })
            case "wait":
                started.fire()
                do {
                    try await Task.sleep(nanoseconds: 30_000_000_000)
                } catch {
                    aborted.fire()
                }
                return .undefined
            default: return args.first ?? .undefined
            }
        }
        defer {
            runtime.close()
            far.close()
        }
        // The far end takes a function of this side, to release it.
        try send(far, #"{"op":"invoke","id":"main:1","path":[],"target":"t","method":"make","args":{"type":"data","value":[]}}"#)
        let made = try await next(far)
        XCTAssertEqual(made["op"]?.stringValue, "return")
        guard let reference = made["value"]?["value"]?["id"]?.int64Value else {
            return XCTFail("no reference: \(made)")
        }
        XCTAssertEqual(runtime.referenceTables.exports, 1)

        // A release with an extra field still gives the reference back: the
        // call after it shows the frame was handled.
        try send(far, #"{"op":"release","reference":\#(reference),"count":1,"extra":1}"#)
        try send(far, #"{"op":"invoke","id":"main:2","path":[],"target":"t","method":"echo","args":{"type":"data","value":[]}}"#)
        let echoed = try await next(far)
        XCTAssertEqual(echoed["op"]?.stringValue, "return")
        XCTAssertEqual(echoed["id"]?.stringValue, "main:2")
        XCTAssertEqual(runtime.referenceTables.exports, 0)

        // A cancel with an extra field still cancels the call it names.
        try send(far, #"{"op":"invoke","id":"main:3","path":[],"target":"t","method":"wait","args":{"type":"data","value":[]}}"#)
        let didStart = await started.wait(seconds: 5)
        XCTAssertTrue(didStart, "the call did not start")
        try send(far, #"{"op":"cancel","id":"main:3","extra":1}"#)
        let didAbort = await aborted.wait(seconds: 5)
        XCTAssertTrue(didAbort, "the call was not cancelled")
        let cancelled = try await next(far)
        XCTAssertEqual(cancelled["op"]?.stringValue, "return")
        XCTAssertEqual(cancelled["id"]?.stringValue, "main:3")

        // And the session is still serving calls.
        try send(far, #"{"op":"invoke","id":"main:4","path":[],"target":"t","method":"echo","args":{"type":"data","value":["after"]}}"#)
        let after = try await next(far)
        XCTAssertEqual(after["op"]?.stringValue, "return")
        XCTAssertEqual(after["id"]?.stringValue, "main:4")
        XCTAssertFalse(runtime.isClosed)
    }

    // A greeting with a field this side does not know fails the handshake:
    // the session ends, saying so.
    func anUnknownFieldInHelloFailsTheHandshake() async throws {
        let (a, far) = try Channels.pair()
        let runtime = try Peer(channel: a, options: PeerOptions(endpoint: Endpoint(local: "swift", expected: "main")))
        try runtime.start()
        defer { far.close() }
        try send(far, #"{"op":"hello","version":5,"endpoint":"main","extra":1}"#)
        await runtime.closed()
        XCTAssertEqual("\(runtime.error.map { "\($0)" } ?? "")",
                       #"incompatible session: unknown field "extra" in hello"#)
    }
}
