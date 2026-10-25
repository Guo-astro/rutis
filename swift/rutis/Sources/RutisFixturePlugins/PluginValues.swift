// The Swift plugins of the plugin-values suite
// (crates/rutis-loader/tests/plugin_values.rs,
// docs/design-plugin-values-2026-10-25.md §9): the same origin as the
// JavaScript one, handing plugin definitions to a Rust service.
import Foundation
import Rutis

/// The part registry, a Rust service: `register` takes a plugin definition,
/// `takesData` only data.
@RutisClient
public protocol ValuesParts {
    func register(_ scope: String, _ definition: Rutis.Definition) async throws
    func takesData(_ value: Rutis.Definition) async throws -> String
}

/// The instance's `tools`.
@RutisClient
public protocol ValuesTools {
    func title() async throws -> String
}

/// `maker`: returns a new definition each time.
@RutisService
public final class ValuesMaker: @unchecked Sendable {
    let made: () -> Rutis.Definition
    init(_ made: @escaping () -> Rutis.Definition) { self.made = made }
    public func make() -> Rutis.Definition { made() }
}

struct PartConfig: Decodable {
    var n: String
}

/// A session part using what its origin's `apply` prepared.
func valuesPart(_ tag: String, _ prepared: String) -> Rutis.Definition {
    Rutis.define(Plugin(name: "values_part", inject: ["tools", "probe"], config: PartConfig.self) { ctx, config in
        let probe = try ctx.use("probe", ProbeProxy.self)
        let title = try await ctx.use("tools", ValuesToolsProxy.self).title()
        try await probe.record("\(tag) \(config.n) sees \(title) with \(prepared)")
        ctx.effect { try await probe.record("\(tag) \(config.n) down") }
    })
}

/// The origin: prepares a value, hands a session part using it to `parts`,
/// and provides `maker`, whose `make` returns another one.
public let valuesOrigin = Rutis.define(Plugin(name: "values_origin", inject: ["parts", "probe"]) { ctx in
    let probe = try ctx.use("probe", ProbeProxy.self)
    let parts = try ctx.use("parts", ValuesPartsProxy.self)
    let prepared = "prepared in apply"
    try await parts.register("session", valuesPart("part", prepared))
    do {
        _ = try await parts.takesData(valuesPart("data", prepared))
        try await probe.record("data took a definition")
    } catch {
        try await probe.record("data refused: \((error as? RutisError)?.message ?? "\(error)")")
    }
    ctx.provide("maker", ValuesMaker { valuesPart("made", prepared) })
    ctx.effect { try await probe.record("origin down") }
})

/// Ends its runtime's process soon after it loads.
public let valuesCrash = Rutis.define(Plugin(name: "values_crash") { _ in
    DispatchQueue.global().asyncAfter(deadline: .now() + 0.2) { exit(17) }
})
