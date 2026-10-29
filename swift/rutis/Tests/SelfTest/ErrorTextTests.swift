import Foundation
@testable import Rutis
@testable import RutisTest

// #677: a call or use this runtime refuses says so with what the developer
// wrote (service, method, 1-based argument), a code and data.

struct TextForecast: Codable {
    var city: String
}

@RutisService
final class TextWeather {
    func today() -> String { "sun" }
    func tomorrow(_ day: Int) -> String { "rain \(day)" }
    func forecast(_ forecast: TextForecast) -> String { forecast.city }
}

@RutisService
final class TextCache {
    func get(_ key: String) -> Int { 0 }
}

final class SavedError: @unchecked Sendable {
    private let lock = NSLock()
    private var saved: [Error] = []
    func add(_ error: Error) { lock.withLock { saved.append(error) } }
    var all: [Error] { lock.withLock { saved } }
}

struct ErrorTextTests {
    func refusal(_ body: () async throws -> Void) async -> RutisError? {
        do {
            try await body()
        } catch let error as RutisError {
            return error
        } catch {
            XCTFail("not a RutisError: \(error)")
            return nil
        }
        XCTFail("expected an error")
        return nil
    }

    func data(_ error: RutisError?) -> String {
        guard let data = error?.data else { return "nil" }
        let encoded = (try? ValueCodec.encodeAny(data))?.plainJSON
        return encoded.map(JSONWriter.string) ?? "?"
    }

    // guarantee: sdk/error-texts (a call the Swift runtime refuses names the
    // service, method and 1-based argument, code no-method or bad-argument)
    func aRefusedCallNamesTheServiceMethodAndArgument() async throws {
        let plugin = Rutis.define(Plugin(name: "weather") { ctx in
            ctx.provide("weather", TextWeather())
        })
        let loaded = try await RutisTest.load(plugin)
        defer { Task { await loaded.close() } }
        let weather = try await loaded.service("weather")

        var error = await refusal { _ = try await weather.callValue("tomorow", []) }
        XCTAssertEqual(error?.message, "weather has no method tomorow (methods: forecast, today, tomorrow)")
        XCTAssertEqual(error?.name, "BindingError")
        XCTAssertEqual(error?.code, "no-method")
        XCTAssertEqual(data(error), #"{"method":"tomorow","methods":["forecast","today","tomorrow"],"service":"weather"}"#)

        error = await refusal { _ = try await weather.callValue("tomorrow", [.data(.string("x"))]) }
        XCTAssertEqual(error?.message, #"weather.tomorrow: argument 1 should be an integer, got "x""#)
        XCTAssertEqual(error?.code, "bad-argument")
        XCTAssertEqual(data(error), #"{"argument":1,"expected":"an integer","method":"tomorrow","service":"weather"}"#)

        error = await refusal { _ = try await weather.callValue("tomorrow", []) }
        XCTAssertEqual(error?.message, "weather.tomorrow: argument 1 is missing")
        XCTAssertEqual(error?.code, "bad-argument")

        error = await refusal { _ = try await weather.callValue("forecast", [.data(.object([("city", .number("3"))]))]) }
        XCTAssertEqual(error?.message, "weather.forecast: argument 1 should be a TextForecast, got an object whose city is a number")
        error = await refusal { _ = try await weather.callValue("forecast", [.data(.array([]))]) }
        XCTAssertEqual(error?.message, "weather.forecast: argument 1 should be a TextForecast, got a list")
    }

    // guarantee: sdk/error-texts (an object passed by reference refuses a
    // method it lacks naming its type)
    func anObjectRefusesAMethodByItsTypeName() async throws {
        let error = await refusal { _ = try await ServiceObject(TextCache()).call(method: "gett", args: []) }
        XCTAssertEqual(error?.message, "TextCache has no method gett (methods: get)")
        XCTAssertEqual(error?.code, "no-method")
        XCTAssertEqual(data(error), #"{"method":"gett","methods":["get"],"object":"TextCache"}"#)
    }

    // guarantee: sdk/undeclared-use (codes: not-declared, unavailable)
    func useSaysWhyAServiceIsNotThere() async throws {
        let definition = Rutis.define(Plugin(name: "sneaky", inject: ["weather"]) { _ in })
        let runtime = Runtime([], project: "")
        let row = Row(key: "row:1.0", name: "sneaky", entry: "sneaky", definition: definition, config: .undefined,
                      exports: [:], isolate: [:], scope: [])
        let saved = SavedError()
        do { _ = try runtime.client(row, "llm") } catch { saved.add(error) }
        do { _ = try runtime.client(row, "weather") } catch { saved.add(error) }
        let errors = saved.all.compactMap { $0 as? RutisError }
        XCTAssertEqual(errors.count, 2)
        guard errors.count == 2 else { return }
        XCTAssertEqual(errors[0].message, "service llm is not available to this plugin, which does not declare it in inject: add 'llm' to its inject")
        XCTAssertEqual(errors[0].code, "not-declared")
        XCTAssertEqual(data(errors[0]), #"{"service":"llm"}"#)
        XCTAssertEqual(errors[1].message, "service weather is not available")
        XCTAssertEqual(errors[1].code, "unavailable")
        XCTAssertEqual(data(errors[1]), #"{"service":"weather"}"#)
    }

    // guarantee: sdk/error-texts (a call once the host went fails with `the
    // host disconnected`, code peer-gone)
    func aCallFailsSayingTheHostDisconnected() async throws {
        let (host, runtime) = try await pairOfPeers { _, _, _ in .undefined }
        host.close()
        host.channel.close()
        await runtime.closed()
        let error = await refusal { _ = try await runtime.call(target: "", method: "wait", args: [], sync: false) }
        XCTAssertEqual(error?.message, "the host disconnected")
        XCTAssertEqual(error?.name, "BindingError")
        XCTAssertEqual(error?.code, "peer-gone")
        XCTAssertEqual(data(error), #"{"reason":"disconnected"}"#)
    }
}
