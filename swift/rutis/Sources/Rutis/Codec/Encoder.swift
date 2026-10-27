import Foundation

/// What an encoder builds: a value, or an array or object still being
/// filled by a container.
final class EncodedNode {
    enum Content {
        case empty
        case value(Value)
        case array([EncodedNode])
        case object([(String, EncodedNode)])
    }

    var content: Content = .empty

    func value() throws -> Value {
        switch content {
        case .empty: return .undefined
        case let .value(v): return v
        case let .array(items): return .list(of: try items.map { try $0.value() })
        case let .object(fields): return .record(of: try fields.map { ($0.0, try $0.1.value()) })
        }
    }
}

/// The SDK's Encoder: Codable values to session values, each value checked
/// (design §4.5).
final class ValueEncoderImpl: Encoder {
    let node: EncodedNode
    let depth: Int
    var codingPath: [CodingKey]
    var userInfo: [CodingUserInfoKey: Any] { [:] }

    init(depth: Int, node: EncodedNode = EncodedNode(), codingPath: [CodingKey] = []) {
        self.depth = depth
        self.node = node
        self.codingPath = codingPath
    }

    func result() throws -> Value { try node.value() }

    func container<Key: CodingKey>(keyedBy _: Key.Type) -> KeyedEncodingContainer<Key> {
        if case .object = node.content {} else { node.content = .object([]) }
        return KeyedEncodingContainer(KeyedContainer<Key>(encoder: self))
    }

    func unkeyedContainer() -> UnkeyedEncodingContainer {
        if case .array = node.content {} else { node.content = .array([]) }
        return UnkeyedContainer(encoder: self)
    }

    func singleValueContainer() -> SingleValueEncodingContainer { SingleContainer(encoder: self) }

    /// Encodes `value` into `node`, one level deeper.
    func encode(_ value: Any, into node: EncodedNode, key: CodingKey) throws {
        if depth + 1 > maxDepth + 1 {
            throw EncodingError.invalidValue(value, .init(codingPath: codingPath + [key], debugDescription:
                "value nested deeper than \(maxDepth) levels, the most a value crossing processes may nest (a cyclic reference?)"))
        }
        node.content = .value(try ValueCodec.encodeAny(value, depth: depth + 1))
    }

    struct KeyedContainer<Key: CodingKey>: KeyedEncodingContainerProtocol {
        let encoder: ValueEncoderImpl
        var codingPath: [CodingKey] { encoder.codingPath }

        private func add(_ key: Key) -> EncodedNode {
            let child = EncodedNode()
            guard case var .object(fields) = encoder.node.content else { return child }
            if let index = fields.firstIndex(where: { $0.0 == key.stringValue }) {
                fields[index] = (key.stringValue, child)
            } else {
                fields.append((key.stringValue, child))
            }
            encoder.node.content = .object(fields)
            return child
        }

        mutating func encodeNil(forKey key: Key) throws { add(key).content = .value(.null) }
        mutating func encode<T: Encodable>(_ value: T, forKey key: Key) throws {
            try encoder.encode(value, into: add(key), key: key)
        }

        mutating func nestedContainer<NestedKey: CodingKey>(keyedBy _: NestedKey.Type, forKey key: Key) -> KeyedEncodingContainer<NestedKey> {
            let child = add(key)
            child.content = .object([])
            let nested = ValueEncoderImpl(depth: encoder.depth + 1, node: child, codingPath: codingPath + [key])
            return KeyedEncodingContainer(ValueEncoderImpl.KeyedContainer<NestedKey>(encoder: nested))
        }

        mutating func nestedUnkeyedContainer(forKey key: Key) -> UnkeyedEncodingContainer {
            let child = add(key)
            child.content = .array([])
            return UnkeyedContainer(encoder: ValueEncoderImpl(depth: encoder.depth + 1, node: child, codingPath: codingPath + [key]))
        }

        mutating func superEncoder() -> Encoder { superEncoder(forKey: Key(stringValue: "super")!) }
        mutating func superEncoder(forKey key: Key) -> Encoder {
            ValueEncoderImpl(depth: encoder.depth + 1, node: add(key), codingPath: codingPath + [key])
        }
    }

    struct UnkeyedContainer: UnkeyedEncodingContainer {
        let encoder: ValueEncoderImpl
        var codingPath: [CodingKey] { encoder.codingPath }
        var count: Int {
            if case let .array(items) = encoder.node.content { return items.count }
            return 0
        }

        private func add() -> EncodedNode {
            let child = EncodedNode()
            if case var .array(items) = encoder.node.content {
                items.append(child)
                encoder.node.content = .array(items)
            }
            return child
        }

        mutating func encodeNil() throws { add().content = .value(.null) }
        mutating func encode<T: Encodable>(_ value: T) throws {
            let index = count
            try encoder.encode(value, into: add(), key: IndexKey(index))
        }

        mutating func nestedContainer<NestedKey: CodingKey>(keyedBy _: NestedKey.Type) -> KeyedEncodingContainer<NestedKey> {
            let key = IndexKey(count)
            let child = add()
            child.content = .object([])
            let nested = ValueEncoderImpl(depth: encoder.depth + 1, node: child, codingPath: codingPath + [key])
            return KeyedEncodingContainer(ValueEncoderImpl.KeyedContainer<NestedKey>(encoder: nested))
        }

        mutating func nestedUnkeyedContainer() -> UnkeyedEncodingContainer {
            let key = IndexKey(count)
            let child = add()
            child.content = .array([])
            return UnkeyedContainer(encoder: ValueEncoderImpl(depth: encoder.depth + 1, node: child, codingPath: codingPath + [key]))
        }

        mutating func superEncoder() -> Encoder {
            let key = IndexKey(count)
            return ValueEncoderImpl(depth: encoder.depth + 1, node: add(), codingPath: codingPath + [key])
        }
    }

    struct SingleContainer: SingleValueEncodingContainer {
        let encoder: ValueEncoderImpl
        var codingPath: [CodingKey] { encoder.codingPath }

        mutating func encodeNil() throws { encoder.node.content = .value(.null) }
        mutating func encode<T: Encodable>(_ value: T) throws {
            // The same level: a single value is not a container.
            encoder.node.content = .value(try ValueCodec.encodeAny(value, depth: encoder.depth))
        }
    }
}
