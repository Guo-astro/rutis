// The Swift owner and holder of the object cases
// (crates/rutis-loader/tests/objects_cross.rs, #687): the same as the
// JavaScript, Python and Go ones there. The owner hands its counter (a
// @RutisService instance, by reference) to the holder's service in another
// runtime; the holder keeps what it is given as a @RutisClient proxy.
import Foundation
import Rutis

/// The holder's parameters: an object of another process, as a proxy.
@RutisClient
public protocol Counted {
    func inc() async throws -> Int
    func value() async throws -> Int
    func nope() async throws
}

@RutisClient
public protocol Pinged {
    func ping() async throws -> String
}

@RutisClient
public protocol Poker {
    func poke(_ object: any Counted) async throws -> Int
}

@RutisClient
public protocol Recorder {
    func record(_ line: String) async throws
}

public enum Objects {
    /// The owner's object.
    @RutisService
    public final class Counter: @unchecked Sendable {
        private let lock = NSLock()
        var n = 0
        public func inc() -> Int { lock.withLock { n += 1; return n } }
        public func value() -> Int { lock.withLock { n } }
    }

    @RutisService
    public final class Pinger: @unchecked Sendable {
        public func ping() -> String { "pong" }
    }

    /// The holder's service as the owner uses it.
    @RutisClient
    public protocol HolderService {
        func take(_ counter: Objects.Counter) async throws -> String
        func inc() async throws -> Int
        func listed(_ items: [Any?]) async throws -> String
        func back() async throws -> Any?
        func rust() async throws -> Int
        func nope() async throws -> String
        func plain(_ thing: Objects.Pinger) async throws -> String
    }

    /// The `owner` service.
    @RutisService
    public final class Owner: @unchecked Sendable {
        let ctx: Rutis.Ctx
        let made = Counter()
        init(ctx: Rutis.Ctx) { self.ctx = ctx }

        public func run() async throws -> String {
            let holder = try ctx.use("holder", HolderServiceProxy.self)
            _ = try await holder.take(made)
            _ = try await holder.inc()
            _ = try await holder.inc()
            let listed = try await holder.listed([["data": 1], made])
            let home = (try await holder.back() as AnyObject?) === made
            let viaRust = try await holder.rust()
            let missing = try await holder.nope()
            let plain = try await holder.plain(Pinger())
            return "\(made.value()) \(listed) \(home) \(viaRust) \(plain) | \(missing)"
        }

        public func announce() throws -> String {
            try ctx.emit("counter", made)
            return "sent"
        }

        /// Ends this runtime's process, as a crash does.
        public func die() -> String {
            DispatchQueue.global().asyncAfter(deadline: .now() + .milliseconds(20)) { exit(1) }
            return "dying"
        }
    }

    /// `<code>: <message>`, as the holder answers an error.
    static func said(_ error: Error) -> String {
        if let error = error as? RutisError { return "\(error.code ?? ""): \(error.message)" }
        return "\(error)"
    }

    /// The `holder` service: what it is given is a proxy.
    @RutisService
    public final class Holder: @unchecked Sendable {
        let poker: any Poker
        var kept: (any Counted)?
        init(poker: any Poker) { self.poker = poker }

        public func take(_ counter: any Counted) -> String {
            kept = counter
            return "taken"
        }

        public func inc() async throws -> Int { try await kept!.inc() }

        public func listed(_ items: [Any?]) -> String {
            let data = (items[0] as? [String: Any?])?["data"] ?? nil
            let given = items[1] as? Rutis.Client, held = (kept as? any RutisProxy)?.rutisClient
            let same = given != nil && given === held
            return "{\"data\":\((data as? Int) ?? -1)} \(same)"
        }

        public func back() -> any Counted { kept! }

        public func rust() async throws -> Int { try await poker.poke(kept!) }

        public func nope() async -> String {
            do {
                try await kept!.nope()
                return "answered"
            } catch {
                return Objects.said(error)
            }
        }

        public func plain(_ thing: any Pinged) async throws -> String { try await thing.ping() }

        public func after() async -> String {
            do {
                return String(try await kept!.inc())
            } catch {
                return Objects.said(error)
            }
        }
    }
}

public let swiftOwner = Rutis.define(Plugin(
    name: "swift_owner",
    inject: ["holder"],
    apply: { ctx in ctx.provide("owner", Objects.Owner(ctx: ctx)) }))

public let swiftHolder = Rutis.define(Plugin(
    name: "swift_holder",
    inject: ["poker", "probe"],
    apply: { ctx in
        let holder = Objects.Holder(poker: try ctx.use("poker", PokerProxy.self))
        let probe = try ctx.use("probe", RecorderProxy.self)
        ctx.on("counter") { (counter: Rutis.Client) in
            let held = (holder.kept as? any RutisProxy)?.rutisClient
            let same = held != nil && counter === held
            let n: Int = try await counter.call("value", [], as: Int.self)
            try await probe.record("event: \(same) \(n)")
        }
        ctx.provide("holder", holder)
    }))
