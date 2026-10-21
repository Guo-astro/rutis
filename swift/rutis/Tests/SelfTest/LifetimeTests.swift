// What outlives a session (#389): invocations of a listener queued behind a
// running one when the session ends find the runtime and the test host gone
// and end, instead of reading freed memory ("Attempted to read an unowned
// reference but object … was already destroyed"); and a channel closed
// between two reads never reads, writes or shuts down its descriptor's
// number again, which by then may be a new session's socket (the new
// session then never heard the hello, and hung).
import Foundation
@testable import Rutis
@testable import RutisTest

struct LifetimeTests {
    /// Emits queue behind listeners (the runtime's and the test side's)
    /// still running when the test host closes and everything is released;
    /// sessions keep starting meanwhile. The process lives on, and every
    /// load completes.
    func queuedListenerInvocationsOutliveTheRuntime() async throws {
        let plugin = Rutis.define(Plugin(name: "slow") { ctx in
            ctx.on("t/slow") { () async throws in
                try await Task.sleep(nanoseconds: 100_000_000)
                throw Slow()
            }
        })
        for _ in 0..<20 {
            var loaded: Loaded? = try await RutisTest.load(plugin)
            await loaded!.on("t/slow") { () async throws in
                try await Task.sleep(nanoseconds: 100_000_000)
                throw Slow()
            }
            for _ in 0..<5 { try await loaded!.emit("t/slow") }
            await loaded!.close()
            loaded = nil
        }
        // The queued invocations run now, with nothing left of the runtimes.
        try await Task.sleep(nanoseconds: 1_000_000_000)
    }

    /// A channel closed while nothing reads it: a later receive or send
    /// does not touch the descriptor's number, which a new pair now has.
    func aClosedChannelLeavesItsDescriptorsNumberAlone() throws {
        for _ in 0..<50 {
            let (old, oldFar) = try Channels.pair()
            old.close()
            oldFar.close()
            // Likely the same numbers again.
            let (new, newFar) = try Channels.pair()
            try newFar.send(Array("hello".utf8))
            XCTAssertNil(try old.receive(), "a closed channel reads nothing")
            XCTAssertThrowsError(try old.send(Array("stray".utf8)))
            XCTAssertEqual(try new.receive().map { String(decoding: $0, as: UTF8.self) }, "hello")
            new.close()
            newFar.close()
        }
    }
}

private struct Slow: Error {}
