// Config schemas (docs/design-config-schema-2026-10-27.en.md): the shared
// case list, run word for word; a row's config filled and validated before
// it is decoded, as the runtime, an app's runtime and RutisTest load it.
import Foundation
@testable import Rutis
import RutisTest

/// A config with a hand-written schema: `city` required, `units` with a
/// default.
struct SchemaWeather: Decodable, RutisConfigurable {
    var city: String
    var units: String

    static var rutisSchema: Rutis.Schema {
        Rutis.Schema(json: #"{"type":"object","required":["city"],"properties":{"city":{"type":"string"},"units":{"enum":["metric","imperial"],"default":"metric"}}}"#)
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
}

/// A schema that does not say what the type needs: `days` is an Int.
struct LooseDays: Decodable, RutisConfigurable {
    var days: Int

    static var rutisSchema: Rutis.Schema {
        Rutis.Schema(json: #"{"type":"object","properties":{"days":{}}}"#)
    }
}

final class Seen<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [T] = []
    func add(_ item: T) { lock.withLock { items.append(item) } }
    var all: [T] { lock.withLock { items } }
}

struct ConfigSchemaTests {
    /// The repository's case list.
    static func cases() throws -> [JSON] {
        let here = URL(fileURLWithPath: #filePath)
        let file = here.deletingLastPathComponent().appendingPathComponent("../../../../specs/config-schema/cases.json")
        let text = try String(contentsOf: file, encoding: .utf8)
        guard case let .object(doc) = try JSONReader.parse(text), case let .array(cases)? = ConfigSchema.lookup(doc, "cases") else {
            throw RutisError(name: "Error", message: "no cases")
        }
        return cases
    }

    static func strings(_ value: JSON?) -> [String] {
        guard case let .array(items)? = value else { return [] }
        return items.compactMap { if case let .string(s) = $0 { return s } else { return nil } }
    }

    // guarantee: sdk/config-schema (the shared case list, Swift's column)
    func everyCaseOfTheSharedListPasses() throws {
        for item in try Self.cases() {
            guard case let .object(c) = item else { continue }
            let name = ConfigSchema.lookup(c, "name").flatMap { if case let .string(s) = $0 { return s } else { return nil } } ?? "?"
            let schema = ConfigSchema.lookup(c, "schema") ?? .null
            let config = ConfigSchema.lookup(c, "config")
            let checked = ConfigSchema.check(schema, config)
            XCTAssertEqual(checked.warnings.map { "config: \($0.message)" }, Self.strings(ConfigSchema.lookup(c, "warnings")), name)
            guard case let .object(then)? = ConfigSchema.lookup(c, "then") else { continue }
            if let errors = ConfigSchema.lookup(then, "errors") {
                XCTAssertEqual(ConfigSchema.lines(checked.errors), Self.strings(errors), name)
            } else {
                XCTAssertEqual(checked.errors.map(\.message), [], name)
                XCTAssertEqual(checked.config, ConfigSchema.lookup(then, "config") ?? .null, name)
            }
        }
    }

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
            XCTAssertEqual(error.name, "BindingError")
            XCTAssertEqual(error.code, "bad-config")
            XCTAssertEqual(error.message, "config: city is required\nconfig: units should be \"metric\" or \"imperial\", got \"kelvin\"")
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
        do {
            let failed = try await RutisTest.load(plugin, config: ["units": 1])
            await failed.close()
            XCTFail("a wrong config loads")
        } catch let error as RutisError {
            XCTAssertEqual(error.message, "config: city is required\nconfig: units should be a string, got 1")
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
            XCTAssertEqual(error.code, "bad-config")
            XCTAssertEqual(error.message, "config: days is not a value the plugin accepts")
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

    func numbersAreWrittenAsJavaScriptWritesThem() {
        for (value, text) in [(1.0, "1"), (1.5, "1.5"), (-0.0, "0"), (1e21, "1e+21"), (1.5e21, "1.5e+21"), (1e-7, "1e-7"),
                              (0.000001, "0.000001"), (0.00001, "0.00001"), (123.456, "123.456"), (-2.5, "-2.5"), (0.1, "0.1"),
                              (1.5e20, "150000000000000000000"), (1e23, "1e+23"), (5e-324, "5e-324")]
        {
            XCTAssertEqual(ConfigSchema.doubleText(value), text, "\(value)")
        }
    }
}
