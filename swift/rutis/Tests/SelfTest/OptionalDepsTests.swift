// Optional services (docs/design-optional-deps-2026-10-28.md): declared in
// `optional` beside `inject`, read with `ctx.get`, which gives nil when rutis
// had none when the row started; `use` counts them as declared; `get` of a
// name declared nowhere says to add it to `optional`; a handle whose row
// stopped says the service is not available. RutisTest gives the optional
// services the test gives, and only those. The cross-language cases:
// crates/rutis-loader/tests/optional_deps.rs.
import Foundation
@testable import Rutis
@testable import RutisTest
import RutisFixturePlugins

/// An `llm` that answers (RutisFixturePlugins' `Llm`).
@RutisService
final class Oracle {
    func ask() -> String { "42" }
}

/// A service without the method `LlmProxy` calls.
@RutisService
final class Mute {
    func hush() -> String { "" }
}

@RutisService
final class Flip {
    let side = "heads"
    func toss() -> String { side }
}

/// What a plugin saw in `apply`.
final class Noted: @unchecked Sendable {
    private let lock = NSLock()
    private var lines: [String: String] = [:]
    subscript(key: String) -> String? {
        get { lock.withLock { lines[key] } }
        set { lock.withLock { lines[key] = newValue } }
    }
}

struct OptionalDepsTests {
    /// An error as `code: message data`.
    static func described(_ error: Error) -> String {
        guard let error = error as? RutisError else { return "not a RutisError: \(error)" }
        let data = error.data.flatMap { (try? ValueCodec.encodeAny($0))?.plainJSON }.map(JSONWriter.string) ?? "nil"
        return "\(error.code ?? "nil"): \(error.message) \(data)"
    }

    static func failure(_ body: () throws -> Any?) -> String {
        do {
            _ = try body()
            return "no error"
        } catch {
            return described(error)
        }
    }

    static func asyncFailure(_ body: () async throws -> Any?) async -> String {
        do {
            _ = try await body()
            return "no error"
        } catch {
            return described(error)
        }
    }

    // guarantee: sdk/optional-deps (get present / absent / undeclared; use
    // reads only what inject lists, as Rust's `require`: an optional name,
    // there or not, is told to go in inject)
    func getReadsAnOptionalServiceWhenThere() async throws {
        let definition = Rutis.define(Plugin(name: "summary", inject: ["notes"], optional: ["llm", "absent"]) { _ in })
        let runtime = Runtime([], project: "")
        runtime.lock.withLock { runtime.hosts["llm"] = ["ask": "sync"] }
        let row = Row(key: "row:1.0", name: "summary", entry: "summary", definition: definition, config: .undefined,
                      exports: [:], isolate: [:], scope: [])
        let ctx = Rutis.Ctx(runtime: runtime, row: row)
        XCTAssertEqual(try ctx.get("llm")?.methods, ["ask": "sync"])
        XCTAssertNotNil(try ctx.get("llm", LlmProxy.self))
        XCTAssertEqual(Self.failure { try ctx.use("llm") },
                       #"not-declared: service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject {"service":"llm"}"#)
        XCTAssertNil(try ctx.get("absent"))
        XCTAssertNil(try ctx.get("absent", LlmProxy.self), "absent: no method check")
        XCTAssertEqual(Self.failure { try ctx.use("absent") },
                       #"not-declared: service absent is not available to this plugin, which does not declare it in inject: add 'absent' to its inject {"service":"absent"}"#)
        XCTAssertEqual(Self.failure { try ctx.get("other") },
                       #"not-declared: service other is not available to this plugin, which does not declare it: add 'other' to its optional (or to its inject, if it cannot run without it) {"service":"other"}"#)
        XCTAssertEqual(Self.failure { try ctx.get("other", LlmProxy.self) },
                       #"not-declared: service other is not available to this plugin, which does not declare it: add 'other' to its optional (or to its inject, if it cannot run without it) {"service":"other"}"#)
        // `use`'s line for a name declared nowhere is unchanged.
        XCTAssertEqual(Self.failure { try ctx.use("other") },
                       #"not-declared: service other is not available to this plugin, which does not declare it in inject: add 'other' to its inject {"service":"other"}"#)
    }

    // guarantee: sdk/optional-deps (a row's own `inject` counts as declared:
    // the plugin uses it without listing it, and it is unavailable rather
    // than not-declared when nothing provides it)
    func aRowsInjectCountsAsDeclared() throws {
        let definition = Rutis.define(Plugin(name: "reader") { _ in })
        let runtime = Runtime([], project: "")
        runtime.lock.withLock {
            runtime.hosts["row"] = ["greet": "sync"]
            runtime.hosts["neighbour"] = ["greet": "sync"]
        }
        let row = Row(key: "row:1.0", name: "reader", entry: "reader", definition: definition, config: .undefined,
                      exports: [:], isolate: [:], inject: ["row", "search"], scope: [])
        let ctx = Rutis.Ctx(runtime: runtime, row: row)
        XCTAssertNotNil(try ctx.use("row"), "use finds a service in the row's inject")
        XCTAssertNotNil(try ctx.get("row"), "get finds a service in the row's inject")
        XCTAssertEqual(Self.failure { try ctx.use("search") }, #"unavailable: service search is not available {"service":"search"}"#)
        // Declared nowhere, though this process has it: say what to declare.
        XCTAssertEqual(Self.failure { try ctx.use("neighbour") },
                       #"not-declared: service neighbour is not available to this plugin, which does not declare it in inject: add 'neighbour' to its inject {"service":"neighbour"}"#)
        XCTAssertEqual(Self.failure { try ctx.get("neighbour") },
                       #"not-declared: service neighbour is not available to this plugin, which does not declare it: add 'neighbour' to its optional (or to its inject, if it cannot run without it) {"service":"neighbour"}"#)
    }

    // guarantee: sdk/optional-deps (the row's `inject` is rows.load's fifth
    // argument, as rutis sends it, and a restarted row keeps it)
    func aRowsInjectArrivesWithRowsLoad() async throws {
        let lines = Lines()
        let reader = Rutis.define(Plugin(name: "reader") { ctx in
            for name in ["row", "search", "other"] { lines.add(Self.failure { try ctx.use(name) }) }
            lines.add((try? ctx.get("row")) != nil ? "get row" : "no row")
        })
        let runtime = Runtime([reader], project: "")
        runtime.lock.withLock { runtime.hosts["row"] = ["greet": "sync"] }
        _ = try await runtime.control("rows.load", [
            .data(.string("row:1.0")), .data(.string("reader")), .undefined, .data(.array([])),
            .data(.array([.string("row"), .string("search")])), .data(.object([])),
            .data(.object([("scope", .array([])), ("id", .string("reader"))])),
        ])
        _ = try await runtime.control("rows.update", [.data(.string("row:1.0")), .undefined])
        let once = [
            "no error",
            #"unavailable: service search is not available {"service":"search"}"#,
            #"not-declared: service other is not available to this plugin, which does not declare it in inject: add 'other' to its inject {"service":"other"}"#,
            "get row",
        ]
        XCTAssertEqual(lines.all, once + once)
    }

    // guarantee: sdk/optional-deps (a name the plugin provides itself counts
    // as declared while it provides it; withdrawn, it is like an undeclared
    // one, as for a Rust plugin's strict read)
    func aPluginReadsWhatItProvidesWhileItProvidesIt() async throws {
        let lines = Lines()
        let local = Rutis.define(Plugin(name: "local") { ctx in
            let withdraw = ctx.provide("local", Oracle())
            lines.add(Self.failure { try ctx.use("local") })
            lines.add((try? ctx.get("local")) != nil ? "get local" : "no local")
            withdraw()
            lines.add(Self.failure { try ctx.use("local") })
            lines.add(Self.failure { try ctx.get("local") })
        })
        let runtime = Runtime([local], project: "")
        try await runtime.load(key: "a", name: nil, entry: "local", config: .undefined, exports: [:], isolate: [:], scope: [])
        XCTAssertEqual(lines.all, [
            "no error",
            "get local",
            #"not-declared: service local is not available to this plugin, which does not declare it in inject: add 'local' to its inject {"service":"local"}"#,
            #"not-declared: service local is not available to this plugin, which does not declare it: add 'local' to its optional (or to its inject, if it cannot run without it) {"service":"local"}"#,
        ])
    }

    // guarantee: sdk/optional-deps (the manifest and rows.schema report
    // `optional`, [] when left out)
    func theManifestReportsOptional() throws {
        let summary = Rutis.define(Plugin(name: "summary", inject: ["notes"], optional: ["llm"]) { _ in })
        let plain = Rutis.define(Plugin(name: "plain") { _ in })
        let manifest = try JSONReader.parse(Rutis.Advanced.manifest([summary, plain]))
        XCTAssertEqual(manifest["plugins"]?["summary"]?["optional"]?.arrayValue?.compactMap(\.stringValue), ["llm"])
        XCTAssertEqual(manifest["plugins"]?["summary"]?["inject"]?.arrayValue?.compactMap(\.stringValue), ["notes"])
        XCTAssertEqual(manifest["plugins"]?["plain"]?["optional"]?.arrayValue?.compactMap(\.stringValue), [])
    }

    // guarantee: sdk/optional-deps (RutisTest: an optional service need not
    // be given; given, get finds it (use does not, as Rust's `require`);
    // typed get checks its methods as use does, saying `get`; a name in
    // both lists fails before load)
    func rutisTestGivesTheOptionalServicesTheTestGives() async throws {
        let seen = Noted()
        let summary = Rutis.define(Plugin(name: "summary", inject: ["notes"], optional: ["llm"]) { ctx in
            seen["get"] = await Self.asyncFailure { try await ctx.get("llm", LlmProxy.self)?.ask() }
            seen["use"] = Self.failure { try ctx.use("llm") }
            seen["undeclared"] = Self.failure { try ctx.get("other") }
        })

        let without = try await RutisTest.load(summary, services: ["notes": Mute()])
        XCTAssertEqual(seen["get"], "no error", "absent: nil")
        XCTAssertEqual(seen["use"],
                       #"not-declared: service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject {"service":"llm"}"#)
        XCTAssertEqual(seen["undeclared"],
                       #"not-declared: service other is not available to this plugin, which does not declare it: add 'other' to its optional (or to its inject, if it cannot run without it) {"service":"other"}"#)
        await without.close()

        let given = Noted()
        let asking = Rutis.define(Plugin(name: "summary", inject: ["notes"], optional: ["llm"]) { ctx in
            given["answer"] = try await ctx.get("llm", LlmProxy.self)?.ask()
            given["use"] = Self.failure { try ctx.use("llm") }
        })
        let with = try await RutisTest.load(asking, services: ["notes": Mute(), "llm": Oracle()])
        XCTAssertEqual(given["answer"], "42")
        XCTAssertEqual(given["use"],
                       #"not-declared: service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject {"service":"llm"}"#)
        await with.close()

        let lacking = try await RutisTest.load(summary, services: ["notes": Mute(), "llm": Mute()])
        XCTAssertEqual(seen["get"], "nil: get llm: LlmProxy calls ask, which the service does not have nil")
        await lacking.close()

        let both = Rutis.define(Plugin(name: "both", inject: ["llm"], optional: ["llm"]) { _ in })
        let refused = await Self.asyncFailure { try await RutisTest.load(both, services: ["llm": Oracle()]) }
        // The host refuses the row, saying why.
        XCTAssertTrue(refused.contains(": llm is in both inject and optional; keep it in one"), refused)
    }

    // guarantee: sdk/optional-deps (a service whose row stopped is not
    // available to whoever still holds its handle, until the host releases
    // it; one its row replaced while running keeps its object until then,
    // as a snapshot of a service does in Rust)
    func aHandleWhoseRowStoppedIsNotAvailable() async throws {
        let runtime = Runtime([], project: "")
        let handle = "flip#1"
        runtime.lock.withLock {
            runtime.handles[handle] = HandleEntry(id: "flip", name: "flip", object: Flip(), methods: nil)
        }
        let call = { try await runtime.dispatch(handle, "toss", []) }
        let read = { try await runtime.dispatch("", "get", [.data(.string(handle)), .data(.string("side"))]) }
        let (called, readFirst) = (await Self.asyncFailure(call), await Self.asyncFailure(read))
        XCTAssertEqual(called, "no error")
        XCTAssertEqual(readFirst, "no error")
        // Replaced while its row runs: the handle keeps its object.
        runtime.lock.withLock { runtime.retire(handle) }
        let (calledReplaced, readReplaced) = (await Self.asyncFailure(call), await Self.asyncFailure(read))
        XCTAssertEqual(calledReplaced, "no error")
        XCTAssertEqual(readReplaced, "no error")
        // Its row stopped: the handle is gone, not yet released.
        runtime.lock.withLock { runtime.gone("flip") }
        let (calledAfter, readAfter) = (await Self.asyncFailure(call), await Self.asyncFailure(read))
        XCTAssertEqual(calledAfter, #"unavailable: service flip is not available {"service":"flip"}"#)
        XCTAssertEqual(readAfter, #"unavailable: service flip is not available {"service":"flip"}"#)
    }

    // guarantee: sdk/optional-deps (a row loaded again gives its service a
    // handle of its own: one a caller still holds for the object before
    // stays retired, rather than reaching the new one)
    func aServiceOfARowLoadedAgainHasANewHandle() async throws {
        let oracle = Rutis.define(Plugin(name: "oracle") { ctx in _ = ctx.provide("oracle", Oracle()) })
        let runtime = Runtime([oracle], project: "")
        let load = { try await runtime.load(key: "a", name: nil, entry: "oracle", config: .undefined, exports: ["oracle": ["ask": "sync"]], isolate: [:], scope: []) }
        let handle = { runtime.lock.withLock { runtime.slots["oracle"]?.handle } }
        try await load()
        let old = try XCTUnwrap(handle())
        try await runtime.unload("a")
        try await load()
        let now = try XCTUnwrap(handle())
        XCTAssertTrue(now != old, "the new object has a handle of its own")
        let (calledNew, calledOld) = (await Self.asyncFailure { try await runtime.dispatch(now, "ask", []) },
                                      await Self.asyncFailure { try await runtime.dispatch(old, "ask", []) })
        XCTAssertEqual(calledNew, "no error")
        XCTAssertEqual(calledOld, #"unavailable: service oracle is not available {"service":"oracle"}"#)
    }
}
