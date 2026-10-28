import Foundation
@testable import Rutis

/// The reply to an `emit` through the host (events protocol §5.5): taken as
/// its frame is read, before the next frame; a 0.13 host's future answer
/// still settles. A runtime session against a scripted host.
struct EmitReplyTests {
    /// A runtime session whose far end the test writes and reads by hand.
    private func scripted(_ dispatch: @escaping (String, String, [Value]) async throws -> Value) async throws -> (Peer, LineChannel) {
        let (a, b) = try Channels.pair()
        var options = PeerOptions(endpoint: Endpoint(local: "swift", expected: "main"))
        options.dispatch = dispatch
        let runtime = try Peer(channel: a, options: options)
        try runtime.start()
        _ = try await next(b) // its hello
        try send(b, #"{"op":"hello","version":4,"endpoint":"main","capabilities":["events-v2"]}"#)
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

    private func id(_ frame: JSON) -> String { "\"" + (frame["id"]?.stringValue ?? "") + "\"" }

    func theReplyIsToldBeforeTheNextFrameIsHandled() async throws {
        let answered = Event()
        let seen = Box<Bool?>(nil)
        let handled = Event()
        let (runtime, far) = try await scripted { _, _, _ in
            seen.set(answered.isSet)
            handled.fire()
            return .undefined
        }
        defer {
            runtime.close()
            far.close()
        }
        let told = Box<Result<Value, Error>?>(nil)
        try runtime.sendAnswered(target: "", method: "events.dispatch", args: [.data(.object([("mode", .string("emit"))]))]) { result in
            told.set(result)
            answered.fire()
        }
        let sent = try await next(far)
        XCTAssertEqual(sent["method"]?.stringValue, "events.dispatch")
        // The reply, then at once a frame the runtime handles after it.
        try send(far, #"{"op":"return","id":"# + id(sent) + #","value":{"type":"undefined"}}"#)
        try send(far, #"{"op":"invoke","id":"main:1","path":[],"target":"","method":"after","args":{"type":"list","value":[]}}"#)
        let wasHandled = await handled.wait(seconds: 5)
        XCTAssertTrue(wasHandled, "the frame after the reply was not handled")
        XCTAssertEqual(seen.get(), true, "the frame after the reply was handled before the reply was told")
        guard case .success = told.get() else { return XCTFail("the reply was told as \(String(describing: told.get()))") }
    }

    func aFutureAnswerOfA013HostSettles() async throws {
        let (runtime, far) = try await scripted { _, _, _ in .undefined }
        defer {
            runtime.close()
            far.close()
        }
        let answered = Event()
        let told = Box<Result<Value, Error>?>(nil)
        try runtime.sendAnswered(target: "", method: "events.dispatch", args: [.data(.object([("mode", .string("emit"))]))]) { result in
            told.set(result)
            answered.fire()
        }
        let sent = try await next(far)
        try send(far, #"{"op":"return","id":"# + id(sent) + #","value":{"type":"reference","value":{"id":1,"kind":"future","home":false,"origin":[]}}}"#)
        let wasAnswered = await answered.wait(seconds: 5)
        XCTAssertTrue(wasAnswered)
        guard case let .success(value) = told.get(), case .remoteFuture = value else {
            return XCTFail("expected a future, got \(String(describing: told.get()))")
        }
        // What the events runtime does with it: settle it.
        let settled = Task { try await settle(value) }
        let waited = try await next(far)
        XCTAssertEqual(waited["op"]?.stringValue, "await")
        XCTAssertEqual(waited["reference"]?.int64Value, 1)
        try send(far, #"{"op":"return","id":"# + id(waited) + #","value":{"type":"undefined"}}"#)
        let result = await settled.result
        XCTAssertNoThrow(try result.get())
    }
}

/// A value shared with callbacks.
private final class Box<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: T
    init(_ value: T) { self.value = value }
    func set(_ value: T) { lock.withLock { self.value = value } }
    func get() -> T { lock.withLock { value } }
}
