// The {{name}} rutis plugin. The Swift plugin guide goes on from here:
// https://github.com/arcships/rutis/blob/main/docs/guide/swift-plugin.en.md
import Rutis

/// The plugin's configuration (guide, step 4): `@RutisConfig` derives its
/// JSON Schema; a `var` with a literal initial value is its default, filled
/// in when the row leaves it out.
@RutisConfig
public struct Config: Decodable {
    /// how to greet
    public var greeting = "Hello"
}

/// The service the plugin provides (guide, step 2): `@RutisService` makes
/// its methods the service's.
@RutisService
public final class Greeter: @unchecked Sendable {
    let greeting: String

    init(greeting: String) { self.greeting = greeting }

    public func hello(_ name: String) -> String { "\(greeting), \(name)!" }
}

public let plugin = Rutis.define(Plugin(
    name: "{{id}}",
    // The services it uses (guide, step 3): it starts once they are all there.
    inject: [],
    // What it provides, so that `rutis-host check` knows before it runs.
    provides: ["greeter": .of(Greeter.self)],
    config: Config.self,
    apply: { ctx, config in
        ctx.provide("greeter", Greeter(greeting: config.greeting))
    }
))
