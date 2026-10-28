import Foundation

// The value types Swift lacks (docs/design-value-types-2026-10-15.md §2):
// a calendar date, a time of day, a local date and time, and a big integer.
// Everything else crosses as a native type: Date (instant), Data (bytes),
// Dictionary (map), Set (set), Decimal (decimal), UUID (uuid).
//
//	| tag      | Swift                       | text on the wire                     |
//	| instant  | Date                        | 2026-10-15T03:22:15.123456Z          |
//	| datetime | Rutis.Advanced.DateTime     | 2026-10-15T14:30:05.25, no time zone |
//	| date     | Rutis.Advanced.CalendarDate | 2026-10-15                           |
//	| time     | Rutis.Advanced.TimeOfDay    | 14:30:05.25, 24-hour, no time zone   |
//	| bytes    | Data                        | base64 (RFC 4648 §4, padded)         |
//	| bigint   | Rutis.Advanced.BigInt       | decimal integer, at most 4300 digits |
//	| decimal  | Decimal                     | decimal, no exponent, zeros kept     |
//	| uuid     | UUID                        | lowercase 8-4-4-4-12 hex             |

/// The most digits a bigint or decimal may have: Python's default limit on
/// integer text.
let maxDigits = 4300

/// A text an error quotes, at most 40 characters.
func quoted(_ text: String) -> String {
    if text.count <= 40 { return "\"\(text)\"" }
    return "\"\(text.prefix(40))\"…"
}

private func daysIn(_ year: Int, _ month: Int) -> Int {
    switch month {
    case 2: return (year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)) ? 29 : 28
    case 4, 6, 9, 11: return 30
    default: return 31
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// days_from_civil).
func daysFromCivil(_ y: Int, _ m: Int, _ d: Int) -> Int64 {
    let y = Int64(m <= 2 ? y - 1 : y)
    let era = (y >= 0 ? y : y - 399) / 400
    let yoe = y - era * 400
    let mp = Int64(m > 2 ? m - 3 : m + 9)
    let doy = (153 * mp + 2) / 5 + Int64(d) - 1
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy
    return era * 146_097 + doe - 719_468
}

/// The date of days since 1970-01-01.
func civilFromDays(_ z: Int64) -> (Int, Int, Int) {
    let z = z + 719_468
    let era = (z >= 0 ? z : z - 146_096) / 146_097
    let doe = z - era * 146_097
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365
    let y = yoe + era * 400
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100)
    let mp = (5 * doy + 2) / 153
    let d = doy - (153 * mp + 2) / 5 + 1
    let m = mp < 10 ? mp + 3 : mp - 9
    return (Int(m <= 2 ? y + 1 : y), Int(m), Int(d))
}

/// Reads the ASCII text of a value.
struct TextCursor {
    let bytes: [UInt8]
    var at = 0

    init(_ text: String) { bytes = Array(text.utf8) }

    mutating func digits(_ count: Int) -> Int? {
        guard at + count <= bytes.count else { return nil }
        var value = 0
        for b in bytes[at ..< at + count] {
            guard b >= 0x30, b <= 0x39 else { return nil }
            value = value * 10 + Int(b - 0x30)
        }
        at += count
        return value
    }

    mutating func byte(_ c: Character) -> Bool {
        if at < bytes.count, bytes[at] == c.asciiValue! {
            at += 1
            return true
        }
        return false
    }

    /// An optional `.` and 1–9 digits, in nanoseconds.
    mutating func fraction() -> Int? {
        guard byte(".") else { return 0 }
        let start = at
        while at < bytes.count, bytes[at] >= 0x30, bytes[at] <= 0x39 { at += 1 }
        let count = at - start
        guard count >= 1, count <= 9 else { return nil }
        var nanos = 0
        for i in 0 ..< 9 {
            nanos *= 10
            if i < count { nanos += Int(bytes[start + i] - 0x30) }
        }
        return nanos
    }

    var end: Bool { at == bytes.count }

    mutating func date() -> Rutis.Advanced.CalendarDate? {
        guard let year = digits(4), byte("-"), let month = digits(2), byte("-"), let day = digits(2) else { return nil }
        let date = Rutis.Advanced.CalendarDate(year: year, month: month, day: day)
        return date.isValid ? date : nil
    }

    mutating func time() -> Rutis.Advanced.TimeOfDay? {
        guard let hour = digits(2), byte(":"), let minute = digits(2), byte(":"), let second = digits(2),
              let nanosecond = fraction() else { return nil }
        let time = Rutis.Advanced.TimeOfDay(hour: hour, minute: minute, second: second, nanosecond: nanosecond)
        return time.isValid ? time : nil
    }

    mutating func dateTime() -> Rutis.Advanced.DateTime? {
        guard let date = date(), byte("T"), let time = time() else { return nil }
        return Rutis.Advanced.DateTime(date: date, time: time)
    }
}

/// `.` and the digits of `nanos` without trailing zeros; "" for 0.
func fractionText(_ nanos: Int) -> String {
    if nanos == 0 { return "" }
    var digits = String(nanos)
    digits = String(repeating: "0", count: 9 - digits.count) + digits
    while digits.hasSuffix("0") { digits.removeLast() }
    return "." + digits
}

private func pad(_ value: Int, _ width: Int) -> String {
    let text = String(value)
    return text.count >= width ? text : String(repeating: "0", count: width - text.count) + text
}

extension Rutis.Advanced {
    /// A calendar date without a time zone (tag `date`), years 0001–9999.
    /// Swift has no such type; it converts to and from DateComponents.
    public struct CalendarDate: Hashable, Comparable, Sendable, Codable, CustomStringConvertible, LosslessStringConvertible {
        public var year: Int
        /// 1–12.
        public var month: Int
        /// 1–31.
        public var day: Int

        public init(year: Int, month: Int, day: Int) {
            self.year = year
            self.month = month
            self.day = day
        }

        /// Reads `YYYY-MM-DD`.
        public init?(_ description: String) {
            var cursor = TextCursor(description)
            guard let date = cursor.date(), cursor.end else { return nil }
            self = date
        }

        /// The date of `components` (its year, month and day).
        public init?(_ components: DateComponents) {
            guard let year = components.year, let month = components.month, let day = components.day else { return nil }
            self.init(year: year, month: month, day: day)
            guard isValid else { return nil }
        }

        /// The date of `date` in `timeZone`.
        public init(_ date: Date, in timeZone: TimeZone) {
            var calendar = Calendar(identifier: .gregorian)
            calendar.timeZone = timeZone
            let c = calendar.dateComponents([.year, .month, .day], from: date)
            self.init(year: c.year ?? 1, month: c.month ?? 1, day: c.day ?? 1)
        }

        /// Whether the date exists, in years 0001–9999.
        public var isValid: Bool {
            year >= 1 && year <= 9999 && month >= 1 && month <= 12 && day >= 1 && day <= daysIn(year, month)
        }

        /// Its year, month and day, in the Gregorian calendar.
        public var dateComponents: DateComponents {
            DateComponents(calendar: Calendar(identifier: .gregorian), year: year, month: month, day: day)
        }

        /// `YYYY-MM-DD`.
        public var description: String { "\(pad(year, 4))-\(pad(month, 2))-\(pad(day, 2))" }

        public static func < (a: Self, b: Self) -> Bool { (a.year, a.month, a.day) < (b.year, b.month, b.day) }

        public init(from decoder: Decoder) throws {
            let text = try decoder.singleValueContainer().decode(String.self)
            guard let value = Self(text) else {
                throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "invalid date \(quoted(text)): expected YYYY-MM-DD"))
            }
            self = value
        }

        public func encode(to encoder: Encoder) throws {
            var container = encoder.singleValueContainer()
            try container.encode(description)
        }

        func check() throws {
            guard isValid else { throw ValueError("invalid date \(description): no such day in years 0001–9999") }
        }
    }

    /// A time of day without a time zone (tag `time`), to the nanosecond:
    /// 24-hour, no leap second.
    public struct TimeOfDay: Hashable, Comparable, Sendable, Codable, CustomStringConvertible, LosslessStringConvertible {
        /// 0–23.
        public var hour: Int
        /// 0–59.
        public var minute: Int
        /// 0–59.
        public var second: Int
        /// 0–999 999 999.
        public var nanosecond: Int

        public init(hour: Int, minute: Int, second: Int = 0, nanosecond: Int = 0) {
            self.hour = hour
            self.minute = minute
            self.second = second
            self.nanosecond = nanosecond
        }

        /// Reads `HH:MM:SS` with an optional fraction of 1–9 digits.
        public init?(_ description: String) {
            var cursor = TextCursor(description)
            guard let time = cursor.time(), cursor.end else { return nil }
            self = time
        }

        /// The time of `components` (hour, minute, second, nanosecond; a
        /// missing one is 0).
        public init?(_ components: DateComponents) {
            self.init(hour: components.hour ?? 0, minute: components.minute ?? 0, second: components.second ?? 0,
                      nanosecond: components.nanosecond ?? 0)
            guard isValid else { return nil }
        }

        public var isValid: Bool {
            (0 ... 23).contains(hour) && (0 ... 59).contains(minute) && (0 ... 59).contains(second)
                && (0 ..< 1_000_000_000).contains(nanosecond)
        }

        public var dateComponents: DateComponents {
            DateComponents(hour: hour, minute: minute, second: second, nanosecond: nanosecond)
        }

        /// `HH:MM:SS` and the fraction without trailing zeros, if any.
        public var description: String {
            "\(pad(hour, 2)):\(pad(minute, 2)):\(pad(second, 2))\(fractionText(nanosecond))"
        }

        public static func < (a: Self, b: Self) -> Bool {
            (a.hour, a.minute, a.second, a.nanosecond) < (b.hour, b.minute, b.second, b.nanosecond)
        }

        public init(from decoder: Decoder) throws {
            let text = try decoder.singleValueContainer().decode(String.self)
            guard let value = Self(text) else {
                throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "invalid time \(quoted(text)): expected HH:MM:SS[.fraction]"))
            }
            self = value
        }

        public func encode(to encoder: Encoder) throws {
            var container = encoder.singleValueContainer()
            try container.encode(description)
        }

        func check() throws {
            guard isValid else {
                throw ValueError("invalid time \(pad(hour, 2)):\(pad(minute, 2)):\(pad(second, 2)) (+\(nanosecond) ns)")
            }
        }
    }

    /// A local date and time without a time zone (tag `datetime`): passed as
    /// it is, never read as a point in time in some zone.
    public struct DateTime: Hashable, Comparable, Sendable, Codable, CustomStringConvertible, LosslessStringConvertible {
        public var date: CalendarDate
        public var time: TimeOfDay

        public init(date: CalendarDate, time: TimeOfDay) {
            self.date = date
            self.time = time
        }

        public init(year: Int, month: Int, day: Int, hour: Int = 0, minute: Int = 0, second: Int = 0, nanosecond: Int = 0) {
            self.init(date: CalendarDate(year: year, month: month, day: day),
                      time: TimeOfDay(hour: hour, minute: minute, second: second, nanosecond: nanosecond))
        }

        /// Reads `YYYY-MM-DDTHH:MM:SS[.fraction]`, no time zone.
        public init?(_ description: String) {
            var cursor = TextCursor(description)
            guard let value = cursor.dateTime(), cursor.end else { return nil }
            self = value
        }

        public init?(_ components: DateComponents) {
            guard let date = CalendarDate(components), let time = TimeOfDay(components) else { return nil }
            self.init(date: date, time: time)
        }

        /// The date and time of day of `date` in `timeZone`.
        public init(_ date: Date, in timeZone: TimeZone) {
            var calendar = Calendar(identifier: .gregorian)
            calendar.timeZone = timeZone
            let c = calendar.dateComponents([.year, .month, .day, .hour, .minute, .second, .nanosecond], from: date)
            self.init(year: c.year ?? 1, month: c.month ?? 1, day: c.day ?? 1, hour: c.hour ?? 0, minute: c.minute ?? 0,
                      second: c.second ?? 0, nanosecond: c.nanosecond ?? 0)
        }

        public var isValid: Bool { date.isValid && time.isValid }

        public var dateComponents: DateComponents {
            DateComponents(calendar: Calendar(identifier: .gregorian), year: date.year, month: date.month, day: date.day,
                           hour: time.hour, minute: time.minute, second: time.second, nanosecond: time.nanosecond)
        }

        public var description: String { "\(date)T\(time)" }

        public static func < (a: Self, b: Self) -> Bool { (a.date, a.time) < (b.date, b.time) }

        public init(from decoder: Decoder) throws {
            let text = try decoder.singleValueContainer().decode(String.self)
            guard let value = Self(text) else {
                throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "invalid datetime \(quoted(text))"))
            }
            self = value
        }

        public func encode(to encoder: Encoder) throws {
            var container = encoder.singleValueContainer()
            try container.encode(description)
        }

        func check() throws {
            try date.check()
            try time.check()
        }
    }

    /// An integer of any size (tag `bigint`), at most 4300 digits. Integers
    /// beyond ±(2^53−1) cross as one by themselves: an `Int64` or `UInt64`
    /// needs no BigInt; a received bigint decodes into any integer type that
    /// holds it.
    public struct BigInt: Hashable, Comparable, Sendable, Codable, CustomStringConvertible, LosslessStringConvertible,
        ExpressibleByIntegerLiteral
    {
        /// Whether it is below zero (never for zero).
        public let isNegative: Bool
        /// Its absolute value's decimal digits, without leading zeros ("0"
        /// for zero).
        let magnitude: String

        public init(_ value: some BinaryInteger) {
            isNegative = value < 0
            let text = String(value)
            magnitude = isNegative ? String(text.dropFirst()) : text
        }

        public init(integerLiteral value: Int) { self.init(value) }

        /// Reads decimal digits with an optional `-`: no leading zeros, no
        /// `-0`, at most 4300 digits.
        public init?(_ description: String) {
            guard let parsed = BigInt.parse(description) else { return nil }
            self = parsed
        }

        private init(negative: Bool, magnitude: String) {
            isNegative = negative
            self.magnitude = magnitude
        }

        static func parse(_ text: String) -> BigInt? {
            var rest = Substring(text)
            let negative = rest.hasPrefix("-")
            if negative { rest = rest.dropFirst() }
            guard !rest.isEmpty, rest.count <= maxDigits, rest.allSatisfy({ $0.isASCII && $0.isNumber }) else { return nil }
            if rest.count > 1, rest.first == "0" { return nil }
            if negative, rest == "0" { return nil }
            return BigInt(negative: negative, magnitude: String(rest))
        }

        /// Decimal, with `-` when negative.
        public var description: String { isNegative ? "-" + magnitude : magnitude }

        /// The value as `T`, when `T` holds it.
        public func exactly<T: FixedWidthInteger>(_: T.Type = T.self) -> T? { T(description, radix: 10) }

        /// Whether it is within ±(2^53−1), the integers every language holds
        /// exactly as a number.
        var isSafe: Bool {
            magnitude.count < 16 || (magnitude.count == 16 && magnitude <= "9007199254740991")
        }

        public static func < (a: Self, b: Self) -> Bool {
            if a.isNegative != b.isNegative { return a.isNegative }
            let less = a.magnitude.count != b.magnitude.count ? a.magnitude.count < b.magnitude.count : a.magnitude < b.magnitude
            return a.isNegative ? (a != b && !less) : less
        }

        public init(from decoder: Decoder) throws {
            let container = try decoder.singleValueContainer()
            if let integer = try? container.decode(Int64.self) {
                self.init(integer)
                return
            }
            let text = try container.decode(String.self)
            guard let value = BigInt(text) else {
                throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "invalid bigint \(quoted(text))"))
            }
            self = value
        }

        public func encode(to encoder: Encoder) throws {
            var container = encoder.singleValueContainer()
            try container.encode(description)
        }
    }
}

/// A value this side cannot send or take: only the call carrying it fails,
/// and the session goes on. It crosses named `Error`.
struct ValueError: Error, CustomStringConvertible, @unchecked Sendable {
    let description: String
    /// bad-value, with `{argument?, part, limit?}`, when it is of a part of
    /// a call (#677): read as any error's `code` and `data` are.
    let code: String?
    let data: [String: Any]?
    init(_ description: String, data: [String: Any]? = nil) {
        self.description = description
        self.data = data
        code = data == nil ? nil : "bad-value"
    }
}

/// A point in time to the nanosecond (tag `instant`), as the session holds
/// it: what a `Date` (about a microsecond) truncates.
package struct Instant: Hashable, Sendable, CustomStringConvertible {
    /// Seconds since 1970-01-01T00:00:00Z.
    package var seconds: Int64
    /// 0–999 999 999.
    package var nanoseconds: Int

    package init(seconds: Int64, nanoseconds: Int) {
        self.seconds = seconds
        self.nanoseconds = nanoseconds
    }

    /// `date`, truncated toward negative infinity to the microsecond.
    package init(_ date: Date) {
        // A Date is a Double of seconds (since 2001; often made from seconds
        // since 1970): a whole microsecond it was made from may read back a
        // hair below it. Within the Doubles' own precision of a whole
        // microsecond, it is that one; otherwise truncated (V7).
        let micros = date.timeIntervalSinceReferenceDate * 1_000_000
        let nearest = micros.rounded()
        let precision = max(date.timeIntervalSince1970.ulp, date.timeIntervalSinceReferenceDate.ulp) * 2_000_000
        let whole = abs(micros - nearest) <= precision ? nearest : micros.rounded(.down)
        let total = Int64(whole) + 978_307_200 * 1_000_000
        var seconds = total / 1_000_000
        var rest = total % 1_000_000
        if rest < 0 {
            seconds -= 1
            rest += 1_000_000
        }
        self.init(seconds: seconds, nanoseconds: Int(rest) * 1000)
    }

    /// As a Date, truncated to the microsecond (V7).
    package var date: Date {
        let micros = (seconds - 978_307_200) * 1_000_000 + Int64(nanoseconds / 1000)
        let whole = micros / 1_000_000
        let rest = micros % 1_000_000
        return Date(timeIntervalSinceReferenceDate: Double(whole) + Double(rest) / 1_000_000)
    }

    /// RFC 3339 in UTC: `YYYY-MM-DDTHH:MM:SS[.fraction]Z`.
    package var description: String {
        var days = seconds / 86400
        var rest = seconds % 86400
        if rest < 0 {
            days -= 1
            rest += 86400
        }
        let (y, m, d) = civilFromDays(days)
        let time = Rutis.Advanced.TimeOfDay(hour: Int(rest / 3600), minute: Int(rest % 3600 / 60), second: Int(rest % 60), nanosecond: nanoseconds)
        return "\(Rutis.Advanced.CalendarDate(year: y, month: m, day: d))T\(time)Z"
    }

    /// Reads RFC 3339 in UTC, with an optional fraction of 1–9 digits.
    package init?(_ text: String) {
        var cursor = TextCursor(text)
        guard let dt = cursor.dateTime(), cursor.byte("Z"), cursor.end else { return nil }
        let days = daysFromCivil(dt.date.year, dt.date.month, dt.date.day)
        self.init(seconds: days * 86400 + Int64(dt.time.hour * 3600 + dt.time.minute * 60 + dt.time.second),
                  nanoseconds: dt.time.nanosecond)
    }

    /// Why it cannot be sent: outside years 0001–9999.
    package func check() throws {
        let first = daysFromCivil(1, 1, 1) * 86400
        let last = daysFromCivil(9999, 12, 31) * 86400 + 86399
        guard seconds >= first, seconds <= last else {
            throw ValueError("the time \(self) is outside years 0001–9999")
        }
    }
}

// ── The texts of bytes, decimals and UUIDs ───────────────────────────────

enum TagText {
    /// Reads padded base64, only in its canonical form: no whitespace,
    /// padding only at the end, unused bits zero.
    static func bytes(_ text: String) throws -> Data {
        let invalid = ValueError("invalid bytes (\(text.utf8.count) characters): expected base64 (RFC 4648 §4) with padding")
        guard text.utf8.count % 4 == 0, let data = Data(base64Encoded: text) else { throw invalid }
        // Foundation takes non-canonical text too: it must read back the same.
        guard data.base64EncodedString() == text else { throw invalid }
        return data
    }

    /// Checks decimal text: `-`, an integer without leading zeros, an
    /// optional fraction, no exponent, at most 4300 digits.
    static func checkDecimal(_ text: String) throws {
        let parts = text.split(separator: ".", maxSplits: 1, omittingEmptySubsequences: false)
        var integer = parts[0]
        if integer.hasPrefix("-") { integer = integer.dropFirst() }
        var ok = !integer.isEmpty && integer.allSatisfy { $0.isASCII && $0.isNumber } && !(integer.count > 1 && integer.first == "0")
        var digits = integer.count
        if parts.count == 2 {
            ok = ok && !parts[1].isEmpty && parts[1].allSatisfy { $0.isASCII && $0.isNumber }
            digits += parts[1].count
        }
        guard ok, digits <= maxDigits else {
            throw ValueError("invalid decimal \(quoted(text)): expected decimal digits with an optional fraction, no exponent, at most 4300 digits")
        }
    }

    /// Checks UUID text: lowercase 8-4-4-4-12 hex.
    static func checkUUID(_ text: String) throws {
        let bytes = Array(text.utf8)
        var ok = bytes.count == 36
        if ok {
            for (i, c) in bytes.enumerated() {
                switch i {
                case 8, 13, 18, 23: ok = ok && c == UInt8(ascii: "-")
                default: ok = ok && ((c >= 0x30 && c <= 0x39) || (c >= 0x61 && c <= 0x66))
                }
            }
        }
        guard ok else { throw ValueError("invalid uuid \(quoted(text)): expected lowercase hex 8-4-4-4-12") }
    }

    /// A Decimal's text as it crosses: no exponent, at most 38 significant
    /// digits (Foundation's), refusing NaN.
    static func decimal(_ value: Decimal) throws -> String {
        guard !value.isNaN else { throw ValueError("a decimal that is not a number (NaN) cannot cross") }
        // Foundation writes plain decimal text (`1.1`, `-0.001`, `100`).
        var text = NSDecimalNumber(decimal: value).stringValue
        if text.hasPrefix(".") { text = "0" + text }
        if text.hasPrefix("-.") { text = "-0" + text.dropFirst() }
        try checkDecimal(text)
        return text
    }

    /// A decimal's text as a Foundation Decimal: it holds 38 significant
    /// digits; more would be lost, and fail.
    static func decimal(_ text: String) throws -> Decimal {
        try checkDecimal(text)
        let significant = text.filter(\.isNumber).drop(while: { $0 == "0" })
        let trimmed = significant.reversed().drop(while: { $0 == "0" })
        guard trimmed.count <= 38, let value = Decimal(string: text, locale: Locale(identifier: "en_US_POSIX")) else {
            throw ValueError("the decimal \(quoted(text)) has more than 38 significant digits, more than a Swift Decimal holds: declare it a String to take its text")
        }
        return value
    }
}
