import Foundation

/// The tags of the values beyond JSON data; each is declared as the
/// capability `value-<tag>`.
package let valueTags = ["instant", "datetime", "date", "time", "bytes", "map", "set", "bigint", "decimal", "uuid"]

/// The capabilities of the tagged values: this side takes them all.
package let valueCapabilities = valueTags.map { "value-" + $0 }

/// Reads the text of a tagged value other than a map or a set.
package func parseTagged(_ tag: String, _ text: String) throws -> Value {
    switch tag {
    case "instant":
        guard let value = Instant(text) else {
            throw ValueError("invalid instant \(quoted(text)): expected RFC 3339 in UTC, YYYY-MM-DDTHH:MM:SS[.fraction]Z, years 0001–9999")
        }
        return .instant(value)
    case "datetime":
        guard let value = Rutis.Advanced.DateTime(text) else {
            throw ValueError("invalid datetime \(quoted(text)): expected YYYY-MM-DDTHH:MM:SS[.fraction] without a time zone, years 0001–9999")
        }
        return .dateTime(value)
    case "date":
        guard let value = Rutis.Advanced.CalendarDate(text) else {
            throw ValueError("invalid date \(quoted(text)): expected YYYY-MM-DD, years 0001–9999")
        }
        return .date(value)
    case "time":
        guard let value = Rutis.Advanced.TimeOfDay(text) else {
            throw ValueError("invalid time \(quoted(text)): expected HH:MM:SS[.fraction], 24-hour, 0–9 fraction digits")
        }
        return .time(value)
    case "bytes": return .bytes(try TagText.bytes(text))
    case "bigint":
        guard let value = Rutis.Advanced.BigInt(text) else {
            throw ValueError("invalid bigint \(quoted(text)): expected a decimal integer without leading zeros, at most 4300 digits")
        }
        return .bigint(value)
    case "decimal":
        try TagText.checkDecimal(text)
        return .decimal(text)
    case "uuid":
        try TagText.checkUUID(text)
        return .uuid(text)
    default:
        throw ValueError("invalid \(tag)")
    }
}

/// The text of a tagged value other than a map or set, checked.
func taggedText(_ value: Value) throws -> String? {
    switch value {
    case let .instant(v):
        try v.check()
        return v.description
    case let .dateTime(v):
        try v.check()
        return v.description
    case let .date(v):
        try v.check()
        return v.description
    case let .time(v):
        try v.check()
        return v.description
    case let .bytes(v): return v.base64EncodedString()
    case let .bigint(v): return v.description
    case let .decimal(v):
        try TagText.checkDecimal(v)
        return v
    case let .uuid(v):
        try TagText.checkUUID(v)
        return v
    default: return nil
    }
}

// ── Map keys and set elements (design §5) ────────────────────────────────

private let maxSafeInteger = Rutis.Advanced.BigInt(9_007_199_254_740_991)

/// A map key or set element as the languages compare it: numbers by value
/// (`1` and `1.0` are one key, as are `0` and `-0`), the rest by kind and
/// text. `what` is "map key" or "set element".
private func keyOf(_ value: Value, _ what: String) throws -> String {
    switch value {
    case let .data(.string(s)): return "s" + s
    case let .data(.bool(b)): return "b\(b)"
    case let .data(.number(text)):
        if JSONNumber.isIntegerLiteral(text), let integer = Rutis.Advanced.BigInt(text) ?? Rutis.Advanced.BigInt(text.replacingOccurrences(of: "-0", with: "0")) {
            return try integerKey(integer, what)
        }
        guard let double = Double(text), double.isFinite else { throw ValueError("a \(what) cannot be \(text)") }
        return floatKey(double)
    case let .bigint(v): return "B" + v.description
    case let .dateTime(v): return "T" + v.description
    case let .date(v): return "D" + v.description
    case let .time(v): return "H" + v.description
    case let .decimal(v): return "M" + v
    case let .uuid(v): return "U" + v
    default:
        throw ValueError("a \(what) cannot be \(value.described): only strings, finite numbers, booleans, bigint, datetime, date, time, decimal and uuid values can (an instant, bytes, null, an array, an object, a map, a set or a reference cannot)")
    }
}

private func integerKey(_ n: Rutis.Advanced.BigInt, _ what: String) throws -> String {
    guard n.isSafe else {
        throw ValueError("a \(what) cannot be the integer \(n), beyond ±(2^53-1): send it as a bigint")
    }
    return n.magnitude == "0" ? "n0" : "n\(n)"
}

private func floatKey(_ f: Double) -> String {
    if f == f.rounded(.towardZero), abs(f) <= 9_007_199_254_740_991 { return "n\(Int64(f))" }
    return "f" + String(f.bitPattern, radix: 16)
}

private func shownKey(_ value: Value) -> String {
    var text: String
    switch value {
    case let .data(json): text = JSONWriter.string(json)
    case let .bigint(v): text = v.description
    case let .dateTime(v): text = v.description
    case let .date(v): text = v.description
    case let .time(v): text = v.description
    case let .decimal(v): text = v
    case let .uuid(v): text = v
    default: text = value.described
    }
    if text.count > 40 { text = String(text.prefix(40)) + "…" }
    return text
}

/// Checks the keys of a map (`what` "map key") or the elements of a set
/// ("set element"): each of a kind every language takes as a key, and no
/// two equal, since a language that merges them would lose an entry.
package func checkKeys(_ keys: [Value], _ what: String) throws {
    var seen = Set<String>()
    for key in keys {
        let identity = try keyOf(key, what)
        guard seen.insert(identity).inserted else {
            throw ValueError("duplicate \(what) \(shownKey(key)): a map or set whose keys collide would lose entries")
        }
    }
}
