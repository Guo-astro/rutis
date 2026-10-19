import Foundation
@testable import Rutis
import XCTest

/// A compat session in this process: `host` drives `runtime`, which serves
/// `dispatch`.
func pairOfPeers(_ dispatch: @escaping (String, String, [Value]) async throws -> Value) async throws -> (host: Peer, runtime: Peer) {
    let (a, b) = try Channels.pair()
    var runtimeOptions = PeerOptions()
    runtimeOptions.dispatch = dispatch
    let runtime = try Peer(channel: a, options: runtimeOptions)
    var hostOptions = PeerOptions()
    hostOptions.host = true
    let host = try Peer(channel: b, options: hostOptions)
    try runtime.start()
    try host.start()
    await host.ready()
    await runtime.ready()
    return (host, runtime)
}

final class PeerTests: XCTestCase {
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
        XCTAssertTrue(runtime.supports("sync-wait"))
        XCTAssertTrue(host.supports("events-v2"))
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
        let chain = try await CallChain.$path.withValue(["node:9"]) {
            try await host.call(target: "t", method: "chain", args: [], sync: false)
        }
        XCTAssertEqual(chain.plainJSON?.arrayValue?.first, .string("node:9"))
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
}
