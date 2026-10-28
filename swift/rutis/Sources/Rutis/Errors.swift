import Foundation

/// An error that crossed a session, or one of rutis's own: matched by its
/// `name`, never by its type, wherever it was thrown.
///
/// What a plugin throws crosses with its name (the error type's simple
/// name), its message (`LocalizedError.errorDescription`, else
/// `String(describing:)`), and, when the error has stored properties named
/// `code` (a String) and `data`, those too. Received, it is a `RutisError`;
/// thrown on as it is, its whole error graph crosses again.
public struct RutisError: Error, CustomStringConvertible, LocalizedError, @unchecked Sendable {
    /// The names of rutis's own errors.
    public enum Name {
        public static let aggregateError = "AggregateError"
        public static let asyncListener = "AsyncListener"
        public static let waterfallNext = "WaterfallNext"
        public static let reservedEvent = "ReservedEvent"
        public static let syncWaitCycle = "SyncWaitCycle"
        public static let bindingError = "BindingError"
    }

    public var name: String
    public var message: String
    /// The code the error carries (a Rust service's error variant, a
    /// JavaScript error's `code`), if any.
    public var code: String?
    /// The data the error carries, decoded as a value of type `Any`.
    public var data: Any?
    /// An AggregateError's errors, in order (a `parallel`'s failures in
    /// registration order).
    public var errors: [RutisError]
    private var causeBox: [RutisError]
    /// The error graph it arrived with: thrown on, it crosses again whole.
    package var graph: JSON?

    public init(name: String, message: String, code: String? = nil, data: Any? = nil, errors: [RutisError] = [],
                cause: RutisError? = nil)
    {
        self.name = name
        self.message = message
        self.code = code
        self.data = data
        self.errors = errors
        causeBox = cause.map { [$0] } ?? []
    }

    /// The error that caused it, if the far end said.
    public var cause: RutisError? {
        get { causeBox.first }
        set { causeBox = newValue.map { [$0] } ?? [] }
    }

    public var description: String { "\(name): \(message)" }
    public var errorDescription: String? { description }

    /// A transport failure, a withdrawn listener, a refused frame.
    package static func binding(_ message: String) -> RutisError {
        RutisError(name: Name.bindingError, message: message)
    }

    /// What a call fails with once the session ended with `error`: when
    /// the host went, `the host disconnected` with code peer-gone and
    /// `{reason}` (#677).
    package static func ended(_ error: Error) -> RutisError {
        if let ended = error as? SessionEnded, let gone = ended.gone {
            return RutisError(name: Name.bindingError, message: ended.reason, code: "peer-gone", data: ["reason": gone])
        }
        return binding(String(describing: error))
    }

    /// A call with no session to the host.
    package static func hostGone() -> RutisError { ended(SessionEnded.hostGone()) }
}

/// The name an error crosses with: a RutisError's own, else the simple name
/// of its type.
package func errorName(_ error: Error) -> String {
    if let error = error as? RutisError { return error.name }
    if error is ValueError || error is DecodingError || error is EncodingError { return "Error" }
    if let named = error as? CustomErrorName { return named.rutisErrorName }
    var name = String(describing: type(of: error))
    if let generic = name.firstIndex(of: "<") { name = String(name[..<generic]) }
    return name
}

/// The message an error crosses with.
package func errorMessage(_ error: Error) -> String {
    if let error = error as? RutisError { return error.message }
    if let error = error as? ValueError { return error.description }
    if let error = error as? DecodingError { return describe(error) }
    if let localized = error as? LocalizedError, let text = localized.errorDescription { return text }
    return String(describing: error)
}

/// An error naming itself (the SDK's own internal errors).
package protocol CustomErrorName {
    var rutisErrorName: String { get }
}

private func describe(_ error: DecodingError) -> String {
    func path(_ context: DecodingError.Context) -> String {
        let keys = context.codingPath.map { $0.intValue.map { "[\($0)]" } ?? $0.stringValue }
        return keys.isEmpty ? "" : " at " + keys.joined(separator: ".")
    }
    switch error {
    case let .typeMismatch(_, context), let .valueNotFound(_, context), let .dataCorrupted(context):
        return context.debugDescription + path(context)
    case let .keyNotFound(key, context):
        return "missing field \(key.stringValue)" + path(context)
    @unknown default:
        return String(describing: error)
    }
}

/// The `code` and `data` of an error that has them: its stored properties
/// of those names (a `code` that is a String), read with Mirror.
func codeAndData(_ error: Error) -> (String?, Any?, Bool) {
    if let error = error as? RutisError { return (error.code, error.data, error.data != nil) }
    var code: String?
    var data: Any?
    var hasData = false
    var mirror: Mirror? = Mirror(reflecting: error)
    while let current = mirror {
        for child in current.children {
            if child.label == "code", code == nil, let text = child.value as? String { code = text }
            if child.label == "data", !hasData {
                data = child.value
                hasData = true
            }
        }
        mirror = current.superclassMirror
    }
    if hasData {
        if isNilOptional(data) {
            hasData = false
            data = nil
        } else {
            data = unwrapOptional(data)
        }
    }
    return (code, data, hasData)
}

/// The value inside an Optional, or the value itself; nil for `nil`.
func unwrapOptional(_ value: Any?) -> Any? {
    guard let value else { return nil }
    let mirror = Mirror(reflecting: value)
    guard mirror.displayStyle == .optional else { return value }
    guard let child = mirror.children.first else { return nil }
    return unwrapOptional(child.value)
}

func isNilOptional(_ value: Any?) -> Bool {
    guard let value else { return true }
    let mirror = Mirror(reflecting: value)
    return mirror.displayStyle == .optional && mirror.children.isEmpty
}

// ── Where a method threw ─────────────────────────────────────────────────

/// Where the method a call ran threw (#677 §4.4): Swift errors carry no
/// stack, so the generated method table records the declaration of the
/// method that threw, and the error crosses with that one frame. The error
/// itself is never wrapped: code catching it in process sees it as thrown.
package final class ThrowSite: @unchecked Sendable {
    private let lock = NSLock()
    private var recorded: String?

    package init() {}

    /// The frame line, `    at Type.method (file:line)`, if a method threw.
    package var frame: String? { lock.withLock { recorded } }

    func record(_ frame: String) { lock.withLock { recorded = frame } }

    /// The site of the call the running Task serves.
    @TaskLocal package static var current: ThrowSite?
}

extension Rutis._Generated {
    /// For generated code (`@RutisService`): runs `body`, a call of the
    /// method `method` declared at `file`:`line`; what it throws is thrown
    /// on unchanged, and the call being served records where.
    public static func located<T>(_ method: String, _ file: String, _ line: Int,
                                  _ body: () async throws -> T) async throws -> T
    {
        do {
            return try await body()
        } catch {
            // The outermost method records last: the one the call ran.
            ThrowSite.current?.record("    at \(method) (\(file):\(line))")
            throw error
        }
    }
}

// ── Error graphs ─────────────────────────────────────────────────────────

/// The wire form of an error: its name, message and error graph (an
/// AggregateError's children in order). A RutisError thrown on crosses with
/// the far end's graph spliced in (nodes appended, references shifted), as
/// Node does. `frame`, where the method that threw was declared, follows
/// "name: message" in the root's `stack`; for a received error thrown on,
/// it is appended to the root node's `rethrown` instead (at most 8 items).
package func encodeError(_ error: Error, frame: String? = nil, encodeData: (Any) throws -> Value) -> JSON {
    if let remote = error as? RutisError, remote.name == RutisError.Name.syncWaitCycle, remote.graph == nil {
        return .object([("name", .string(remote.name)), ("message", .string(remote.message))])
    }
    var nodes: [JSON] = []

    func shift(_ value: JSON, _ offset: Int) -> JSON {
        switch value {
        case let .object(fields):
            if JSON.lookup(fields, "type") == .string("reference"), let id = JSON.lookup(fields, "value")?.int64Value {
                return .object([("type", .string("reference")), ("value", .int(id + Int64(offset)))])
            }
            return .object(fields.map { ($0.0, shift($0.1, offset)) })
        case let .array(items): return .array(items.map { shift($0, offset) })
        default: return value
        }
    }

    func visit(_ error: Error, _ depth: Int) -> JSON {
        if let remote = error as? RutisError, let graph = remote.graph, let root = graph["root"],
           let received = graph["nodes"]?.arrayValue
        {
            let offset = nodes.count
            nodes.append(contentsOf: received.map { shift($0, offset) })
            let shifted = shift(root, offset)
            // Thrown on by a method: its declaration joins the root's `rethrown`.
            if depth == 0, let frame, let id = shifted["value"]?.int64Value, id >= 0, Int(id) < nodes.count,
               case let .object(fields) = nodes[Int(id)], JSON.lookup(fields, "type") == .string("error")
            {
                var items = JSON.lookup(fields, "rethrown")?.arrayValue ?? []
                if items.count < 8 {
                    items.append(.string(frame))
                    var updated = fields.filter { $0.0 != "rethrown" }
                    updated.append(("rethrown", .array(items)))
                    nodes[Int(id)] = .object(updated)
                }
            }
            return shifted
        }
        let name = errorName(error)
        let message = errorMessage(error)
        let id = nodes.count
        nodes.append(.null)
        var node: [(String, JSON)] = [
            ("type", .string("error")), ("name", .string(name)), ("message", .string(message)),
            ("stack", .string(depth == 0 && frame != nil ? "\(name): \(message)\n\(frame!)" : "\(name): \(message)")),
        ]
        let (code, data, hasData) = codeAndData(error)
        if let code { node.append(("code", .string(code))) }
        if hasData, let data {
            let mark = nodes.count
            if let value = try? encodeData(data), let encoded = graphData(value, &nodes, 0) {
                node.append(("data", encoded))
            } else {
                nodes.removeSubrange(mark...)
            }
        }
        if let remote = error as? RutisError, depth < 64 {
            if remote.name == RutisError.Name.aggregateError || !remote.errors.isEmpty {
                node.append(("errors", .array(remote.errors.map { visit($0, depth + 1) })))
            }
            if let cause = remote.cause { node.append(("cause", visit(cause, depth + 1))) }
        }
        nodes[id] = .object(node)
        return .object([("type", .string("reference")), ("value", .int(id))])
    }

    let root = visit(error, 0)
    return .object([
        ("name", .string(errorName(error))),
        ("message", .string(errorMessage(error))),
        ("graph", .object([("root", root), ("nodes", .array(nodes))])),
    ])
}

/// `value` (an error's data) as an error graph value: data as it is,
/// integers beyond ±(2^53−1) as bigints, arrays and objects as nodes.
private func graphData(_ value: Value, _ nodes: inout [JSON], _ depth: Int) -> JSON? {
    if depth > 64 { return nil }
    func tagged(_ tag: String, _ text: String) -> JSON { .object([("type", .string(tag)), ("value", .string(text))]) }
    func node(_ make: (inout [JSON]) -> JSON?) -> JSON? {
        let id = nodes.count
        nodes.append(.null)
        guard let made = make(&nodes) else { return nil }
        nodes[id] = made
        return .object([("type", .string("reference")), ("value", .int(id))])
    }
    switch value {
    case .undefined: return .object([("type", .string("undefined"))])
    case let .data(json):
        switch json {
        case let .number(text):
            if JSONNumber.isIntegerLiteral(text), let big = Rutis.Advanced.BigInt(text), !big.isSafe { return tagged("bigint", text) }
            return .object([("type", .string("data")), ("value", json)])
        case let .array(items):
            return graphData(.list(items.map(Value.data)), &nodes, depth)
        case let .object(fields):
            return graphData(.record(fields.map { ($0.0, Value.data($0.1)) }), &nodes, depth)
        default:
            return .object([("type", .string("data")), ("value", json)])
        }
    case let .list(items):
        return node { nodes in
            var values: [JSON] = []
            for item in items {
                guard let encoded = graphData(item, &nodes, depth + 1) else { return nil }
                values.append(encoded)
            }
            return .object([("type", .string("array")), ("values", .array(values))])
        }
    case let .record(fields):
        return node { nodes in
            var values: [JSON] = []
            for (key, item) in fields {
                guard let encoded = graphData(item, &nodes, depth + 1) else { return nil }
                values.append(.array([.string(key), encoded]))
            }
            return .object([("type", .string("object")), ("values", .array(values))])
        }
    case let .map(entries):
        var pairs: [JSON] = []
        for (key, item) in entries {
            guard let k = graphData(key, &nodes, depth + 1), let v = graphData(item, &nodes, depth + 1) else { return nil }
            pairs.append(.array([k, v]))
        }
        return .object([("type", .string("map")), ("value", .array(pairs))])
    case let .set(elements):
        var items: [JSON] = []
        for element in elements {
            guard let e = graphData(element, &nodes, depth + 1) else { return nil }
            items.append(e)
        }
        return .object([("type", .string("set")), ("value", .array(items))])
    case let .instant(v): return tagged("instant", v.description)
    case let .dateTime(v): return tagged("datetime", v.description)
    case let .date(v): return tagged("date", v.description)
    case let .time(v): return tagged("time", v.description)
    case let .bytes(v): return tagged("bytes", v.base64EncodedString())
    case let .bigint(v): return tagged("bigint", v.description)
    case let .decimal(v): return tagged("decimal", v)
    case let .uuid(v): return tagged("uuid", v)
    default: return nil
    }
}

/// The error the far end threw, as the `error` field of its `throw` frame.
package func decodeError(_ json: JSON, decodeData: (Value) -> Any?) -> RutisError {
    guard case .object = json else { return RutisError(name: "Error", message: "malformed error") }
    func text(_ value: JSON?) -> String? {
        switch value {
        case nil, .null?: return nil
        case let .string(s)?: return s
        case let other?: return JSONWriter.string(other)
        }
    }
    var graph = json["graph"]
    if graph?.isNull == true { graph = nil }
    var error = RutisError(name: text(json["name"]) ?? "Error", message: text(json["message"]) ?? "")
    error.graph = graph
    if let graph { fill(&error, graph: graph, root: graph["root"], decodeData: decodeData, depth: 0) }
    return error
}

/// Reads the code, data, children and cause of the error node `root` of
/// `graph` into `error`.
private func fill(_ error: inout RutisError, graph: JSON, root: JSON?, decodeData: (Value) -> Any?, depth: Int) {
    guard depth <= 64, let nodes = graph["nodes"]?.arrayValue else { return }
    func node(_ reference: JSON?) -> (Int, JSON)? {
        guard let reference, reference["type"] == .string("reference"), let id = reference["value"]?.int64Value,
              id >= 0, id < nodes.count else { return nil }
        return (Int(id), nodes[Int(id)])
    }
    guard let (_, rootNode) = node(root) else { return }
    if let code = rootNode["code"]?.stringValue { error.code = code }
    if let data = rootNode["data"], let value = graphValue(data, nodes, 0) { error.data = decodeData(value) }
    func child(_ reference: JSON) -> RutisError {
        guard let (_, childNode) = node(reference) else { return RutisError(name: "Error", message: "malformed error") }
        let name = childNode["name"]?.stringValue ?? "Error"
        var failure = RutisError(name: name.isEmpty ? "Error" : name, message: childNode["message"]?.stringValue ?? "")
        let sub = JSON.object([("root", reference), ("nodes", .array(nodes))])
        failure.graph = sub
        fill(&failure, graph: sub, root: reference, decodeData: decodeData, depth: depth + 1)
        return failure
    }
    if let children = rootNode["errors"]?.arrayValue { error.errors = children.map(child) }
    if let cause = rootNode["cause"], node(cause)?.1["type"] == .string("error") { error.cause = child(cause) }
}

/// A value of an error graph.
private func graphValue(_ value: JSON, _ nodes: [JSON], _ depth: Int) -> Value? {
    guard depth <= 64, let type = value["type"]?.stringValue else { return nil }
    switch type {
    case "undefined": return .undefined
    case "data": return .data(value["value"] ?? .null)
    case "number":
        // NaN and the infinities: not data; as text.
        return value["value"].map { .data($0) }
    case "map", "set":
        guard let items = value["value"]?.arrayValue else { return nil }
        if type == "set" {
            var out: [Value] = []
            for item in items {
                guard let decoded = graphValue(item, nodes, depth + 1) else { return nil }
                out.append(decoded)
            }
            return .set(out)
        }
        var entries: [(Value, Value)] = []
        for item in items {
            guard let pair = item.arrayValue, pair.count == 2, let k = graphValue(pair[0], nodes, depth + 1),
                  let v = graphValue(pair[1], nodes, depth + 1) else { return nil }
            entries.append((k, v))
        }
        return .map(entries)
    case "reference":
        guard let id = value["value"]?.int64Value, id >= 0, id < nodes.count else { return nil }
        let node = nodes[Int(id)]
        switch node["type"]?.stringValue {
        case "array":
            guard let items = node["values"]?.arrayValue else { return nil }
            var out: [Value] = []
            for item in items {
                guard let decoded = graphValue(item, nodes, depth + 1) else { return nil }
                out.append(decoded)
            }
            return .list(out)
        case "object":
            guard let entries = node["values"]?.arrayValue else { return nil }
            var out: [(String, Value)] = []
            for entry in entries {
                guard let pair = entry.arrayValue, pair.count == 2, let key = pair[0].stringValue,
                      let decoded = graphValue(pair[1], nodes, depth + 1) else { return nil }
                out.append((key, decoded))
            }
            return .record(out)
        default: return nil
        }
    default:
        guard let text = value["value"]?.stringValue else { return nil }
        return try? parseTagged(type, text)
    }
}
