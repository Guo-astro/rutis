// The {{name}} rutis plugin.
import Rutis

/// The plugin's configuration: a plain Decodable type. `@RutisConfig`
/// derives its JSON Schema, which `rutis-host check` validates rows with.
@RutisConfig
public struct Config: Decodable {
    /// how to greet
    public var greeting: String?
}

/// A service other plugins provide, in any language: `@RutisClient` makes
/// `ClockProxy`. To use one, add "clock" to `inject` and, in `apply`,
/// `let clock = try ctx.use("clock", ClockProxy.self)`.
@RutisClient
public protocol Clock {
    func now() async throws -> String
}

/// The service the plugin provides: `@RutisService` makes its methods the
/// service's (`hello` to other languages).
@RutisService
public final class Greeter: @unchecked Sendable {
    let greeting: String

    init(greeting: String) { self.greeting = greeting }

    public func hello(_ name: String) -> String { "\(greeting), \(name)!" }
}

public let plugin = Rutis.define(Plugin(
    name: "{{id}}",
    // Services this plugin uses: it runs while they are all there.
    inject: [],
    // What it provides, so that `rutis-host check` (and `check --strict` in a
    // host's CI) knows it before the plugin runs: Greeter's methods, async;
    // .of(Greeter.self, sync: ["hello"]) would make hello sync.
    provides: ["greeter": .of(Greeter.self)],
    config: Config.self,
    apply: { ctx, config in
        ctx.provide("greeter", Greeter(greeting: config.greeting ?? "Hello"))
        // Cleanups run when the plugin unloads, latest first.
        ctx.effect {}
    }
))
