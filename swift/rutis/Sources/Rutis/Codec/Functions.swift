import Foundation

// What generated code (the macros) and the SDK use to pass values across:
// a service's method table, the arguments a method receives, and closures
// crossing by reference.

/// A type whose instances are services: `@RutisService` generates the
/// conformance, its method table from the type's methods. Its instances
/// cross by reference.
public protocol RutisExportable {
    /// The wire names of the type's methods (each method's base name).
    static var rutisMethodNames: [String] { get }
    /// The methods of this instance by wire name.
    func rutisMethods() -> [String: Rutis.Method]
}

extension Rutis {
    /// One method of a service: called with the call's arguments, it
    /// returns the encoded result.
    public typealias Method = (Rutis.Arguments) async throws -> Rutis.Encoded

    /// The arguments of a call, decoded by the declared types.
    public struct Arguments: @unchecked Sendable {
        package let values: [Value]

        package init(_ values: [Value]) { self.values = values }

        /// How many arguments the caller passed.
        public var count: Int { values.count }

        /// The argument at `index` as a `T`. A missing argument is nil: a
        /// non-optional `T` then fails the call.
        public func decode<T>(_ index: Int, as type: T.Type = T.self) throws -> T {
            let value = index < values.count ? values[index] : .undefined
            do {
                return try ValueCodec.decode(type, from: value)
            } catch {
                if index >= values.count, !(T.self is OptionalProtocol.Type) {
                    throw RutisError(name: "Error", message: "missing argument \(index + 1) (\(T.self))")
                }
                throw RutisError(name: "Error", message: "argument \(index + 1): \(errorMessage(error))")
            }
        }

        /// The argument at `index` as a function of the far end, typed by
        /// the closure type it is assigned to.
        public func function<each A, R>(_ index: Int) throws -> (repeat each A) async throws -> R {
            let value = index < values.count ? values[index] : .undefined
            return try Rutis.function(from: value)
        }

        /// The argument at `index` as an optional function.
        public func optionalFunction<each A, R>(_ index: Int) throws -> ((repeat each A) async throws -> R)? {
            let value = index < values.count ? values[index] : .undefined
            if value.isNullish { return nil }
            return try Rutis.function(from: value)
        }
    }

    /// A function of the far end as a typed closure: its arguments encoded,
    /// its result decoded into `R`.
    static func function<each A, R>(from value: Value) throws -> (repeat each A) async throws -> R {
        switch value {
        case let .remoteFunction(far):
            return { (args: repeat each A) in
                var encoded: [Value] = []
                repeat encoded.append(try ValueCodec.encode(each args))
                let result = try await far.call(encoded, sync: false)
                return try ValueCodec.decode(R.self, from: result)
            }
        case let .function(function):
            // A function of this process, passed back: called directly.
            return { (args: repeat each A) in
                var encoded: [Value] = []
                repeat encoded.append(try ValueCodec.encode(each args))
                return try ValueCodec.decode(R.self, from: try await function.body(encoded))
            }
        default:
            throw RutisError(name: "Error", message: "expected a function, got \(value.described)")
        }
    }

    /// A closure as an argument or result: it crosses by reference, the far
    /// end calling it with arguments decoded by its parameter types.
    public static func export<each A, R>(_ function: @escaping (repeat each A) async throws -> R) -> Encoded {
        Encoded(.function(SessionFunction { args in
            var index = 0
            func next<T>(_: T.Type) throws -> T {
                defer { index += 1 }
                return try Arguments(args).decode(index, as: T.self)
            }
            let result = try await function(repeat try next((each A).self))
            return try ValueCodec.encode(result)
        }))
    }

    /// An optional closure as an argument or result.
    public static func export<each A, R>(_ function: ((repeat each A) async throws -> R)?) -> Encoded {
        guard let function else { return Encoded(.null) }
        return export(function)
    }
}

/// A @RutisService instance as the session exports it.
final class ServiceObject: SessionObject, @unchecked Sendable {
    let service: RutisExportable
    let identity: ObjectIdentifier
    private let lock = Lock()
    private var table: [String: Rutis.Method]?

    init(_ service: RutisExportable) {
        self.service = service
        identity = ObjectIdentifier(type(of: service) is AnyClass ? (service as AnyObject) : (Box() as AnyObject))
    }

    private final class Box {}

    var methods: [String: Rutis.Method] {
        lock.lock()
        defer { lock.unlock() }
        if let table { return table }
        let made = service.rutisMethods()
        table = made
        return made
    }

    func call(method: String, args: [Value]) async throws -> Value {
        guard let body = methods[method] else {
            throw RutisError(name: "Error", message: "\(type(of: service)) has no method \(method)")
        }
        return try await body(Rutis.Arguments(args)).value
    }

    func get(property: String) async throws -> Value {
        try readProperty(of: service, property)
    }
}

/// The stored property `property` of `object`, as a value: what `get`
/// reads (Swift reads stored properties with Mirror).
func readProperty(of object: Any, _ property: String) throws -> Value {
    var mirror: Mirror? = Mirror(reflecting: object)
    while let current = mirror {
        for child in current.children where child.label == property {
            return try ValueCodec.encodeAny(child.value)
        }
        mirror = current.superclassMirror
    }
    return .undefined
}
