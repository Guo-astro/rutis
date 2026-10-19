import Foundation
@testable import Rutis
import XCTest

final class JSONTests: XCTestCase {
    func testReadsAndWritesData() throws {
        let text = #"{"a":[1,2.5,-3,9007199254740991,1e+21,-0.0],"b":"x\ny\"\u0001é😀","c":null,"d":true}"#
        let json = try JSONReader.parse(text)
        XCTAssertEqual(json["a"], .array([.number("1"), .number("2.5"), .number("-3"), .number("9007199254740991"),
                                          .number("1e+21"), .number("-0.0")]))
        XCTAssertEqual(json["b"], .string("x\ny\"\u{1}é😀"))
        // Numbers keep their text; strings escape control characters.
        XCTAssertEqual(JSONWriter.string(json), #"{"a":[1,2.5,-3,9007199254740991,1e+21,-0.0],"b":"x\ny\"\u0001é😀","c":null,"d":true}"#)
    }

    func testRefusesWhatIsNotJSON() {
        for bad in ["", "{", "[1,]", "\u{FEFF}{}", "{} x", "01", "1.", "-", "\"\u{1}\"", "{\"a\" 1}", "nul", "[1 2]"] {
            XCTAssertThrowsError(try JSONReader.parse(bad), bad)
        }
        // Not UTF-8 inside a string.
        XCTAssertThrowsError(try JSONReader.parse([0x22, 0xFF, 0x22]))
        XCTAssertThrowsError(try JSONReader.parse([0x22, 0xED, 0xA0, 0x80, 0x22]), "an encoded surrogate")
        XCTAssertThrowsError(try JSONReader.parse([0x22, 0xC0, 0xAF, 0x22]), "an overlong form")
    }

    func testSurrogatePairs() throws {
        XCTAssertEqual(try JSONReader.parse(#""😀""#), .string("😀"))
        // A lone surrogate becomes U+FFFD.
        XCTAssertEqual(try JSONReader.parse(#""\ud83d""#), .string("\u{FFFD}"))
    }

    func testDoublesKeepTheirBits() throws {
        for value in [0.1, 2.0, -0.0, 5e-324, Double.greatestFiniteMagnitude, 9_007_199_254_740_993.0, 1e21, 1e-7] {
            let json = try XCTUnwrap(JSON.double(value))
            guard case let .number(text) = json else { return XCTFail() }
            XCTAssertEqual(Double(text)?.bitPattern, value.bitPattern, text)
            XCTAssertFalse(JSONNumber.isIntegerLiteral(text), "a float is written as one: \(text)")
        }
        XCTAssertNil(JSON.double(.nan))
        XCTAssertNil(JSON.double(.infinity))
    }
}

final class TaggedValueTests: XCTestCase {
    func testDatesAndTimes() throws {
        XCTAssertEqual(Rutis.CalendarDate("2026-10-15"), Rutis.CalendarDate(year: 2026, month: 10, day: 15))
        XCTAssertNil(Rutis.CalendarDate("2026-02-29"))
        XCTAssertNotNil(Rutis.CalendarDate("2024-02-29"))
        XCTAssertNil(Rutis.CalendarDate("0000-01-01"))
        XCTAssertNil(Rutis.CalendarDate("2026-1-15"))
        XCTAssertEqual(Rutis.TimeOfDay("14:30:05.25")?.nanosecond, 250_000_000)
        XCTAssertEqual(Rutis.TimeOfDay(hour: 14, minute: 30, second: 5, nanosecond: 250_000_000).description, "14:30:05.25")
        XCTAssertNil(Rutis.TimeOfDay("24:00:00"))
        XCTAssertNil(Rutis.TimeOfDay("14:30:05.1234567890"))
        XCTAssertEqual(Rutis.DateTime("2026-10-15T14:30:05.25")?.description, "2026-10-15T14:30:05.25")
        XCTAssertNil(Rutis.DateTime("2026-10-15T14:30:05Z"))
    }

    func testInstants() throws {
        let instant = try XCTUnwrap(Instant("2026-10-15T03:22:15.123456789Z"))
        XCTAssertEqual(instant.description, "2026-10-15T03:22:15.123456789Z")
        // A Date holds about a microsecond: truncated toward negative
        // infinity (V7).
        XCTAssertEqual(Instant(instant.date).description, "2026-10-15T03:22:15.123456Z")
        XCTAssertEqual(Instant(try XCTUnwrap(Instant("2026-10-15T03:22:15.123Z")).date).description, "2026-10-15T03:22:15.123Z")
        let before = try XCTUnwrap(Instant("1969-12-31T23:59:59.999999Z"))
        XCTAssertEqual(Instant(before.date).description, "1969-12-31T23:59:59.999999Z")
        XCTAssertEqual(Instant(try XCTUnwrap(Instant("0001-01-01T00:00:00Z")).date).description, "0001-01-01T00:00:00Z")
        XCTAssertNil(Instant("2026-10-15T03:22:15+02:00"))
        XCTAssertThrowsError(try Instant(Date(timeIntervalSince1970: -70_000_000_000)).check())
    }

    func testBigIntegers() {
        let big = Rutis.BigInt("-123456789012345678901")
        XCTAssertEqual(big?.description, "-123456789012345678901")
        XCTAssertNil(Rutis.BigInt("-0"))
        XCTAssertNil(Rutis.BigInt("012"))
        XCTAssertNil(Rutis.BigInt(String(repeating: "9", count: 4301)))
        XCTAssertEqual(Rutis.BigInt(UInt64.max).exactly(UInt64.self), UInt64.max)
        XCTAssertNil(Rutis.BigInt(UInt64.max).exactly(Int64.self))
        XCTAssertTrue(Rutis.BigInt(9_007_199_254_740_991).isSafe)
        XCTAssertFalse(Rutis.BigInt(9_007_199_254_740_992).isSafe)
        XCTAssertTrue(Rutis.BigInt(-5) < Rutis.BigInt(-4))
        XCTAssertTrue(Rutis.BigInt(-5) < Rutis.BigInt(3))
        XCTAssertTrue(Rutis.BigInt(10) > Rutis.BigInt(9))
    }

    func testBytesDecimalsAndUUIDs() throws {
        XCTAssertEqual(try TagText.bytes("aGVsbG8="), Data("hello".utf8))
        XCTAssertThrowsError(try TagText.bytes("aGVsbG8"))
        XCTAssertThrowsError(try TagText.bytes("aGVsbG9="), "unused bits must be zero")
        XCTAssertNoThrow(try TagText.checkDecimal("-12.3400"))
        XCTAssertThrowsError(try TagText.checkDecimal("1e5"))
        XCTAssertThrowsError(try TagText.checkDecimal("01.5"))
        XCTAssertEqual(try TagText.decimal(Decimal(string: "-0.001")!), "-0.001")
        XCTAssertThrowsError(try TagText.decimal("1234567890123456789012345678901234567890"), "more than 38 digits")
        XCTAssertThrowsError(try TagText.checkUUID("0F8FAD5B-D9CB-469F-A165-70867728950E"))
        XCTAssertNoThrow(try TagText.checkUUID("0f8fad5b-d9cb-469f-a165-70867728950e"))
    }

    func testMapKeys() throws {
        XCTAssertNoThrow(try checkKeys([.data(.string("1")), .data(.number("1")), .data(.bool(true))], "map key"))
        XCTAssertThrowsError(try checkKeys([.data(.number("1")), .data(.number("1.0"))], "map key"), "1 and 1.0 are one key")
        XCTAssertThrowsError(try checkKeys([.data(.number("9007199254740992"))], "map key"), "beyond ±(2^53−1)")
        XCTAssertThrowsError(try checkKeys([.instant(Instant(seconds: 0, nanoseconds: 0))], "map key"))
        XCTAssertThrowsError(try checkKeys([.data(.null)], "set element"))
        XCTAssertNoThrow(try checkKeys([.bigint(Rutis.BigInt(UInt64.max)), .date(Rutis.CalendarDate(year: 2026, month: 1, day: 1))], "map key"))
    }
}

struct Reading: Codable, Equatable {
    var at: Date?
    var raw: Data?
    var n: Int64?
    var tags: Set<String>?
    var byDay: [Int: String]?
    var amount: Decimal?
    var id: UUID?
    var day: Rutis.CalendarDate?
}

final class CodecTests: XCTestCase {
    func written(_ value: Value) -> String {
        switch value {
        case let .data(json): return JSONWriter.string(json)
        case let .list(items): return "[" + items.map(written).joined(separator: ",") + "]"
        case let .record(fields): return "{" + fields.map { "\($0.0):\(written($0.1))" }.joined(separator: ",") + "}"
        case let .map(entries): return "map(" + entries.map { "\(written($0.0))=\(written($0.1))" }.joined(separator: ",") + ")"
        case let .set(elements): return "set(" + elements.map(written).joined(separator: ",") + ")"
        default: return "\(value.tag ?? "?")(\((try? taggedText(value)).flatMap { $0 } ?? value.described))"
        }
    }

    func testStructFieldsCrossAsTheirValues() throws {
        // The V9 rule: inside a struct, a Date is an instant, Data bytes, a
        // big Int64 a bigint, a Dictionary with Int keys a map.
        let reading = Reading(at: Instant("2025-10-15T03:22:15.123Z")!.date, raw: Data("hello".utf8), n: 1 << 60,
                              byDay: [1: "mon"], amount: Decimal(string: "1.5"), id: UUID(uuidString: "0F8FAD5B-D9CB-469F-A165-70867728950E"),
                              day: Rutis.CalendarDate(year: 2026, month: 10, day: 15))
        let value = try ValueCodec.encode(reading)
        XCTAssertEqual(written(value), "{at:instant(2025-10-15T03:22:15.123Z),raw:bytes(aGVsbG8=),n:bigint(1152921504606846976),byDay:map(1=\"mon\"),amount:decimal(1.5),id:uuid(0f8fad5b-d9cb-469f-a165-70867728950e),day:date(2026-10-15)}")
        let back = try ValueCodec.decode(Reading.self, from: value)
        XCTAssertEqual(back, reading)
    }

    func testPlainDataIsData() throws {
        struct Point: Codable { var x: Int; var y: [Double] }
        let value = try ValueCodec.encode(Point(x: 1, y: [2.5]))
        guard case .data = value else { return XCTFail("\(value)") }
        XCTAssertEqual(written(value), #"{"x":1,"y":[2.5]}"#)
    }

    func testIntegersBeyondTheSafeRangeAreBigInts() throws {
        XCTAssertEqual(written(try ValueCodec.encode(9_007_199_254_740_991)), "9007199254740991")
        XCTAssertEqual(written(try ValueCodec.encode(UInt64.max)), "bigint(18446744073709551615)")
        XCTAssertEqual(try ValueCodec.decode(UInt64.self, from: .bigint(Rutis.BigInt(UInt64.max))), UInt64.max)
        XCTAssertThrowsError(try ValueCodec.decode(Int64.self, from: .bigint(Rutis.BigInt(UInt64.max))))
        XCTAssertEqual(try ValueCodec.decode(Int.self, from: .data(.number("2.0"))), 2)
        XCTAssertThrowsError(try ValueCodec.decode(Int.self, from: .data(.number("2.5"))))
    }

    func testNonFiniteNumbersDoNotCross() {
        XCTAssertThrowsError(try ValueCodec.encode(Double.nan))
        XCTAssertThrowsError(try ValueCodec.encode([1.0, .infinity]))
    }

    func testAnyDecodesIntoNativeValues() throws {
        let value = Value.list([.data(.number("1")), .data(.number("1.5")), .data(.string("a")), .null,
                                .map([(.data(.number("1")), .data(.string("a")))]), .set([.data(.string("s"))]),
                                .bigint(Rutis.BigInt(UInt64.max)), .uuid("0f8fad5b-d9cb-469f-a165-70867728950e")])
        let native = try XCTUnwrap(ValueCodec.native(value) as? [Any?])
        XCTAssertEqual(native[0] as? Int, 1)
        XCTAssertEqual(native[1] as? Double, 1.5)
        XCTAssertEqual(native[2] as? String, "a")
        XCTAssertNil(native[3] ?? nil)
        XCTAssertEqual((native[4] as? [AnyHashable: Any?])?[1] as? String, "a")
        XCTAssertEqual(native[5] as? Set<AnyHashable>, ["s"])
        XCTAssertEqual(native[6] as? Rutis.BigInt, Rutis.BigInt(UInt64.max))
        XCTAssertEqual(native[7] as? UUID, UUID(uuidString: "0f8fad5b-d9cb-469f-a165-70867728950e"))
        // Sent on as they are, they keep their kinds.
        let again = try ValueCodec.encodeAny(native as Any)
        XCTAssertEqual(written(again), #"[1,1.5,"a",null,map(1="a"),set("s"),bigint(18446744073709551615),uuid(0f8fad5b-d9cb-469f-a165-70867728950e)]"#)
    }

    func testDecimalKeepsItsTextAsAString() throws {
        XCTAssertEqual(try ValueCodec.decode(String.self, from: .decimal("1.10")), "1.10")
        XCTAssertEqual(try ValueCodec.decode(Decimal.self, from: .decimal("1.10")), Decimal(string: "1.1"))
    }

    func testCyclicValuesFail() {
        final class Node: Encodable {
            var next: Node?
        }
        let node = Node()
        node.next = node
        XCTAssertThrowsError(try ValueCodec.encode(node))
    }

    func testOptionalsAndMissingFields() throws {
        struct Config: Decodable, Equatable { var city: String; var days: Int? }
        let value = Value.data(.object([("city", .string("Oslo"))]))
        XCTAssertEqual(try ValueCodec.decode(Config.self, from: value), Config(city: "Oslo", days: nil))
        XCTAssertThrowsError(try ValueCodec.decode(Config.self, from: .data(.object([]))))
        XCTAssertEqual(try ValueCodec.decode(Int?.self, from: .undefined), nil)
    }
}

final class ErrorGraphTests: XCTestCase {
    struct Quota: Error, LocalizedError {
        var code = "quota"
        var data: [String: Int] = ["left": 0]
        var errorDescription: String? { "quota exhausted" }
    }

    func testAnErrorCrossesWithItsCodeAndData() {
        let wire = encodeError(Quota()) { try ValueCodec.encodeAny($0) }
        XCTAssertEqual(wire["name"], .string("Quota"))
        XCTAssertEqual(wire["message"], .string("quota exhausted"))
        let back = decodeError(wire) { ValueCodec.native($0) }
        XCTAssertEqual(back.name, "Quota")
        XCTAssertEqual(back.code, "quota")
        XCTAssertEqual((back.data as? [String: Any?])?["left"] as? Int, 0)
        // Thrown on, the whole graph crosses again.
        let again = decodeError(encodeError(back) { try ValueCodec.encodeAny($0) }) { ValueCodec.native($0) }
        XCTAssertEqual(again.code, "quota")
        XCTAssertEqual(again.graph, back.graph)
    }

    func testAnAggregateKeepsItsChildrenInOrder() {
        let aggregate = RutisError(name: RutisError.Name.aggregateError, message: "2 listeners of t/x failed",
                                   errors: [RutisError(name: "AErr", message: "a"), RutisError(name: "BErr", message: "b", code: "B")])
        let back = decodeError(encodeError(aggregate) { try ValueCodec.encodeAny($0) }) { ValueCodec.native($0) }
        XCTAssertEqual(back.name, "AggregateError")
        XCTAssertEqual(back.errors.map(\.name), ["AErr", "BErr"])
        XCTAssertEqual(back.errors[1].code, "B")
    }
}
