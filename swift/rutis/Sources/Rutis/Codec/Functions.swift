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
    func rutisMethods() -> [String: Rutis._Generated.Method]
}

extension Rutis._Generated {
    /// One method of a service: called with the call's arguments, it
    /// returns the encoded result.
    public typealias Method = (Rutis._Generated.Arguments) async throws -> Rutis._Generated.Encoded

    /// The arguments of a call, decoded by the declared types.
    public struct Arguments: @unchecked Sendable {
        package let values: [Value]
        /// The service (or object) and method called, which a refused
        /// argument names.
        package let callee: Refusal.Callee?

        package init(_ values: [Value], callee: Refusal.Callee? = nil) {
            self.values = values
            self.callee = callee
        }

        /// How many arguments the caller passed.
        public var count: Int { values.count }

        /// The argument at `index` as a `T`. A missing argument is nil: a
        /// non-optional `T` then fails the call.
        public func decode<T>(_ index: Int, as type: T.Type = T.self) throws -> T {
            try decode(index, as: type, object: nil)
        }

        /// `decode(_:as:)` for a parameter that may take an object of
        /// another process: made into a `T` by `object` (a @RutisClient
        /// protocol's proxy, `rutisObjectProxy`), or a `Rutis.Client` for
        /// `Any`; any other type takes none (#687 §四).
        public func decode<T>(_ index: Int, as type: T.Type = T.self, object proxy: ((Rutis.Advanced.Client) -> T)?) throws -> T {
            let value = index < values.count ? values[index] : .undefined
            if case let .remoteObject(far) = value {
                if let proxy { return proxy(Rutis.Advanced.Client.of(far)) }
                if let client = Rutis.Advanced.Client.of(far) as? T { return client }
                let callee = callee ?? Refusal.Callee(method: "")
                throw Refusal.badArgument(callee, index, expected: Refusal.expected(T.self),
                                          actual: "an object (an object is received only as a @RutisClient protocol or Any)")
            }
            do {
                return try ValueCodec.decode(type, from: value)
            } catch {
                let callee = callee ?? Refusal.Callee(method: "")
                let expected = Refusal.expected(T.self)
                var undefined = index >= values.count
                if case .undefined = value { undefined = true }
                if undefined, !(T.self is OptionalProtocol.Type) {
                    throw Refusal.missing(callee, index, expected: expected)
                }
                throw Refusal.badArgument(callee, index, expected: expected, actual: Refusal.actual(value, error))
            }
        }

        /// The argument at `index` as a function of the far end, typed by
        /// the closure type it is assigned to.
        public func function<each A, R>(_ index: Int) throws -> (repeat each A) async throws -> R {
            let value = index < values.count ? values[index] : .undefined
            return try Rutis._Generated.function(from: value)
        }

        /// The argument at `index` as an optional function.
        public func optionalFunction<each A, R>(_ index: Int) throws -> ((repeat each A) async throws -> R)? {
            let value = index < values.count ? values[index] : .undefined
            if value.isNullish { return nil }
            return try Rutis._Generated.function(from: value)
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
    private var table: [String: Rutis._Generated.Method]?

    init(_ service: RutisExportable) {
        self.service = service
        identity = ObjectIdentifier(type(of: service) is AnyClass ? (service as AnyObject) : (Box() as AnyObject))
    }

    private final class Box {}

    var methods: [String: Rutis._Generated.Method] {
        lock.lock()
        defer { lock.unlock() }
        if let table { return table }
        let made = service.rutisMethods()
        table = made
        return made
    }

    func call(method: String, args: [Value]) async throws -> Value {
        let callee = Refusal.Callee(object: String(describing: type(of: service)), method: method)
        guard let body = methods[method] else {
            throw Refusal.noMethod(callee, methods.keys)
        }
        return try await body(Rutis._Generated.Arguments(args, callee: callee)).value
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
    throw RutisError(name: "Error", message: "no property \(property)")
}
