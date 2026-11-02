// Unloading runs one stack in reverse: cleanups and the withdrawals of the
// services a row provides, in the order they were registered (Rust's
// drain_effects; docs/design-unload-order-2026-11-01.md).
import Foundation
import RutisTest
@testable import Rutis

/// A service of the test's own.
@RutisService
final class Counter: @unchecked Sendable {
    let n: Int
    init(_ n: Int) { self.n = n }
    func count() -> Int { n }
}

/// A value a plugin's closure writes and the test reads.
final class Cell<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: T?
    var value: T? {
        get { lock.withLock { stored } }
        set { lock.withLock { stored = newValue } }
    }
}

struct UnloadOrderTests {
    /// What reading `tally` through `ctx` gives: the count, or the error's code.
    private static func read(_ ctx: Rutis.Ctx) async -> String {
        do { return "count \(try await ctx.use("tally").call("count", values: [], as: Int.self))" }
        catch let error as RutisError { return "error \(error.code ?? "nil")" }
        catch { return "error \(error)" }
    }

    private static func runtime(log: @escaping @Sendable (String) -> Void = Events.standardError) -> Runtime {
        Runtime([], project: "", settings: Events.Settings(log: log))
    }

    private static func load(_ runtime: Runtime, _ key: String, _ definition: Rutis.Definition) async throws {
        try await runtime.load(key: key, name: key, entry: key, definition: definition, config: .undefined,
                               exports: [:], isolate: [:], inject: [], scope: [])
    }

    /// A cleanup registered after `provide` reads the service; one registered
    /// before runs after the withdrawal and no longer counts it as its own.
    func aCleanupRegisteredAfterProvideReadsIt() async throws {
        let lines = Lines()
        let plugin = Rutis.define(Plugin(name: "order") { ctx in
            ctx.effect {
                let seen = await Self.read(ctx)
                lines.add("before: \(seen)")
            }
            ctx.provide("tally", Counter(7))
            ctx.effect {
                let seen = await Self.read(ctx)
                lines.add("after: \(seen)")
            }
        })
        let runtime = Self.runtime()
        try await Self.load(runtime, "o", plugin)
        try await runtime.unload("o")
        XCTAssertEqual(lines.all, ["after: count 7", "before: error not-declared"])
    }

    /// Another row loses what a row provides when that row starts unloading;
    /// the row itself still reads it then.
    func anotherRowLosesItWhenTheUnloadStarts() async throws {
        let lines = Lines()
        let user = Cell<Rutis.Ctx>()
        let provider = Rutis.define(Plugin(name: "provider") { ctx in
            ctx.provide("tally", Counter(2))
            // Registered after the provide: it runs before the withdrawal.
            ctx.effect {
                let own = await Self.read(ctx)
                lines.add("own: \(own)")
                guard let reader = user.value else { return }
                let other = await Self.read(reader)
                lines.add("other: \(other)")
            }
        })
        let using = Rutis.define(Plugin(name: "using", inject: ["tally"]) { ctx in user.value = ctx })
        let runtime = Self.runtime()
        try await Self.load(runtime, "p", provider)
        try await Self.load(runtime, "u", using)
        let before = await Self.read(try XCTUnwrap(user.value))
        XCTAssertEqual(before, "count 2")
        try await runtime.unload("p")
        XCTAssertEqual(lines.all, ["own: count 2", "other: error unavailable"])
        let after = await Self.read(try XCTUnwrap(user.value))
        XCTAssertEqual(after, "error unavailable")
    }

    /// A cleanup registering a cleanup is refused, as Rust's InactiveEffect.
    func aCleanupRegisteringOneIsRefused() async throws {
        let lines = Lines()
        let ran = Cell<Bool>()
        ran.value = false
        let plugin = Rutis.define(Plugin(name: "nested") { ctx in
            ctx.effect { ctx.effect { ran.value = true } }
        })
        let runtime = Self.runtime(log: { lines.add($0) })
        try await Self.load(runtime, "nested", plugin)
        try await runtime.unload("nested")
        XCTAssertFalse(ran.value ?? true, "the cleanup registered while unloading ran")
        XCTAssertEqual(lines.all, ["rutis: row nested is not active: cleanup is not registered"])
    }

    /// Withdrawing by the returned function and providing again with an equal
    /// value: the older withdrawal leaves the newer registration alone.
    func anOldWithdrawalLeavesANewRegistrationAlone() async throws {
        let lines = Lines()
        let plugin = Rutis.define(Plugin(name: "again") { ctx in
            let counter = Counter(5)
            let withdraw = ctx.provide("tally", counter)
            withdraw()
            ctx.provide("tally", counter)
            withdraw()
            ctx.effect {
                let during = await Self.read(ctx)
                lines.add("during: \(during)")
            }
        })
        let runtime = Self.runtime()
        try await Self.load(runtime, "a", plugin)
        try await runtime.unload("a")
        XCTAssertEqual(lines.all, ["during: count 5"])
    }

    /// A row's unload stack still emits (events protocol §7.1): the other
    /// listeners hear it, while the row's own left when the unload started.
    /// Once rutis answered `rows.unload`, a context kept past the row emits
    /// nothing (§3).
    func aRowsCleanupStillEmits() async throws {
        let late = Cell<Rutis.Ctx>()
        let plugin = Rutis.define(Plugin(name: "closing") { ctx in
            late.value = ctx
            ctx.effect { try ctx.emit("t/bye", "bye") }
        })
        let loaded = try await RutisTest.load(plugin)
        defer { Task { await loaded.close() } }
        let heard = Cell<Any?>()
        await loaded.on("t/bye") { (value: Any?) in
            heard.value = value
        }
        try await loaded.unload()
        for _ in 0..<100 where heard.value == nil {
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        XCTAssertEqual(heard.value as? String, "bye")
        let error = Self.failure { try late.value?.emit("t/bye", "late") }
        XCTAssertEqual(error, "BindingError: row closing is not loaded in this runtime")
    }

    /// What `body` fails with, as `name: message`, or "no error".
    private static func failure(_ body: () throws -> Void) -> String {
        do {
            try body()
            return "no error"
        } catch let error as RutisError {
            return "\(error.name): \(error.message)"
        } catch {
            return "\(error)"
        }
    }
}
