import Foundation

// How Swift values become session values and back (design §4.5): the
// SDK's own Encoder and Decoder, driven by Codable, checking every value;
// never JSONEncoder (which turns a Date into a number and Data into base64
// text inside structs). Instances of @RutisService types and closures go by
// reference; other Codable values (structs, enums, classes) as data.

/// A Dictionary whose entries cross as a map (its keys are not Strings).
protocol MapEncodable {
    var rutisKeyIsString: Bool { get }
    func rutisEntries() -> [(Any, Any)]
}

extension Dictionary: MapEncodable where Key: Encodable, Value: Encodable {
    var rutisKeyIsString: Bool { Key.self == String.self }
    func rutisEntries() -> [(Any, Any)] { map { ($0.key, $0.value) } }
}

/// A Dictionary decoded from a map (or an object).
protocol MapDecodable {
    static func rutisDecode(_ entries: [(Value, Value)], _ decoder: ValueDecoderImpl) throws -> Any
}

extension Dictionary: MapDecodable where Key: Decodable, Value: Decodable {
    static func rutisDecode(_ entries: [(Rutis.ValueSlot, Rutis.ValueSlot)], _ decoder: ValueDecoderImpl) throws -> Any {
        var out: [Key: Value] = [:]
        for (index, (key, value)) in entries.enumerated() {
            let k = try decoder.nested(key, IndexKey(index)).decode(Key.self)
            let v = try decoder.nested(value, IndexKey(index)).decode(Value.self)
            if out.updateValue(v, forKey: k) != nil {
                throw ValueError("duplicate map key \(k): an entry would be lost")
            }
        }
        return out
    }
}

/// A Set whose elements cross as a set.
protocol SetEncodable {
    func rutisElements() -> [Any]
}

extension Set: SetEncodable where Element: Encodable {
    func rutisElements() -> [Any] { Array(self) }
}

protocol SetDecodable {
    static func rutisDecode(_ elements: [Value], _ decoder: ValueDecoderImpl) throws -> Any
}

extension Set: SetDecodable where Element: Decodable {
    static func rutisDecode(_ elements: [Rutis.ValueSlot], _ decoder: ValueDecoderImpl) throws -> Any {
        var out = Set<Element>()
        for (index, element) in elements.enumerated() {
            let e = try decoder.nested(element, IndexKey(index)).decode(Element.self)
            if !out.insert(e).inserted { throw ValueError("duplicate set element \(e): an element would be lost") }
        }
        return out
    }
}

/// An Optional, seen through `Any`.
protocol OptionalProtocol {
    static var wrappedType: Any.Type { get }
    static var rutisNone: Any { get }
    static func rutisSome(_ value: Any) -> Any
}

extension Optional: OptionalProtocol {
    static var wrappedType: Any.Type { Wrapped.self }
    static var rutisNone: Any { Wrapped?.none as Any }
    static func rutisSome(_ value: Any) -> Any { Wrapped?.some(value as! Wrapped) as Any }
}

extension Rutis {
    /// A value as the session holds it (internal to the SDK).
    package typealias ValueSlot = Value
}

struct IndexKey: CodingKey {
    let intValue: Int?
    let stringValue: String
    init(_ index: Int) {
        intValue = index
        stringValue = "[\(index)]"
    }
    init?(intValue: Int) { self.init(intValue) }
    init?(stringValue: String) { nil }
}

struct NameKey: CodingKey {
    let stringValue: String
    var intValue: Int? { nil }
    init(_ name: String) { stringValue = name }
    init?(stringValue: String) { self.init(stringValue) }
    init?(intValue: Int) { nil }
}

/// Converts between Swift values and session values.
package enum ValueCodec {
    // ── Encoding ─────────────────────────────────────────────────────

    /// `value` as a session value: what a parameter or result of type `T`
    /// crosses as.
    package static func encode<T>(_ value: T) throws -> Value {
        if T.self == Void.self { return .undefined }
        return try encodeAny(value, depth: 0)
    }

    /// `value`, of whatever type, as a session value.
    package static func encodeAny(_ value: Any) throws -> Value { try encodeAny(value, depth: 0) }

    static func encodeAny(_ value: Any, depth: Int) throws -> Value {
        if depth > maxDepth + 1 { throw tooDeep }
        // nil, through any number of Optionals.
        let mirror = Mirror(reflecting: value)
        if mirror.displayStyle == .optional {
            guard let wrapped = mirror.children.first?.value else { return .null }
            return try encodeAny(wrapped, depth: depth)
        }
        if let special = try encodeSpecial(value, depth: depth) { return special }
        if let encodable = value as? Encodable { return try encodeEncodable(encodable, depth: depth) }
        switch value {
        case let list as [Any]:
            return .list(of: try list.map { try encodeAny($0, depth: depth + 1) })
        case let object as [String: Any]:
            return .record(of: try object.sorted { $0.key < $1.key }.map { ($0.key, try encodeAny($0.value, depth: depth + 1)) })
        case let map as [AnyHashable: Any]:
            let entries = try map.map { (try encodeAny($0.key.base, depth: depth + 1), try encodeAny($0.value, depth: depth + 1)) }
            return .map(entries.sorted { keyOrder($0.0, $1.0) })
        case let set as Set<AnyHashable>:
            return .set(try set.map { try encodeAny($0.base, depth: depth + 1) }.sorted(by: keyOrder))
        default:
            throw ValueError("a \(type(of: value)) cannot cross processes: it is neither Codable, a @RutisService instance, nor a value rutis knows")
        }
    }

    private static var tooDeep: ValueError {
        ValueError("value nested deeper than \(maxDepth) levels, the most a value crossing processes may nest (a cyclic reference?)")
    }

    /// The values the encoder takes as they are, before Codable.
    static func encodeSpecial(_ value: Any, depth: Int) throws -> Value? {
        switch value {
        case let value as Value: return value
        case is Void: return .undefined
        case let encoded as Rutis.Encoded: return encoded.value
        case let bool as Bool: return .data(.bool(bool))
        case let string as String: return .data(.string(string))
        case let int as Int: return integer(int)
        case let int as Int64: return integer(int)
        case let int as UInt64: return integer(int)
        case let int as UInt: return integer(int)
        case let int as Int32: return integer(int)
        case let int as UInt32: return integer(int)
        case let int as Int16: return integer(int)
        case let int as UInt16: return integer(int)
        case let int as Int8: return integer(int)
        case let int as UInt8: return integer(int)
        case let double as Double: return try number(double)
        case let float as Float: return try number(Double(float))
        case let date as Date: return .instant(Instant(date))
        case let data as Data: return .bytes(data)
        case let uuid as UUID: return .uuid(uuid.uuidString.lowercased())
        case let decimal as Decimal: return .decimal(try TagText.decimal(decimal))
        case let url as URL: return .data(.string(url.absoluteString))
        case let value as Rutis.DateTime: return .dateTime(value)
        case let value as Rutis.CalendarDate: return .date(value)
        case let value as Rutis.TimeOfDay: return .time(value)
        case let value as Rutis.BigInt: return .bigint(value)
        case let function as Rutis.RemoteFunction: return function.value
        // An object of another process, handed on: that object again (#687).
        case let client as Rutis.Client where client.object != nil: return .remoteObject(client.object!)
        case let proxy as any RutisProxy where proxy.rutisClient.object != nil: return .remoteObject(proxy.rutisClient.object!)
        // A plugin definition crosses as one (a `plugin` reference), to be
        // mounted by what receives it (plugin values, R1).
        case let definition as Rutis.Definition: return .plugin(definition)
        case let service as RutisExportable: return .object(ServiceObject(service))
        case let error as Error:
            return .data(.object([("name", .string(errorName(error))), ("message", .string(errorMessage(error)))]))
        case let map as MapEncodable where !map.rutisKeyIsString:
            let entries = try map.rutisEntries().map { (try encodeAny($0.0, depth: depth + 1), try encodeAny($0.1, depth: depth + 1)) }
            return .map(entries.sorted { keyOrder($0.0, $1.0) })
        case let set as SetEncodable:
            return .set(try set.rutisElements().map { try encodeAny($0, depth: depth + 1) }.sorted(by: keyOrder))
        default:
            return nil
        }
    }

    static func integer(_ value: some BinaryInteger) -> Value {
        if value <= 9_007_199_254_740_991, value >= -9_007_199_254_740_991 { return .data(.int(value)) }
        // Beyond ±(2^53−1): a bigint, by itself (design V6).
        return .bigint(Rutis.BigInt(value))
    }

    static func number(_ value: Double) throws -> Value {
        guard let json = JSON.double(value) else {
            throw ValueError("a number that is not finite (\(value)) cannot cross")
        }
        return .data(json)
    }

    static func encodeEncodable(_ value: Encodable, depth: Int) throws -> Value {
        let encoder = ValueEncoderImpl(depth: depth)
        try value.encode(to: encoder)
        return try encoder.result()
    }

    // ── Decoding ─────────────────────────────────────────────────────

    /// `value` as a `T`: what a parameter or result declared `T` receives.
    package static func decode<T>(_ type: T.Type, from value: Value) throws -> T {
        if T.self == Void.self { return () as! T }
        if !decodes(T.self) {
            // A type of Swift's own values ([String: Any?], [Any?], ...): the
            // value as Any, cast.
            if value.isNullish, let optional = T.self as? OptionalProtocol.Type { return optional.rutisNone as! T }
            if let native = native(value) as? T { return native }
            throw ValueError("cannot decode \(value.described) into \(T.self)")
        }
        let decoded = try decodeAny(type, value, codingPath: [])
        guard let typed = decoded as? T else {
            throw ValueError("cannot decode \(value.described) into \(T.self)")
        }
        return typed
    }

    /// Whether `decodeAny` takes `type`: Any, Decodable types, Optionals of
    /// them, and the session's own.
    static func decodes(_ type: Any.Type) -> Bool {
        if type == Any.self || type == Value.self || type == Rutis.RemoteFunction.self || type == Rutis.Definition.self
            || type == Rutis.Client.self || type is Decodable.Type { return true }
        if let optional = type as? OptionalProtocol.Type { return decodes(optional.wrappedType) }
        return false
    }

    /// `value` as a value of the type `type`.
    static func decodeAny(_ type: Any.Type, _ value: Value, codingPath: [CodingKey]) throws -> Any {
        if type == Any.self {
            guard let native = native(value) else { throw ValueError("expected a value, got \(value.described)") }
            return native
        }
        if let optional = type as? OptionalProtocol.Type {
            if value.isNullish { return optional.rutisNone }
            return optional.rutisSome(try decodeAny(optional.wrappedType, value, codingPath: codingPath))
        }
        if type == Value.self { return value }
        if let decodable = type as? Decodable.Type {
            return try ValueDecoderImpl(value, codingPath: codingPath).decodeExistential(decodable)
        }
        if type == Rutis.Client.self, case let .remoteObject(far) = value { return Rutis.Client.of(far) }
        if type == Rutis.RemoteFunction.self {
            if case let .remoteFunction(far) = value { return Rutis.RemoteFunction(far) }
            if case let .function(function) = value { return Rutis.RemoteFunction(local: function) }
        }
        if type == Rutis.Definition.self {
            switch value {
            case let .plugin(definition): return definition
            case let .remotePlugin(far):
                guard let local = far.local else { throw ValueError("a plugin definition of another process can only be handed back to it") }
                return local
            default: throw ValueError("expected a plugin definition, got \(value.described)")
            }
        }
        throw ValueError("a value cannot be decoded into \(type): it is not Decodable")
    }

    /// A received value as Swift's own: numbers as Int or Double, objects as
    /// [String: Any?], lists as [Any?], maps as [AnyHashable: Any?], sets as
    /// Set<AnyHashable>, the tagged values as their types, functions as
    /// Rutis.RemoteFunction; nil for null and undefined.
    package static func native(_ value: Value) -> Any? {
        switch value {
        case .undefined: return nil
        case let .data(json): return native(json)
        case let .list(items): return items.map(native)
        case let .record(fields):
            var out: [String: Any?] = [:]
            for (key, item) in fields { out[key] = .some(native(item)) }
            return out
        case let .instant(v): return v.date
        case let .dateTime(v): return v
        case let .date(v): return v
        case let .time(v): return v
        case let .bytes(v): return v
        case let .bigint(v):
            // An integer that fits Int is an Int: a bigint in range is one.
            return v
        case let .decimal(text): return (try? TagText.decimal(text)) ?? text
        case let .uuid(text): return UUID(uuidString: text) ?? text
        case let .map(entries):
            var out: [AnyHashable: Any?] = [:]
            for (key, item) in entries {
                guard let k = native(key) as? AnyHashable else { continue }
                out[k] = .some(native(item))
            }
            return out
        case let .set(elements):
            var out = Set<AnyHashable>()
            for element in elements {
                if let e = native(element) as? AnyHashable { out.insert(e) }
            }
            return out
        case let .remoteFunction(far): return Rutis.RemoteFunction(far)
        case let .remoteObject(far): return Rutis.Client.of(far)
        case .remoteFuture: return nil
        case let .function(function): return Rutis.RemoteFunction(local: function)
        case let .object(object): return (object as? ServiceObject)?.service ?? object
        case let .plugin(definition): return definition
        case let .remotePlugin(far): return far.local
        case .signal: return nil
        }
    }

    static func native(_ json: JSON) -> Any? {
        switch json {
        case .null: return nil
        case let .bool(b): return b
        case let .number(text):
            if JSONNumber.isIntegerLiteral(text), let int = Int(text) { return int }
            return Double(text) ?? 0
        case let .string(s): return s
        case let .array(items): return items.map { native($0) }
        case let .object(fields):
            var out: [String: Any?] = [:]
            for (key, item) in fields { out[key] = .some(native(item)) }
            return out
        }
    }
}

extension Rutis {
    /// A value encoded for the session: what generated code passes as an
    /// argument or returns as a result.
    public struct Encoded: @unchecked Sendable {
        package let value: Value
        package init(_ value: Value) { self.value = value }
    }

    /// `value` encoded as a parameter or result of type `T` crosses: data,
    /// a tagged value, or a reference (an instance of a @RutisService type).
    public static func encode<T>(_ value: T) throws -> Encoded {
        Encoded(try ValueCodec.encode(value))
    }

    /// A function of another process, received where a value of type `Any`
    /// was expected.
    public final class RemoteFunction: @unchecked Sendable {
        /// The function as the session holds it.
        let value: Value
        init(_ far: FarFunction) { value = .remoteFunction(far) }
        init(local function: SessionFunction) { value = .function(function) }

        /// Calls it with `args` and returns its result as `Any?`.
        @discardableResult
        public func callAsFunction(_ args: Any?...) async throws -> Any? {
            try await call(args, as: Any?.self)
        }

        /// Calls it with `args` and decodes its result into `R`.
        public func call<R>(_ args: [Any?], as type: R.Type = R.self) async throws -> R {
            let values = try args.map { try ValueCodec.encodeAny($0 as Any) }
            let result: Value
            switch value {
            case let .remoteFunction(far): result = try await settle(try await far.call(values, sync: false))
            case let .function(function): result = try await function.body(values)
            default: result = .undefined
            }
            return try ValueCodec.decode(R.self, from: result)
        }
    }
}

/// The order a Swift Dictionary's or Set's entries are sent in (they have
/// none of their own): numbers by value, then the rest by kind and text, so
/// the same value is always sent alike.
func keyOrder(_ a: Value, _ b: Value) -> Bool {
    func rank(_ v: Value) -> (Int, Double, String) {
        switch v {
        case let .data(.number(text)): return (0, Double(text) ?? 0, text)
        case let .bigint(n): return (0, Double(n.description) ?? 0, n.description)
        case let .data(.string(s)): return (1, 0, s)
        case let .data(.bool(b)): return (2, b ? 1 : 0, "")
        default: return (3, 0, (try? taggedText(v)).flatMap { $0 } ?? v.described)
        }
    }
    let (x, y) = (rank(a), rank(b))
    if x.0 != y.0 { return x.0 < y.0 }
    if x.1 != y.1 { return x.1 < y.1 }
    return x.2 < y.2
}
