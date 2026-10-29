// Config schemas (docs/design-config-schema-2026-10-27.en.md): a row's
// config filled and validated by the host before the plugin decodes it.
import Foundation
@testable import Rutis
import RutisTest

/// A config with a hand-written schema: `city` required, `units` with a
/// default.
struct SchemaWeather: Decodable, RutisConfigurable {
    var city: String
    var units: String

    static var rutisSchema: Rutis._Generated.Schema {
        Rutis._Generated.Schema(json: #"{"type":"object","required":["city"],"properties":{"city":{"type":"string"},"units":{"enum":["metric","imperial"],"default":"metric"}}}"#)
    }
}

/// A config whose `var` with a literal initial value is its default.
@RutisConfig
struct LiteralWeather: Decodable {
    var city: String
    var units = "metric"
    var days: Int = 3
    var path = #"C:\x"#
    var weights: [String: Int] = ["a": 1]
    var labels: Set<String> = ["x"]
    var amount: Decimal = 1.5
}

/// A schema that does not say what the type needs: `days` is an Int.
struct LooseDays: Decodable, RutisConfigurable {
    var days: Int

    static var rutisSchema: Rutis._Generated.Schema {
        Rutis._Generated.Schema(json: #"{"type":"object","properties":{"days":{}}}"#)
    }
}

final class Seen<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [T] = []
    func add(_ item: T) { lock.withLock { items.append(item) } }
    var all: [T] { lock.withLock { items } }
}

struct ConfigSchemaTests {
    // guarantee: sdk/config-schema (filled and validated before decoding)
    func aRowsConfigIsFilledAndValidatedBeforeItIsDecoded() async throws {
        let seen = Seen<SchemaWeather>()
        let plugin = Rutis.define(Plugin(name: "weather", config: SchemaWeather.self) { _, config in seen.add(config) })
        let loaded = try await RutisTest.load(plugin, config: ["city": "Oslo"])
        await loaded.close()
        XCTAssertEqual(seen.all.map(\.units), ["metric"])

        do {
            let failed = try await RutisTest.load(plugin, config: ["units": "kelvin"])
            await failed.close()
            XCTFail("a wrong config loads")
        } catch let error as RutisError {
            // The host's line of the row.
            XCTAssertEqual(error.message, "weather: config: city is required\nconfig: units should be \"metric\" or \"imperial\", got \"kelvin\"")
        }
        XCTAssertEqual(seen.all.count, 1)
    }

    /// `var units = "metric"` is not required: left out, it is "metric"
    /// (config schema design §3.3).
    func aLiteralInitialValueIsTheDefault() async throws {
        let seen = Seen<LiteralWeather>()
        let plugin = Rutis.define(Plugin(name: "weather", config: LiteralWeather.self) { _, config in seen.add(config) })
        let loaded = try await RutisTest.load(plugin, config: ["city": "Oslo"])
        await loaded.close()
        XCTAssertEqual(seen.all.map(\.units), ["metric"])
        XCTAssertEqual(seen.all.map(\.days), [3])
        XCTAssertEqual(seen.all.map(\.path), [#"C:\x"#])
        XCTAssertEqual(seen.all.map(\.weights), [["a": 1]])
        XCTAssertEqual(seen.all.map(\.labels), [["x"]])
        XCTAssertEqual(seen.all.map(\.amount), [1.5])
        do {
            let failed = try await RutisTest.load(plugin, config: ["units": 1])
            await failed.close()
            XCTFail("a wrong config loads")
        } catch let error as RutisError {
            XCTAssertEqual(error.message, "weather: config: city is required\nconfig: units should be a string, got 1")
        }
    }

    /// A decoder error after the schema passed is one sentence, never the
    /// decoder's text (design §3.3).
    func aDecoderErrorAfterTheSchemaIsOneSentence() async throws {
        let plugin = Rutis.define(Plugin(name: "loose", config: LooseDays.self) { _, _ in })
        do {
            let loaded = try await RutisTest.load(plugin, config: ["days": "x"])
            await loaded.close()
            XCTFail("a wrong config loads")
        } catch let error as RutisError {
            XCTAssertEqual(error.message, "loose: config: days is not a value the plugin accepts")
        }
    }

    /// A plain Decodable config (no schema of its own) is only decoded: a
    /// `String` config takes a string.
    func aConfigTypeWithoutASchemaIsOnlyDecoded() async throws {
        let seen = Seen<String>()
        let plugin = Rutis.define(Plugin(name: "named", config: String.self) { _, config in seen.add(config) })
        // What `check` validates rows with: what the type says.
        XCTAssertEqual(plugin.schema, .object([("type", .string("string"))]))
        let loaded = try await RutisTest.load(plugin, config: "pixel 1")
        await loaded.close()
        XCTAssertEqual(seen.all, ["pixel 1"])
    }

    /// A configuration no host checked (an app's own plugin): a missing key
    /// takes its literal default; a problem names its key in the host's
    /// words (`rutis::config::check`).
    func aConfigNoHostCheckedTakesDefaultsAndNamesItsKey() async throws {
        let seen = Seen<LiteralWeather>()
        let plugin = Rutis.define(Plugin(name: "weather", config: LiteralWeather.self) { _, config in seen.add(config) })
        let runtime = Runtime([plugin], project: "")
        try await runtime.load(key: "a", name: nil, entry: "weather", config: .data(.object([("city", .string("Oslo"))])),
                               exports: [:], isolate: [:], scope: [])
        XCTAssertEqual(seen.all.map(\.units), ["metric"])
        XCTAssertEqual(seen.all.map(\.days), [3])
        try await runtime.unload("a")
        for (config, message) in [(JSON.object([]), "config: city is required"),
                                  (.object([("city", .string("Oslo")), ("days", .string("x"))]), #"config: days should be an integer, got "x""#)]
        {
            do {
                try await runtime.load(key: "b", name: nil, entry: "weather", config: .data(config), exports: [:], isolate: [:], scope: [])
                XCTFail("a wrong config loads")
            } catch let error as RutisError {
                XCTAssertEqual(error.code, "bad-config")
                XCTAssertEqual(error.message, message)
            }
        }
    }
}
