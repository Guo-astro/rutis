// guarantee: bridge/plugin-values
// Plugin definitions as values (docs/design-plugin-values-2026-10-25.md):
// a `Rutis.define` result crosses as a `plugin` reference, only to a far
// end that takes plugin definitions; back home it is the definition itself;
// the runtime describes it (`plugins.read`) and loads it as a row's entry,
// each copy with its config; RutisTest hands a test double the definition
// itself, so the test loads it as the host would mount it (R10). The
// cross-language cases: crates/rutis-loader/tests/plugin_values.rs.
import Foundation
@testable import Rutis
import RutisTest

@RutisClient
protocol TestParts {
    func register(_ scope: String, _ definition: Rutis.Definition) async throws
    func echo(_ definition: Rutis.Definition) async throws -> Rutis.Definition
}

/// A test double of a part registry: keeps what it is handed.
@RutisService
final class HandedParts: @unchecked Sendable {
    private let lock = NSLock()
    private var handed: [(String, Rutis.Definition)] = []
    func register(_ scope: String, _ definition: Rutis.Definition) {
        lock.withLock { handed.append((scope, definition)) }
    }

    /// Hands the definition back.
    func echo(_ definition: Rutis.Definition) -> Rutis.Definition { definition }

    var all: [(String, Rutis.Definition)] { lock.withLock { handed } }
}

@RutisService
final class Tag: @unchecked Sendable {
    let line: String
    init(_ line: String) { self.line = line }
    func text() -> String { line }
}

struct DepthConfig: Decodable {
    var depth: Int
}

struct PluginValueTests {
    func theRuntimeTakesDefinitionsBack() async throws {
        let part = Rutis.define(Plugin(name: "part", inject: ["history"], config: DepthConfig.self) { _, _ in })
        let runtime = Runtime([], project: "")
        let described = try await runtime.control("plugins.read", [.plugin(part)])
        XCTAssertEqual(described["name"]?.string, "part")
        XCTAssertEqual(described["inject"]?.plainJSON, .array([.string("history")]))
        XCTAssertEqual(described["announces"]?.plainJSON, .bool(true))
        do {
            _ = try await runtime.control("plugins.read", [.data(.int(42))])
            XCTFail("a number is not a plugin definition")
        } catch let error as RutisError {
            XCTAssertTrue(error.message.contains("not a plugin definition"), error.message)
        }
        let mounted = try await runtime.control("mount", [.data(.object([]))])
        XCTAssertTrue(mounted["features"]?.plainJSON?.arrayValue?.contains(.string("plugin-values")) ?? false)
    }

    /// Each copy loads with its own config and leaves with its row; the
    /// binary need not list the definition.
    func eachCopyLoadsWithItsConfig() async throws {
        let lines = Lines()
        let part = Rutis.define(Plugin(name: "part", config: DepthConfig.self) { ctx, config in
            lines.add("up \(config.depth)")
            ctx.effect { lines.add("down \(config.depth)") }
        })
        let runtime = Runtime([], project: "")
        for (key, depth) in [("c1", 1), ("c2", 2)] {
            _ = try await runtime.control("rows.load", [
                .data(.string(key)), .plugin(part), .data(.object([("depth", .int(depth))])), .data(.array([])), .data(.array([])), .null,
            ])
        }
        XCTAssertEqual(lines.all, ["up 1", "up 2"])
        _ = try await runtime.control("rows.unload", [.data(.string("c1"))])
        XCTAssertEqual(lines.all, ["up 1", "up 2", "down 1"])
        _ = try await runtime.control("rows.unload", [.data(.string("c2"))])
        XCTAssertEqual(lines.all, ["up 1", "up 2", "down 1", "down 2"])
    }

    /// Sent home it is the same definition; a far end that does not take
    /// plugin definitions fails only that call.
    func aDefinitionGoesHomeAsItself() async throws {
        let part = Rutis.define(Plugin(name: "part") { _ in })
        let back = SavedValue()
        let (host, runtime) = try await pairOfPeers { _, method, args in
            switch method {
            case "give": return .plugin(part)
            case "take":
                back.value = args.first
                return .undefined
            default: throw RutisError(name: "Error", message: "no \(method)")
            }
        }
        defer {
            host.close()
            runtime.close()
        }
        let given = try await host.call(target: "t", method: "give", args: [], sync: false)
        guard case let .remotePlugin(first) = given else { return XCTFail("\(given)") }
        // The same value handed over twice is one reference.
        let twice = try await host.call(target: "t", method: "give", args: [], sync: false)
        guard case let .remotePlugin(second) = twice else { return XCTFail("\(twice)") }
        XCTAssertEqual(first.record.id, second.record.id)
        XCTAssertTrue(first === second)
        _ = try await host.call(target: "t", method: "take", args: [given], sync: false)
        guard case let .plugin(home)? = back.value else { return XCTFail("\(String(describing: back.value))") }
        XCTAssertTrue(home === part)
        // The runtime's far end (the test host) takes them; the host's does
        // not: sending one fails that call, and the session goes on.
        XCTAssertTrue(runtime.supports("plugins"))
        XCTAssertFalse(host.supports("plugins"))
        do {
            _ = try await host.call(target: "t", method: "take", args: [.plugin(part)], sync: false)
            XCTFail("the far end does not take plugin definitions")
        } catch {
            XCTAssertTrue("\(error)".contains("does not accept plugin definitions"), "\(error)")
        }
        let again = try await host.call(target: "t", method: "give", args: [], sync: false)
        guard case .remotePlugin = again else { return XCTFail("\(again)") }
    }

    /// A definition handed to a test double is the definition itself: the
    /// test loads it, each copy with its config (R10).
    func aDefinitionHandedToATestDoubleLoads() async throws {
        let prepared = "prepared in apply"
        let parts = HandedParts()
        let made = SavedDefinition()
        let origin = Rutis.define(Plugin(name: "origin", inject: ["parts"]) { ctx in
            let part = Rutis.define(Plugin(name: "part", provides: ["tag": .of(Tag.self, sync: ["text"])], config: DepthConfig.self) { ctx, config in
                ctx.provide("tag", Tag("\(config.depth) \(prepared)"))
            })
            made.value = part
            try await ctx.use("parts", TestPartsProxy.self).register("session", part)
        })
        let loaded = try await RutisTest.load(origin, services: ["parts": parts])
        defer { Task { await loaded.close() } }
        XCTAssertEqual(parts.all.count, 1)
        let (scope, definition) = try XCTUnwrap(parts.all.first)
        XCTAssertEqual(scope, "session")
        XCTAssertTrue(definition === made.value)
        // One copy at a time: both provide `tag`.
        let one = try await RutisTest.load(definition, config: ["depth": 1])
        let first = try await one.service("tag").call("text", as: String.self)
        await one.close()
        let two = try await RutisTest.load(definition, config: ["depth": 2])
        let second = try await two.service("tag").call("text", as: String.self)
        await two.close()
        XCTAssertEqual(first, "1 \(prepared)")
        XCTAssertEqual(second, "2 \(prepared)")
    }

    /// A test double hands a definition back: the plugin gets the same
    /// definition, as rutis sends it home.
    func aTestDoubleHandsADefinitionBack() async throws {
        let parts = HandedParts()
        let same = SavedValue()
        let origin = Rutis.define(Plugin(name: "origin", inject: ["parts"]) { ctx in
            let part = Rutis.define(Plugin(name: "part") { _ in })
            let back = try await ctx.use("parts", TestPartsProxy.self).echo(part)
            same.value = .data(.bool(back === part))
        })
        let loaded = try await RutisTest.load(origin, services: ["parts": parts])
        XCTAssertEqual(same.value?.plainJSON, .bool(true))
        await loaded.close()
    }
}

final class SavedValue: @unchecked Sendable {
    private let lock = NSLock()
    private var saved: Value?
    var value: Value? {
        get { lock.withLock { saved } }
        set { lock.withLock { saved = newValue } }
    }
}

final class SavedDefinition: @unchecked Sendable {
    private let lock = NSLock()
    private var saved: Rutis.Definition?
    var value: Rutis.Definition? {
        get { lock.withLock { saved } }
        set { lock.withLock { saved = newValue } }
    }
}
