import Foundation

/// A JSON value as the session reads and writes it. Numbers keep their text,
/// so data crossing the session comes back as it was sent (an integer stays
/// an integer, a float keeps every bit); objects keep their keys in order.
package indirect enum JSON: Equatable, Sendable {
    case null
    case bool(Bool)
    /// A number as its JSON text (always valid JSON number grammar).
    case number(String)
    case string(String)
    case array([JSON])
    case object([(String, JSON)])

    package static func == (a: JSON, b: JSON) -> Bool {
        switch (a, b) {
        case (.null, .null): return true
        case let (.bool(x), .bool(y)): return x == y
        case let (.number(x), .number(y)): return x == y
        case let (.string(x), .string(y)): return x == y
        case let (.array(x), .array(y)): return x == y
        case let (.object(x), .object(y)):
            guard x.count == y.count else { return false }
            for (key, value) in x {
                guard let other = JSON.lookup(y, key), other == value else { return false }
            }
            return true
        default: return false
        }
    }

    static func lookup(_ fields: [(String, JSON)], _ key: String) -> JSON? {
        // The last of duplicate keys counts, as in most JSON readers.
        for (name, value) in fields.reversed() where name == key { return value }
        return nil
    }

    /// The field `key` of an object, nil for anything else.
    package subscript(key: String) -> JSON? {
        if case let .object(fields) = self { return JSON.lookup(fields, key) }
        return nil
    }

    package var stringValue: String? {
        if case let .string(s) = self { return s }
        return nil
    }

    package var boolValue: Bool? {
        if case let .bool(b) = self { return b }
        return nil
    }

    package var arrayValue: [JSON]? {
        if case let .array(a) = self { return a }
        return nil
    }

    package var objectValue: [(String, JSON)]? {
        if case let .object(o) = self { return o }
        return nil
    }

    package var isNull: Bool {
        if case .null = self { return true }
        return false
    }

    /// An integer number's value, if it is one that fits an Int64.
    package var int64Value: Int64? {
        guard case let .number(text) = self else { return nil }
        return JSONNumber.integer(text)
    }

    package var doubleValue: Double? {
        guard case let .number(text) = self else { return nil }
        return Double(text)
    }

    /// How deep arrays and objects nest in it (a scalar is 0).
    package var depth: Int {
        switch self {
        case let .array(items): return 1 + (items.map(\.depth).max() ?? 0)
        case let .object(fields): return 1 + (fields.map(\.1.depth).max() ?? 0)
        default: return 0
        }
    }

    package static func int(_ value: some BinaryInteger) -> JSON { .number(String(value)) }

    /// A finite double as a JSON number: its shortest text that reads back
    /// to the same bits, always written as a float (`2.0`, `1e+21`).
    package static func double(_ value: Double) -> JSON? {
        guard value.isFinite else { return nil }
        return .number(value.description)
    }
}

/// JSON number text.
package enum JSONNumber {
    /// The integer `text` names, when it is an integer literal (no fraction,
    /// no exponent) that fits an Int64.
    package static func integer(_ text: String) -> Int64? {
        if text.contains(where: { $0 == "." || $0 == "e" || $0 == "E" }) { return nil }
        return Int64(text)
    }

    /// Whether `text` is an integer literal (no fraction or exponent).
    package static func isIntegerLiteral(_ text: String) -> Bool {
        !text.contains(where: { $0 == "." || $0 == "e" || $0 == "E" })
    }
}

/// Why a message is not a JSON text this side takes.
package struct JSONError: Error, CustomStringConvertible {
    package let description: String
    init(_ description: String) { self.description = description }
}

// ── Reading ─────────────────────────────────────────────────────────────

/// Reads a JSON text strictly (RFC 8259): UTF-8 throughout, no byte order
/// mark, nothing but whitespace after the value.
package struct JSONReader {
    /// How deep a text may nest before it is refused (well beyond what the
    /// session sends: 64 levels of value inside a frame).
    package static let maxDepth = 512

    private let bytes: [UInt8]
    private var at = 0

    private init(_ bytes: [UInt8]) { self.bytes = bytes }

    package static func parse(_ data: some Collection<UInt8>) throws -> JSON {
        var reader = JSONReader(Array(data))
        reader.skipSpace()
        let value = try reader.value(depth: 0)
        reader.skipSpace()
        guard reader.at == reader.bytes.count else { throw JSONError("unexpected text after the JSON value") }
        return value
    }

    package static func parse(_ text: String) throws -> JSON { try parse(Array(text.utf8)) }

    private mutating func skipSpace() {
        while at < bytes.count {
            switch bytes[at] {
            case 0x20, 0x09, 0x0A, 0x0D: at += 1
            default: return
            }
        }
    }

    private mutating func expect(_ literal: String, _ value: JSON) throws -> JSON {
        let utf8 = Array(literal.utf8)
        guard at + utf8.count <= bytes.count, Array(bytes[at ..< at + utf8.count]) == utf8 else {
            throw JSONError("invalid literal")
        }
        at += utf8.count
        return value
    }

    private mutating func value(depth: Int) throws -> JSON {
        guard at < bytes.count else { throw JSONError("unexpected end of JSON") }
        switch bytes[at] {
        case UInt8(ascii: "{"):
            guard depth < JSONReader.maxDepth else { throw JSONError("JSON nested too deep") }
            at += 1
            var fields: [(String, JSON)] = []
            skipSpace()
            if at < bytes.count, bytes[at] == UInt8(ascii: "}") {
                at += 1
                return .object(fields)
            }
            while true {
                skipSpace()
                guard at < bytes.count, bytes[at] == UInt8(ascii: "\"") else { throw JSONError("expected a key") }
                let key = try string()
                skipSpace()
                guard at < bytes.count, bytes[at] == UInt8(ascii: ":") else { throw JSONError("expected ':'") }
                at += 1
                skipSpace()
                fields.append((key, try value(depth: depth + 1)))
                skipSpace()
                guard at < bytes.count else { throw JSONError("unexpected end of JSON") }
                if bytes[at] == UInt8(ascii: ",") { at += 1; continue }
                if bytes[at] == UInt8(ascii: "}") { at += 1; return .object(fields) }
                throw JSONError("expected ',' or '}'")
            }
        case UInt8(ascii: "["):
            guard depth < JSONReader.maxDepth else { throw JSONError("JSON nested too deep") }
            at += 1
            var items: [JSON] = []
            skipSpace()
            if at < bytes.count, bytes[at] == UInt8(ascii: "]") {
                at += 1
                return .array(items)
            }
            while true {
                skipSpace()
                items.append(try value(depth: depth + 1))
                skipSpace()
                guard at < bytes.count else { throw JSONError("unexpected end of JSON") }
                if bytes[at] == UInt8(ascii: ",") { at += 1; continue }
                if bytes[at] == UInt8(ascii: "]") { at += 1; return .array(items) }
                throw JSONError("expected ',' or ']'")
            }
        case UInt8(ascii: "\""):
            return .string(try string())
        case UInt8(ascii: "t"): return try expect("true", .bool(true))
        case UInt8(ascii: "f"): return try expect("false", .bool(false))
        case UInt8(ascii: "n"): return try expect("null", .null)
        case UInt8(ascii: "-"), UInt8(ascii: "0") ... UInt8(ascii: "9"):
            return .number(try number())
        default:
            throw JSONError("unexpected character")
        }
    }

    private mutating func number() throws -> String {
        let start = at
        func digit(_ b: UInt8) -> Bool { b >= 0x30 && b <= 0x39 }
        if bytes[at] == UInt8(ascii: "-") { at += 1 }
        guard at < bytes.count, digit(bytes[at]) else { throw JSONError("invalid number") }
        if bytes[at] == UInt8(ascii: "0") {
            at += 1
        } else {
            while at < bytes.count, digit(bytes[at]) { at += 1 }
        }
        if at < bytes.count, bytes[at] == UInt8(ascii: ".") {
            at += 1
            guard at < bytes.count, digit(bytes[at]) else { throw JSONError("invalid number") }
            while at < bytes.count, digit(bytes[at]) { at += 1 }
        }
        if at < bytes.count, bytes[at] == UInt8(ascii: "e") || bytes[at] == UInt8(ascii: "E") {
            at += 1
            if at < bytes.count, bytes[at] == UInt8(ascii: "+") || bytes[at] == UInt8(ascii: "-") { at += 1 }
            guard at < bytes.count, digit(bytes[at]) else { throw JSONError("invalid number") }
            while at < bytes.count, digit(bytes[at]) { at += 1 }
        }
        return String(decoding: bytes[start ..< at], as: UTF8.self)
    }

    private mutating func hex4() throws -> UInt32 {
        guard at + 4 <= bytes.count else { throw JSONError("invalid escape") }
        var value: UInt32 = 0
        for _ in 0 ..< 4 {
            let b = bytes[at]
            let digit: UInt32
            switch b {
            case 0x30 ... 0x39: digit = UInt32(b - 0x30)
            case 0x41 ... 0x46: digit = UInt32(b - 0x41 + 10)
            case 0x61 ... 0x66: digit = UInt32(b - 0x61 + 10)
            default: throw JSONError("invalid escape")
            }
            value = value * 16 + digit
            at += 1
        }
        return value
    }

    private mutating func string() throws -> String {
        at += 1 // the opening quote
        var out: [UInt8] = []
        var run = at
        while true {
            guard at < bytes.count else { throw JSONError("unterminated string") }
            let b = bytes[at]
            if b == UInt8(ascii: "\"") {
                out.append(contentsOf: bytes[run ..< at])
                at += 1
                break
            }
            if b < 0x20 { throw JSONError("a control character in a string") }
            if b == UInt8(ascii: "\\") {
                out.append(contentsOf: bytes[run ..< at])
                at += 1
                guard at < bytes.count else { throw JSONError("invalid escape") }
                let e = bytes[at]
                at += 1
                switch e {
                case UInt8(ascii: "\""): out.append(0x22)
                case UInt8(ascii: "\\"): out.append(0x5C)
                case UInt8(ascii: "/"): out.append(0x2F)
                case UInt8(ascii: "b"): out.append(0x08)
                case UInt8(ascii: "f"): out.append(0x0C)
                case UInt8(ascii: "n"): out.append(0x0A)
                case UInt8(ascii: "r"): out.append(0x0D)
                case UInt8(ascii: "t"): out.append(0x09)
                case UInt8(ascii: "u"):
                    var scalar = try hex4()
                    if scalar >= 0xD800, scalar < 0xDC00 {
                        // A high surrogate: its low one should follow.
                        if at + 6 <= bytes.count, bytes[at] == UInt8(ascii: "\\"), bytes[at + 1] == UInt8(ascii: "u") {
                            let save = at
                            at += 2
                            let low = try hex4()
                            if low >= 0xDC00, low < 0xE000 {
                                scalar = 0x10000 + ((scalar - 0xD800) << 10) + (low - 0xDC00)
                            } else {
                                at = save
                                scalar = 0xFFFD
                            }
                        } else {
                            scalar = 0xFFFD
                        }
                    } else if scalar >= 0xDC00, scalar < 0xE000 {
                        scalar = 0xFFFD
                    }
                    out.append(contentsOf: Array(String(Character(Unicode.Scalar(scalar)!)).utf8))
                default:
                    throw JSONError("invalid escape")
                }
                run = at
                continue
            }
            at += 1
        }
        guard UTF8Check.valid(out) else { throw JSONError("a string is not UTF-8") }
        return String(decoding: out, as: UTF8.self)
    }
}

// ── Writing ─────────────────────────────────────────────────────────────

package enum JSONWriter {
    /// The compact JSON text of `value`: no whitespace, no raw control
    /// characters (so never a raw newline), non-ASCII text as it is.
    package static func write(_ value: JSON) -> [UInt8] {
        var out: [UInt8] = []
        out.reserveCapacity(64)
        write(value, into: &out)
        return out
    }

    package static func string(_ value: JSON) -> String {
        String(decoding: write(value), as: UTF8.self)
    }

    private static let hex: [UInt8] = Array("0123456789abcdef".utf8)

    package static func write(_ value: JSON, into out: inout [UInt8]) {
        switch value {
        case .null: out.append(contentsOf: [0x6E, 0x75, 0x6C, 0x6C])
        case let .bool(b): out.append(contentsOf: Array((b ? "true" : "false").utf8))
        case let .number(text): out.append(contentsOf: Array(text.utf8))
        case let .string(s): writeString(s, into: &out)
        case let .array(items):
            out.append(UInt8(ascii: "["))
            for (i, item) in items.enumerated() {
                if i > 0 { out.append(UInt8(ascii: ",")) }
                write(item, into: &out)
            }
            out.append(UInt8(ascii: "]"))
        case let .object(fields):
            out.append(UInt8(ascii: "{"))
            for (i, (key, item)) in fields.enumerated() {
                if i > 0 { out.append(UInt8(ascii: ",")) }
                writeString(key, into: &out)
                out.append(UInt8(ascii: ":"))
                write(item, into: &out)
            }
            out.append(UInt8(ascii: "}"))
        }
    }

    static func writeString(_ s: String, into out: inout [UInt8]) {
        out.append(UInt8(ascii: "\""))
        for b in s.utf8 {
            switch b {
            case 0x22: out.append(contentsOf: [0x5C, 0x22])
            case 0x5C: out.append(contentsOf: [0x5C, 0x5C])
            case 0x0A: out.append(contentsOf: [0x5C, UInt8(ascii: "n")])
            case 0x0D: out.append(contentsOf: [0x5C, UInt8(ascii: "r")])
            case 0x09: out.append(contentsOf: [0x5C, UInt8(ascii: "t")])
            case 0 ..< 0x20:
                out.append(contentsOf: [0x5C, UInt8(ascii: "u"), 0x30, 0x30, hex[Int(b >> 4)], hex[Int(b & 0xF)]])
            default: out.append(b)
            }
        }
        out.append(UInt8(ascii: "\""))
    }
}

/// Whether bytes are well-formed UTF-8 (no overlong forms, no surrogates,
/// nothing above U+10FFFF).
package enum UTF8Check {
    package static func valid(_ bytes: [UInt8]) -> Bool {
        var i = 0
        let n = bytes.count
        while i < n {
            let b = bytes[i]
            if b < 0x80 { i += 1; continue }
            let count: Int
            var low: UInt8 = 0x80, high: UInt8 = 0xBF
            switch b {
            case 0xC2 ... 0xDF: count = 1
            case 0xE0: count = 2; low = 0xA0
            case 0xE1 ... 0xEC, 0xEE ... 0xEF: count = 2
            case 0xED: count = 2; high = 0x9F
            case 0xF0: count = 3; low = 0x90
            case 0xF1 ... 0xF3: count = 3
            case 0xF4: count = 3; high = 0x8F
            default: return false
            }
            guard i + count < n else { return false }
            for k in 1 ... count {
                let c = bytes[i + k]
                if k == 1 {
                    guard c >= low, c <= high else { return false }
                } else {
                    guard c >= 0x80, c <= 0xBF else { return false }
                }
            }
            i += count + 1
        }
        return true
    }
}
