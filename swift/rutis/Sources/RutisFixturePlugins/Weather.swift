// The runtime conformance plugin (rutis_bridge::runtime::testing::runtime)
// for the Swift runtime: it injects `clock` and provides `weather`. What
// calls on it return is in crates/rutis-bridge/src/runtime/testing/weather.expected.json.
import Foundation
import Rutis

@RutisConfig
public struct WeatherConfig: Decodable {
    /// the city to report on
    public var city: String?
}

@RutisClient
public protocol Clock {
    func now() async throws -> Int
}

/// A city that listeners change while calls read it.
final class City: @unchecked Sendable {
    private let lock = NSLock()
    private var name: String
    init(_ name: String) { self.name = name }
    var value: String {
        get { lock.withLock { name } }
        set { lock.withLock { name = newValue } }
    }
}

@RutisService
public final class Weather: @unchecked Sendable {
    let clock: any Clock
    let city: City

    init(clock: any Clock, city: City) {
        self.clock = clock
        self.city = city
    }

    public func today() async throws -> String {
        let now = try await clock.now()
        return "\(city.value) at \(now)"
    }

    public func later() async throws -> String {
        try await Task.sleep(nanoseconds: 10_000_000)
        return "\(city.value) later"
    }

    /// Calls back during the call.
    public func each(_ callback: @escaping (String) async throws -> String) async throws -> [String] {
        var out: [String] = []
        for day in ["mon", "tue"] { out.append(try await callback(day)) }
        return out
    }

    /// Returns the value it was given.
    public func echo(_ value: Any?) -> Any? { value }

    public func crash() { exit(17) }

    /// Names the values only Swift makes.
    public func samples() -> [String] { weatherSamples.keys.sorted() }

    /// Returns the value `name`.
    public func sample(_ name: String) throws -> Any? {
        guard let sample = weatherSamples[name] else { throw RutisError(name: "Error", message: "no sample \(name)") }
        return try sample()
    }
}

/// A struct with fields JSONEncoder would have turned into numbers and
/// text: they cross as their values.
struct Reading: Codable {
    var raw: Data?
    var at: Date?
    var n: Int64?
}

struct Overdrawn: Error, LocalizedError {
    var errorDescription: String? { "out of range" }
}

/// The values only Swift makes, by name (`samples` in the expectations).
let weatherSamples: [String: () throws -> Any?] = [
    "swift/value-instant": { Date(timeIntervalSince1970: 1_760_498_535.123456) },
    "swift/value-struct": {
        Reading(raw: Data("hello".utf8), at: Date(timeIntervalSince1970: 1_760_498_535), n: 1 << 60)
    },
    "swift/value-int-keys": { [2: "b", 1: "a"] as [Int: String] },
    "swift/value-set": { Set(["b", "a"]) },
    "swift/value-bigint": { Rutis.Advanced.BigInt("-123456789012345678901")! },
    "swift/big-int": { UInt64.max },
    "swift/value-constructed": {
        [
            Rutis.Advanced.DateTime(year: 2026, month: 10, day: 15, hour: 14, minute: 30, second: 5, nanosecond: 250_000_000),
            Rutis.Advanced.CalendarDate(year: 2026, month: 10, day: 15),
            Rutis.Advanced.TimeOfDay(hour: 14, minute: 30, second: 5, nanosecond: 250_000_000),
            Decimal(string: "-12.34")!,
            UUID(uuidString: "0F8FAD5B-D9CB-469F-A165-70867728950E")!,
        ] as [Any]
    },
    "swift/nan": { Double.nan },
    "swift/value-date-key": { [Date(timeIntervalSince1970: 0): "epoch"] },
    "swift/error-value": { Overdrawn() },
]

/// `undeclared` in the expectations: a service no `provides` declares,
/// with its methods, a stored property and a private method.
@RutisService
public final class Forecast: @unchecked Sendable {
    public let city: String

    init(city: String) { self.city = city }

    public func tomorrow() -> String { "\(city) tomorrow" }

    /// Swift types have no inherited methods a macro sees: the week is the
    /// type's own method.
    public func week(_ n: Int) -> [String] { Array(["mon", "tue", "wed"].prefix(n)) }

    private func hidden() -> String { "not a method rutis sees" }
}

public let weatherPlugin = Rutis.define(Plugin(
    name: "weather",
    inject: ["clock"],
    provides: ["weather": .of(Weather.self, sync: ["today", "each", "echo", "crash", "samples", "sample"])],
    config: WeatherConfig.self,
    apply: { ctx, config in
        let clock = try ctx.use("clock", ClockProxy.self)
        let city = City(config.city ?? "Oslo")
        ctx.provide("weather", Weather(clock: clock, city: city))
        ctx.provide("forecast", Forecast(city: "Oslo"))
        ctx.on("weather/city") { (name: String) in city.value = name }
        ctx.effect { print("weather: bye") }
    }))
