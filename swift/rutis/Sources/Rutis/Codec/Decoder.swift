import Foundation

/// The SDK's Decoder: session values to Decodable types (design §4.5). It
/// takes the tagged values into their Swift types (a Date from an instant,
/// Data from bytes, a Dictionary from a map, ...), wherever they are.
final class ValueDecoderImpl: Decoder {
    let value: Value
    var codingPath: [CodingKey]
    var userInfo: [CodingUserInfoKey: Any] { [:] }

    init(_ value: Value, codingPath: [CodingKey]) {
        self.value = value
        self.codingPath = codingPath
    }

    func nested(_ value: Value, _ key: CodingKey) -> ValueDecoderImpl {
        ValueDecoderImpl(value, codingPath: codingPath + [key])
    }

    private func mismatch(_ type: Any.Type, _ expected: String) -> DecodingError {
        .typeMismatch(type, .init(codingPath: codingPath, debugDescription: "expected \(expected), got \(value.described)"))
    }

    /// Decodes `type`, taking the types the session knows before Codable.
    func decode<T: Decodable>(_ type: T.Type) throws -> T {
        guard let decoded = try decodeExistential(type) as? T else { throw mismatch(T.self, "\(T.self)") }
        return decoded
    }

    func decodeExistential(_ type: Decodable.Type) throws -> Any {
        if let special = try decodeSpecial(type) { return special }
        if let optional = type as? OptionalProtocol.Type {
            if value.isNullish { return optional.rutisNone }
            return optional.rutisSome(try ValueCodec.decodeAny(optional.wrappedType, value, codingPath: codingPath))
        }
        if let map = type as? MapDecodable.Type {
            switch value {
            case let .map(entries): return try map.rutisDecode(entries, self)
            default:
                // An object: its keys as strings.
                if let fields = value.fields {
                    return try map.rutisDecode(fields.map { (Value.data(.string($0.0)), $0.1) }, self)
                }
                throw mismatch(type, "a map")
            }
        }
        if let set = type as? SetDecodable.Type {
            switch value {
            case let .set(elements): return try set.rutisDecode(elements, self)
            default:
                guard let items = value.items else { throw mismatch(type, "a set") }
                return try set.rutisDecode(items, self)
            }
        }
        return try type.init(from: self)
    }

    /// The types the session knows; nil for any other.
    private func decodeSpecial(_ type: Any.Type) throws -> Any? {
        switch type {
        case is Value.Type: return value
        case is Date.Type:
            guard case let .instant(instant) = value else { throw mismatch(type, "an instant") }
            return instant.date
        case is Data.Type:
            guard case let .bytes(data) = value else { throw mismatch(type, "bytes") }
            return data
        case is UUID.Type:
            switch value {
            case let .uuid(text), let .data(.string(text)):
                guard let uuid = UUID(uuidString: text) else { throw mismatch(type, "a uuid") }
                return uuid
            default: throw mismatch(type, "a uuid")
            }
        case is Decimal.Type:
            switch value {
            case let .decimal(text), let .data(.number(text)):
                return try TagText.decimal(text)
            case let .bigint(v): return try TagText.decimal(v.description)
            default: throw mismatch(type, "a decimal")
            }
        case is URL.Type:
            guard let text = value.string, let url = URL(string: text) else { throw mismatch(type, "a URL") }
            return url
        case is Rutis.DateTime.Type:
            switch value {
            case let .dateTime(v): return v
            case let .data(.string(text)):
                guard let v = Rutis.DateTime(text) else { throw mismatch(type, "a datetime") }
                return v
            default: throw mismatch(type, "a datetime")
            }
        case is Rutis.CalendarDate.Type:
            switch value {
            case let .date(v): return v
            case let .data(.string(text)):
                guard let v = Rutis.CalendarDate(text) else { throw mismatch(type, "a date") }
                return v
            default: throw mismatch(type, "a date")
            }
        case is Rutis.TimeOfDay.Type:
            switch value {
            case let .time(v): return v
            case let .data(.string(text)):
                guard let v = Rutis.TimeOfDay(text) else { throw mismatch(type, "a time") }
                return v
            default: throw mismatch(type, "a time")
            }
        case is Rutis.BigInt.Type:
            switch value {
            case let .bigint(v): return v
            case let .data(.number(text)) where JSONNumber.isIntegerLiteral(text):
                guard let v = Rutis.BigInt(text) else { throw mismatch(type, "an integer") }
                return v
            default: throw mismatch(type, "a bigint")
            }
        case is Rutis.RemoteFunction.Type:
            if case let .remoteFunction(far) = value { return Rutis.RemoteFunction(far) }
            if case let .function(function) = value { return Rutis.RemoteFunction(local: function) }
            throw mismatch(type, "a function")
        default:
            return nil
        }
    }

    func container<Key: CodingKey>(keyedBy _: Key.Type) throws -> KeyedDecodingContainer<Key> {
        guard let fields = value.fields else { throw mismatch([String: Any].self, "an object") }
        return KeyedDecodingContainer(KeyedContainer<Key>(decoder: self, fields: fields))
    }

    func unkeyedContainer() throws -> UnkeyedDecodingContainer {
        switch value {
        case let .set(elements): return UnkeyedContainer(decoder: self, items: elements)
        default:
            guard let items = value.items else { throw mismatch([Any].self, "an array") }
            return UnkeyedContainer(decoder: self, items: items)
        }
    }

    func singleValueContainer() throws -> SingleValueDecodingContainer { SingleContainer(decoder: self) }

    // ── Scalars ──────────────────────────────────────────────────────

    func decodeBool() throws -> Bool {
        guard case let .data(.bool(b)) = value else { throw mismatch(Bool.self, "a boolean") }
        return b
    }

    func decodeString() throws -> String {
        switch value {
        case let .data(.string(s)): return s
        // The values JavaScript receives as strings decode into a String
        // too: a decimal keeps its text so.
        case let .decimal(text), let .uuid(text): return text
        case let .dateTime(v): return v.description
        case let .date(v): return v.description
        case let .time(v): return v.description
        default: throw mismatch(String.self, "a string")
        }
    }

    func decodeDouble() throws -> Double {
        switch value {
        case let .data(.number(text)):
            guard let d = Double(text) else { throw mismatch(Double.self, "a number") }
            return d
        default: throw mismatch(Double.self, "a number")
        }
    }

    func decodeInteger<T: FixedWidthInteger>(_ type: T.Type) throws -> T {
        switch value {
        case let .data(.number(text)):
            if let exact = T(text, radix: 10) { return exact }
            // An integral float (`2.0`) is that integer.
            if let d = Double(text), d.rounded(.towardZero) == d, let exact = T(exactly: d) { return exact }
            throw DecodingError.dataCorrupted(.init(codingPath: codingPath, debugDescription: "the number \(text) does not fit \(T.self)"))
        case let .bigint(v):
            guard let exact = v.exactly(T.self) else {
                throw DecodingError.dataCorrupted(.init(codingPath: codingPath, debugDescription: "the bigint \(v) does not fit \(T.self)"))
            }
            return exact
        default: throw mismatch(T.self, "an integer")
        }
    }

    struct KeyedContainer<Key: CodingKey>: KeyedDecodingContainerProtocol {
        let decoder: ValueDecoderImpl
        let fields: [(String, Value)]
        var codingPath: [CodingKey] { decoder.codingPath }

        var allKeys: [Key] { fields.compactMap { Key(stringValue: $0.0) } }

        func contains(_ key: Key) -> Bool { fields.contains { $0.0 == key.stringValue } }

        private func field(_ key: Key) -> Value? {
            for (name, value) in fields.reversed() where name == key.stringValue { return value }
            return nil
        }

        private func child(_ key: Key) throws -> ValueDecoderImpl {
            guard let value = field(key) else {
                throw DecodingError.keyNotFound(key, .init(codingPath: codingPath, debugDescription: "missing field \(key.stringValue)"))
            }
            return decoder.nested(value, key)
        }

        func decodeNil(forKey key: Key) throws -> Bool { field(key)?.isNullish ?? true }
        func decode(_: Bool.Type, forKey key: Key) throws -> Bool { try child(key).decodeBool() }
        func decode(_: String.Type, forKey key: Key) throws -> String { try child(key).decodeString() }
        func decode(_: Double.Type, forKey key: Key) throws -> Double { try child(key).decodeDouble() }
        func decode(_: Float.Type, forKey key: Key) throws -> Float { Float(try child(key).decodeDouble()) }
        func decode(_ type: Int.Type, forKey key: Key) throws -> Int { try child(key).decodeInteger(type) }
        func decode(_ type: Int8.Type, forKey key: Key) throws -> Int8 { try child(key).decodeInteger(type) }
        func decode(_ type: Int16.Type, forKey key: Key) throws -> Int16 { try child(key).decodeInteger(type) }
        func decode(_ type: Int32.Type, forKey key: Key) throws -> Int32 { try child(key).decodeInteger(type) }
        func decode(_ type: Int64.Type, forKey key: Key) throws -> Int64 { try child(key).decodeInteger(type) }
        func decode(_ type: UInt.Type, forKey key: Key) throws -> UInt { try child(key).decodeInteger(type) }
        func decode(_ type: UInt8.Type, forKey key: Key) throws -> UInt8 { try child(key).decodeInteger(type) }
        func decode(_ type: UInt16.Type, forKey key: Key) throws -> UInt16 { try child(key).decodeInteger(type) }
        func decode(_ type: UInt32.Type, forKey key: Key) throws -> UInt32 { try child(key).decodeInteger(type) }
        func decode(_ type: UInt64.Type, forKey key: Key) throws -> UInt64 { try child(key).decodeInteger(type) }
        func decode<T: Decodable>(_ type: T.Type, forKey key: Key) throws -> T {
            // A missing optional field is nil (Codable asks decodeIfPresent).
            try child(key).decode(type)
        }

        func nestedContainer<NestedKey: CodingKey>(keyedBy type: NestedKey.Type, forKey key: Key) throws -> KeyedDecodingContainer<NestedKey> {
            try child(key).container(keyedBy: type)
        }

        func nestedUnkeyedContainer(forKey key: Key) throws -> UnkeyedDecodingContainer { try child(key).unkeyedContainer() }
        func superDecoder() throws -> Decoder { decoder }
        func superDecoder(forKey key: Key) throws -> Decoder { try child(key) }
    }

    struct UnkeyedContainer: UnkeyedDecodingContainer {
        let decoder: ValueDecoderImpl
        let items: [Value]
        var currentIndex = 0
        var codingPath: [CodingKey] { decoder.codingPath }
        var count: Int? { items.count }
        var isAtEnd: Bool { currentIndex >= items.count }

        init(decoder: ValueDecoderImpl, items: [Value]) {
            self.decoder = decoder
            self.items = items
        }

        private mutating func next() throws -> ValueDecoderImpl {
            guard !isAtEnd else {
                throw DecodingError.valueNotFound(Any.self, .init(codingPath: codingPath, debugDescription: "the array has only \(items.count) items"))
            }
            defer { currentIndex += 1 }
            return decoder.nested(items[currentIndex], IndexKey(currentIndex))
        }

        mutating func decodeNil() throws -> Bool {
            guard !isAtEnd, items[currentIndex].isNullish else { return false }
            currentIndex += 1
            return true
        }

        mutating func decode(_: Bool.Type) throws -> Bool { try next().decodeBool() }
        mutating func decode(_: String.Type) throws -> String { try next().decodeString() }
        mutating func decode(_: Double.Type) throws -> Double { try next().decodeDouble() }
        mutating func decode(_: Float.Type) throws -> Float { Float(try next().decodeDouble()) }
        mutating func decode(_ type: Int.Type) throws -> Int { try next().decodeInteger(type) }
        mutating func decode(_ type: Int8.Type) throws -> Int8 { try next().decodeInteger(type) }
        mutating func decode(_ type: Int16.Type) throws -> Int16 { try next().decodeInteger(type) }
        mutating func decode(_ type: Int32.Type) throws -> Int32 { try next().decodeInteger(type) }
        mutating func decode(_ type: Int64.Type) throws -> Int64 { try next().decodeInteger(type) }
        mutating func decode(_ type: UInt.Type) throws -> UInt { try next().decodeInteger(type) }
        mutating func decode(_ type: UInt8.Type) throws -> UInt8 { try next().decodeInteger(type) }
        mutating func decode(_ type: UInt16.Type) throws -> UInt16 { try next().decodeInteger(type) }
        mutating func decode(_ type: UInt32.Type) throws -> UInt32 { try next().decodeInteger(type) }
        mutating func decode(_ type: UInt64.Type) throws -> UInt64 { try next().decodeInteger(type) }
        mutating func decode<T: Decodable>(_ type: T.Type) throws -> T { try next().decode(type) }

        mutating func nestedContainer<NestedKey: CodingKey>(keyedBy type: NestedKey.Type) throws -> KeyedDecodingContainer<NestedKey> {
            try next().container(keyedBy: type)
        }

        mutating func nestedUnkeyedContainer() throws -> UnkeyedDecodingContainer { try next().unkeyedContainer() }
        mutating func superDecoder() throws -> Decoder { try next() }
    }

    struct SingleContainer: SingleValueDecodingContainer {
        let decoder: ValueDecoderImpl
        var codingPath: [CodingKey] { decoder.codingPath }

        func decodeNil() -> Bool { decoder.value.isNullish }
        func decode(_: Bool.Type) throws -> Bool { try decoder.decodeBool() }
        func decode(_: String.Type) throws -> String { try decoder.decodeString() }
        func decode(_: Double.Type) throws -> Double { try decoder.decodeDouble() }
        func decode(_: Float.Type) throws -> Float { Float(try decoder.decodeDouble()) }
        func decode(_ type: Int.Type) throws -> Int { try decoder.decodeInteger(type) }
        func decode(_ type: Int8.Type) throws -> Int8 { try decoder.decodeInteger(type) }
        func decode(_ type: Int16.Type) throws -> Int16 { try decoder.decodeInteger(type) }
        func decode(_ type: Int32.Type) throws -> Int32 { try decoder.decodeInteger(type) }
        func decode(_ type: Int64.Type) throws -> Int64 { try decoder.decodeInteger(type) }
        func decode(_ type: UInt.Type) throws -> UInt { try decoder.decodeInteger(type) }
        func decode(_ type: UInt8.Type) throws -> UInt8 { try decoder.decodeInteger(type) }
        func decode(_ type: UInt16.Type) throws -> UInt16 { try decoder.decodeInteger(type) }
        func decode(_ type: UInt32.Type) throws -> UInt32 { try decoder.decodeInteger(type) }
        func decode(_ type: UInt64.Type) throws -> UInt64 { try decoder.decodeInteger(type) }
        func decode<T: Decodable>(_ type: T.Type) throws -> T { try decoder.decode(type) }
    }
}
