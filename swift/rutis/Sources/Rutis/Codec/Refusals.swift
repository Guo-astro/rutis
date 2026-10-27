import Foundation

/// The errors this runtime makes when it refuses a call or a use (#677,
/// docs/design-diagnostics-2026-10-26.md §4.3): a BindingError with a code
/// for programs and data with the details; the text only uses names the
/// developer wrote. The same in every runtime.
package enum Refusal {
    /// What a refused call names: the service (or the object's type) and
    /// the method a caller called.
    package struct Callee: Sendable {
        package var service: String?
        package var object: String?
        package var method: String

        package init(service: String? = nil, object: String? = nil, method: String) {
            self.service = service
            self.object = object
            self.method = method
        }

        /// `weather.tomorrow`, `Cache.get`; `the function` for a function.
        var label: String {
            guard let owner = service ?? object else { return "the function" }
            return "\(owner).\(method)"
        }

        func data(_ fields: [String: Any?]) -> [String: Any?] {
            var out = fields
            if let service {
                out["service"] = service
            } else if object != nil {
                out["object"] = object
            } else {
                return out
            }
            out["method"] = method
            return out
        }
    }

    static func binding(_ code: String, _ message: String, _ data: [String: Any?]) -> RutisError {
        RutisError(name: RutisError.Name.bindingError, message: message, code: code, data: data)
    }

    /// `<service> has no method <method> (methods: a, b)`, code no-method;
    /// for an object `<Type> has no method …`.
    package static func noMethod(_ callee: Callee, _ methods: some Sequence<String>) -> RutisError {
        let sorted = methods.sorted()
        let listed = sorted.isEmpty ? "no methods" : "methods: " + sorted.joined(separator: ", ")
        let owner = callee.service ?? callee.object ?? "the object"
        return binding("no-method", "\(owner) has no method \(callee.method) (\(listed))",
                       callee.data(["methods": sorted]))
    }

    /// `<service>.<method>: argument <i> is missing` (i from 1), code
    /// bad-argument.
    static func missing(_ callee: Callee, _ index: Int, expected: String) -> RutisError {
        binding("bad-argument", "\(callee.label): argument \(index + 1) is missing",
                callee.data(["argument": index + 1, "expected": expected]))
    }

    /// `<service>.<method>: argument <i> should be <expected>, got
    /// <actual>` (i from 1), code bad-argument.
    static func badArgument(_ callee: Callee, _ index: Int, expected: String, actual: String) -> RutisError {
        binding("bad-argument", "\(callee.label): argument \(index + 1) should be \(expected), got \(actual)",
                callee.data(["argument": index + 1, "expected": expected]))
    }

    /// What a parameter of type `type` takes, in words every language
    /// shares: `a string`, `an integer`, `a list`; another type by its
    /// name (`a Forecast`).
    static func expected(_ type: Any.Type) -> String {
        var name = String(describing: type)
        while let inner = unwrapped(name, "Optional<") { name = inner }
        if name.hasSuffix("?") { name.removeLast() }
        switch name {
        case "Int", "Int8", "Int16", "Int32", "Int64", "UInt", "UInt8", "UInt16", "UInt32", "UInt64": return "an integer"
        case "Double", "Float", "CGFloat", "Float16", "Float80": return "a number"
        case "String", "Substring", "Character": return "a string"
        case "Bool": return "a boolean"
        case "Date", "Instant": return "an instant"
        case "Data": return "bytes"
        case "Any", "Value": return "a value"
        case "BigInt": return "a bigint"
        default: break
        }
        if name.hasPrefix("Array<") || name.hasPrefix("[") && !name.contains(":") { return "a list" }
        if name.hasPrefix("Set<") { return "a set" }
        if name.hasPrefix("Dictionary<String,") || name.hasPrefix("[String:") { return "an object" }
        if name.hasPrefix("Dictionary<") || name.hasPrefix("[") { return "a map" }
        if let generic = name.firstIndex(of: "<") { name = String(name[..<generic]) }
        if let dot = name.lastIndex(of: ".") { name = String(name[name.index(after: dot)...]) }
        return article(name)
    }

    private static func unwrapped(_ name: String, _ prefix: String) -> String? {
        guard name.hasPrefix(prefix), name.hasSuffix(">") else { return nil }
        return String(name.dropFirst(prefix.count).dropLast())
    }

    static func article(_ word: String) -> String {
        guard let first = word.first else { return "a value" }
        return ("AEIOUaeiou".contains(first) ? "an " : "a ") + word
    }

    /// The argument as a refusal shows it: a short value (`"x"`, `1.5`,
    /// `true`), or what it is (`a list`, `an object`, `a function`); for a
    /// field of the wrong type, `an object whose city is a number`.
    static func actual(_ value: Value, _ error: Error) -> String {
        if let error = error as? DecodingError {
            switch error {
            case let .typeMismatch(_, context), let .valueNotFound(_, context):
                if let path = path(context.codingPath), let inner = value.at(context.codingPath) {
                    return "an object whose \(path) is \(kind(inner))"
                }
            case let .keyNotFound(key, context):
                let keys = context.codingPath + [key]
                if let path = path(keys) { return "an object without \(path)" }
            default: break
            }
        }
        return shown(value)
    }

    private static func path(_ keys: [CodingKey]) -> String? {
        if keys.isEmpty { return nil }
        return keys.map { $0.intValue.map { "[\($0)]" } ?? $0.stringValue }.joined(separator: ".")
    }

    static func shown(_ value: Value) -> String {
        switch value {
        case .undefined: return "nothing"
        case let .data(json):
            switch json {
            case .array: return "a list"
            case .object: return "an object"
            default: return abbreviated(JSONWriter.string(json))
            }
        case .list: return "a list"
        case .record: return "an object"
        case .function, .remoteFunction: return "a function"
        case .object: return "an object"
        case .remoteFuture: return "an asynchronous result"
        default: return value.described
        }
    }

    /// What a value is, for a field: `a number`, `a string`, `a list`.
    static func kind(_ value: Value) -> String {
        switch value {
        case let .data(json):
            switch json {
            case .null: return "null"
            case .bool: return "a boolean"
            case .number: return "a number"
            case .string: return "a string"
            case .array: return "a list"
            case .object: return "an object"
            }
        default: return shown(value)
        }
    }

    private static func abbreviated(_ text: String) -> String {
        text.count > 60 ? String(text.prefix(57)) + "..." : text
    }
}

extension Value {
    /// The value at `path` inside it (fields and list items), if there.
    func at(_ path: [CodingKey]) -> Value? {
        var current = self
        for key in path {
            if let index = key.intValue {
                guard let items = current.items, index >= 0, index < items.count else { return nil }
                current = items[index]
            } else {
                guard let field = current[key.stringValue] else { return nil }
                current = field
            }
        }
        return current
    }
}
