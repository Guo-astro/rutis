// What is left of a row's config here: decoding it. The host validates it
// against its plugin's JSON Schema and fills its defaults before sending it
// (`rutis::config::check`, platform cleanup design §7.5); a decoder error is
// the schema and the type disagreeing (config schema design §3.3).
import Foundation

/// A key of an object or an index of an array.
package enum ConfigPathSegment: Equatable, Sendable {
    case key(String)
    case index(Int)
}

/// One problem: where it is, and its sentence (path included).
package struct ConfigIssue: Equatable, Sendable {
    package var path: [ConfigPathSegment]
    package var message: String
}

package enum ConfigSchema {
    /// At most this many problems are written; the rest as one last line.
    static let shown = 20

    /// The lines a config error is written as: `config: <problem>`, at most
    /// 20, then `config: and N more problems`.
    package static func lines(_ issues: [ConfigIssue]) -> [String] {
        var lines = issues.prefix(shown).map { "config: \($0.message)" }
        if issues.count > shown { lines.append("config: and \(issues.count - shown) more problems") }
        return lines
    }

    /// How a path is written: `home.lat`, `cities[2]`, `headers["x-api"]`;
    /// `config` for the root, and before a first index or non-identifier key.
    package static func pathText(_ path: [ConfigPathSegment]) -> String {
        var out = ""
        for (position, segment) in path.enumerated() {
            let first = position == 0 ? "config" : ""
            switch segment {
            case let .index(index): out += "\(first)[\(index)]"
            case let .key(key) where identifier(key): out += position == 0 ? key : ".\(key)"
            case let .key(key): out += "\(first)[\(quoted(key, limit: nil))]"
            }
        }
        return out.isEmpty ? "config" : out
    }

    static func identifier(_ key: String) -> Bool {
        guard let first = key.unicodeScalars.first else { return false }
        func letter(_ c: Unicode.Scalar) -> Bool { (c >= "A" && c <= "Z") || (c >= "a" && c <= "z") || c == "_" }
        return letter(first) && key.unicodeScalars.allSatisfy { letter($0) || ($0 >= "0" && $0 <= "9") }
    }

    /// `s` as a JSON string, its content cut to `limit` scalar values with
    /// `…`; `"`, `\` and control characters escaped, everything else as is.
    static func quoted(_ s: String, limit: Int?) -> String {
        let scalars = Array(s.unicodeScalars)
        var out = "\""
        for c in scalars.prefix(limit ?? scalars.count) {
            switch c {
            case "\"": out += "\\\""
            case "\\": out += "\\\\"
            case "\n": out += "\\n"
            case "\r": out += "\\r"
            case "\t": out += "\\t"
            case "\u{8}": out += "\\b"
            case "\u{c}": out += "\\f"
            case _ where c.value < 0x20: out += String(format: "\\u%04x", c.value)
            default: out.unicodeScalars.append(c)
            }
        }
        if let limit, scalars.count > limit { out += "…" }
        return out + "\""
    }

    /// The error a row fails with when its config does not match: a
    /// BindingError with code `bad-config`, its message the lines of
    /// `lines`, its data `{ issues: [{ path, message }] }` (design §5.2).
    package static func error(_ issues: [ConfigIssue]) -> RutisError {
        let data: [[String: Any]] = issues.map { problem in
            ["path": problem.path.map { segment -> Any in
                switch segment {
                case let .key(key): return key
                case let .index(index): return index
                }
            }, "message": problem.message]
        }
        return RutisError(name: RutisError.Name.bindingError, message: lines(issues).joined(separator: "\n"),
                          code: "bad-config", data: ["issues": data])
    }

    /// Where a decoder error is, as a config path.
    package static func path(of error: Error) -> [ConfigPathSegment] {
        let keys: [CodingKey]
        switch error as? DecodingError {
        case let .keyNotFound(key, context)?: keys = context.codingPath + [key]
        case let .typeMismatch(_, context)?, let .valueNotFound(_, context)?, let .dataCorrupted(context)?: keys = context.codingPath
        default: keys = []
        }
        return keys.map { key in key.intValue.map(ConfigPathSegment.index) ?? .key(key.stringValue) }
    }

    package static func issue(_ path: [ConfigPathSegment], _ reason: String) -> ConfigIssue {
        ConfigIssue(path: path, message: "\(pathText(path)) \(reason)")
    }
}

/// A problem `@RutisConfig`'s decoding names: where, and its reason.
struct ConfigProblem: Error {
    let path: [ConfigPathSegment]
    let reason: String
}

extension Rutis._Generated {
    /// A key of a configuration type's own decoding (`@RutisConfig`).
    public struct ConfigKey: CodingKey, ExpressibleByStringLiteral {
        public var stringValue: String
        public var intValue: Int? { nil }
        public init(stringValue: String) { self.stringValue = stringValue }
        public init?(intValue _: Int) { nil }
        public init(stringLiteral value: String) { stringValue = value }
    }

    /// The property `key` of a configuration, as `@RutisConfig`'s
    /// `init(from:)` decodes it: missing (or null), its default when it has
    /// one, nil when optional, else `is required`; of another type,
    /// `should be <type>, got <value>`, as the host's check says.
    public static func config<T: Decodable>(_ container: KeyedDecodingContainer<ConfigKey>, _ key: ConfigKey, as _: T.Type,
                                            default fallback: @autoclosure () -> T?) throws -> T
    {
        let path = (container.codingPath + [key]).map { key in key.intValue.map(ConfigPathSegment.index) ?? .key(key.stringValue) }
        if !container.contains(key) || ((try? container.decodeNil(forKey: key)) ?? false) {
            if let fallback = fallback() { return fallback }
            if let optional = T.self as? OptionalProtocol.Type, let none = optional.rutisNone as? T { return none }
            throw ConfigProblem(path: path, reason: "is required")
        }
        do {
            return try container.decode(T.self, forKey: key)
        } catch let error as DecodingError {
            // A problem inside the value is the decoder's to place.
            guard ConfigSchema.path(of: error) == path else { throw error }
            guard let raw = try? container.decode(RawValue.self, forKey: key).value, case let .data(json) = raw,
                  let expected = expectedText(Schema.of(T.self).json)
            else { throw error }
            throw ConfigProblem(path: path, reason: "should be \(expected), got \(valueText(json))")
        }
    }

    /// A value as the decoder holds it.
    struct RawValue: Decodable {
        let value: Value
        init(from decoder: Decoder) throws {
            guard let decoder = decoder as? ValueDecoderImpl else { throw ValueError("not a rutis value") }
            value = decoder.value
        }
    }

    /// `a string`, `an integer or null`, … of a schema's `type`.
    static func expectedText(_ schema: JSON) -> String? {
        let names: [String]
        switch schema["type"] {
        case let .string(name)?: names = [name]
        case let .array(items)?: names = items.compactMap(\.stringValue)
        default: return nil
        }
        let texts = names.map { name -> String in
            switch name {
            case "string": return "a string"
            case "number": return "a number"
            case "integer": return "an integer"
            case "boolean": return "a boolean"
            case "object": return "an object"
            case "array": return "an array"
            default: return "null"
            }
        }
        switch texts.count {
        case 0: return nil
        case 1: return texts[0]
        case 2: return "\(texts[0]) or \(texts[1])"
        default: return "one of " + texts.joined(separator: ", ")
        }
    }

    /// How a value is written after `got`, as the host writes it.
    static func valueText(_ value: JSON) -> String {
        switch value {
        case .null: return "null"
        case let .bool(b): return b ? "true" : "false"
        case let .number(text): return text
        case let .string(s): return ConfigSchema.quoted(s, limit: 40)
        case let .array(items): return "an array of \(items.count)"
        case .object: return "an object"
        }
    }
}
