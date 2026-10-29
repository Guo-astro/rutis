// guarantee: sdk/testing-matches-runtime
import Foundation
@testable import Rutis
import RutisFixturePlugins
@testable import RutisTest

/// The repository's root, from this file's path.
let repositoryRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
    .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()

func expectations(_ name: String) throws -> JSON {
    let url = repositoryRoot.appendingPathComponent("crates/rutis-bridge/src/runtime/testing/\(name)")
    return try JSONReader.parse(Array(try Data(contentsOf: url)))
}

/// A value as the expectations write it: data as JSON, a tagged value as
/// `{"$<tag>": <its wire text>}`.
func written(_ value: Value) -> JSON {
    func tagged(_ tag: String, _ json: JSON) -> JSON { .object([("$" + tag, json)]) }
    switch value {
    case .undefined: return .null
    case let .data(json): return json
    case let .list(items): return .array(items.map(written))
    case let .record(fields): return .object(fields.map { ($0.0, written($0.1)) })
    case let .map(entries): return tagged("map", .array(entries.map { .array([written($0.0), written($0.1)]) }))
    case let .set(elements): return tagged("set", .array(elements.map(written)))
    default:
        guard let tag = value.tag, let text = try? taggedText(value) else { return .string("<\(value.described)>") }
        return tagged(tag, .string(text))
    }
}

/// The clock the weather plugin injects: 0, 1, 2, ...
@RutisService
final class TestClock: @unchecked Sendable {
    private let lock = NSLock()
    private var n = 0
    func now() -> Int { lock.withLock { defer { n += 1 }; return n } }
}

struct WeatherConformanceTests {
    /// An argument as the expectations write it: data, or a callback.
    func argument(_ json: JSON) -> Any? {
        guard let callback = json["$callback"] else { return Value.data(json) }
        if callback == .string("upper") { return Rutis._Generated.export { (day: String) in day.uppercased() } }
        let thrown = callback["throws"]
        return Rutis._Generated.export { (_: String) async throws -> String in
            throw RutisError(name: thrown?["name"]?.stringValue ?? "Error", message: thrown?["message"]?.stringValue ?? "")
        }
    }

    func check(_ what: String, _ entry: JSON, _ result: Result<Value, Error>) {
        let why = entry["why"]?.stringValue ?? ""
        if let returns = entry["returns"] {
            switch result {
            case let .failure(error): record("\(what) (\(why)): expected \(JSONWriter.string(returns)), got \(error)")
            case let .success(value):
                var got = written(value)
                if let ignore = entry["ignore"]?.arrayValue, case let .object(fields) = got {
                    got = .object(fields.filter { field in !ignore.contains(.string(field.0)) })
                }
                expect(got == returns, "\(what) (\(why)): got \(JSONWriter.string(got)), want \(JSONWriter.string(returns))")
            }
        } else if let thrown = entry["throws"] {
            switch result {
            case let .success(value): record("\(what) (\(why)): expected an error, got \(JSONWriter.string(written(value)))")
            case let .failure(error):
                guard thrown["name"] != nil || thrown["message"] != nil else { return }
                guard let error = error as? RutisError else { record("\(what): not an error of the plugin: \(error)"); return }
                if let name = thrown["name"]?.stringValue { expect(error.name == name, "\(what) (\(why))") }
                if let message = thrown["message"]?.stringValue { expect(error.message == message, "\(what) (\(why))") }
            }
        } else {
            record("\(what): the expectations say neither returns nor throws")
        }
    }

    func call(_ service: Service, _ method: String, _ args: [Any?]) async -> Result<Value, Error> {
        do {
            return .success(try await service.callValue(method, try args.map { try ValueCodec.encodeAny($0 as Any) }))
        } catch {
            return .failure(error)
        }
    }

    func rutisTestMeetsTheRuntimeExpectations() async throws {
        let expected = try expectations("weather.expected.json")
        let loaded = try await RutisTest.load(weatherPlugin, services: ["clock": TestClock()])
        let service = try await loaded.service(expected["service"]!.stringValue!)
        for (method, kind) in expected["methods"]!.objectValue! {
            expect(service.methods[method] == kind.stringValue, "\(method)")
        }
        for (i, entry) in expected["calls"]!.arrayValue!.enumerated() where entry["allowed"] == nil {
            let method = entry["call"]!.stringValue!
            let args = entry["args"]!.arrayValue!.map(argument)
            check("calls[\(i)] \(method)", entry, await call(service, method, args))
        }
        // Values only Swift makes.
        let names = try await service.call("samples", as: [String].self)
        expect(!names.isEmpty, "the Swift plugin names its samples")
        let samples = expected["samples"]!.arrayValue!
        for name in names {
            guard let entry = samples.first(where: { $0["sample"] == .string(name) }) else {
                record("the expectations list no sample \(name)")
                continue
            }
            // It holds only across processes: the test plugin is in the
            // plugin's runtime.
            if entry["crossing"] == .bool(true) { continue }
            check("sample \(name)", entry, await call(service, "sample", [name]))
        }
        for event in expected["events"]!.arrayValue! {
            guard var then = event["then"] else { continue }
            let args = event["args"]!.arrayValue!.map(argument)
            // As a host plugin emits: parallel waits for the listeners.
            try await loaded.ctx.parallel(event["emit"]!.stringValue!, arguments: args)
            if case var .object(fields) = then {
                fields.append(("why", event["why"] ?? .null))
                then = .object(fields)
            }
            check("after \(event["emit"]!.stringValue!)", then, await call(service, then["call"]!.stringValue!, []))
        }
        // A service the plugin does not declare: its methods, all async (on
        // a device the test tool reaches only declared services).
        #if os(macOS)
        let undeclared = expected["undeclared"]!
        let forecast = try await loaded.service(undeclared["service"]!.stringValue!)
        for (method, kind) in undeclared["methods"]!.objectValue! {
            expect(forecast.methods[method] == kind.stringValue, "\(method)")
        }
        expect(forecast.methods.count == undeclared["methods"]!.objectValue!.count, "\(forecast.methods)")
        for (i, entry) in undeclared["calls"]!.arrayValue!.enumerated() {
            let method = entry["call"]!.stringValue!
            check("undeclared.calls[\(i)] \(method)", entry, await call(forecast, method, entry["args"]!.arrayValue!.map(argument)))
        }
        #endif
        await loaded.close()
    }
}
