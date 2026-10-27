// A row's config against its plugin's JSON Schema: defaults filled, then
// validated (config schema design, docs/design-config-schema-2026-10-27.en.md
// §3–§6). The rules every language's SDK has; the shared case list
// specs/config-schema/cases.json keeps them the same word for word, and its
// README states the details settled while implementing.
//
// Keywords checked: type, properties, required, additionalProperties,
// items, enum, const, minimum, maximum, default, anyOf, oneOf, allOf, $ref
// (to #/$defs/… and #/definitions/…). Others are neither checked nor
// warned about.

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

/// What checking a config gives.
package struct ConfigChecked {
    /// The config with defaults filled; `.null` for no value.
    package var config: JSON
    /// Every problem, sorted by path; none when it passes.
    package var errors: [ConfigIssue]
    /// Extra keys where the schema allows them, sorted.
    package var warnings: [ConfigIssue]
}

package enum ConfigSchema {
    /// At most this many problems are written; the rest as one last line.
    static let shown = 20

    /// Fill `config` (`nil` or `.null`: no config) with the defaults of
    /// `schema`, then validate it.
    package static func check(_ schema: JSON, _ config: JSON?, required: Bool = true) -> ConfigChecked {
        let doc = Document(root: schema)
        let none = config == nil || config == .null
        var value: JSON
        if let config, !none {
            value = config
        } else if let fallback = doc.defaultOf([schema]) {
            value = fallback
        } else if doc.isObjectRoot(schema) {
            value = .object([])
        } else {
            value = .null
        }
        doc.fill([schema], &value)
        var sink = Sink(warn: true)
        var tried: [String: Tried] = [:]
        doc.validate([schema], &value, [], required, &sink, &tried)
        if case .null = value, none, !sink.errors.isEmpty {
            sink.errors = [issue([], "is required")]
        }
        return ConfigChecked(config: value, errors: sorted(sink.errors), warnings: sorted(sink.warnings))
    }

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

    /// How a value is written after `got`: strings, numbers, booleans and
    /// null as JSON (a string past 40 scalar values cut with `…`); `an
    /// object`, `an array of N`.
    package static func valueText(_ value: JSON) -> String {
        switch value {
        case .null: return "null"
        case let .bool(b): return b ? "true" : "false"
        case let .number(text): return numberText(text)
        case let .string(s): return quoted(s, limit: 40)
        case let .array(items): return "an array of \(items.count)"
        case .object: return "an object"
        }
    }

    /// A number as JavaScript's `String(number)` writes it; an integral
    /// value of magnitude at most 2^53 without a decimal point.
    package static func numberText(_ text: String) -> String {
        if JSONNumber.isIntegerLiteral(text), let integer = Int64(text) { return String(integer) }
        guard let double = Double(text), double.isFinite else { return text }
        return doubleText(double)
    }

    /// `f` as JavaScript's `String(number)` writes it.
    package static func doubleText(_ f: Double) -> String {
        if f == 0 { return "0" }
        if f.rounded() == f, abs(f) <= 9_007_199_254_740_992 { return String(Int64(f)) }
        // Shortest round-trip digits and the exponent, from Swift's own
        // shortest text (`1.5`, `1e-07`, `1.5e+20`).
        let shortest = "\(abs(f))"
        let parts = shortest.lowercased().split(separator: "e", maxSplits: 1)
        let mantissa = String(parts[0])
        let exponent = parts.count > 1 ? Int(parts[1].replacingOccurrences(of: "+", with: ""))! : 0
        let dot = mantissa.firstIndex(of: ".").map { mantissa.distance(from: mantissa.startIndex, to: $0) } ?? mantissa.count
        var digits = mantissa.replacingOccurrences(of: ".", with: "")
        // The decimal point's position among `digits`, then leading zeros
        // dropped.
        var n = dot + exponent
        while digits.count > 1, digits.first == "0" {
            digits.removeFirst()
            n -= 1
        }
        while digits.count > 1, digits.last == "0" { digits.removeLast() }
        let k = digits.count
        let body: String
        if k <= n, n <= 21 {
            body = digits + String(repeating: "0", count: n - k)
        } else if n > 0, n <= 21 {
            body = String(digits.prefix(n)) + "." + String(digits.dropFirst(n))
        } else if n > -6, n <= 0 {
            body = "0." + String(repeating: "0", count: -n) + digits
        } else {
            let e = n - 1
            let mark = e >= 0 ? "e+\(e)" : "e-\(-e)"
            body = (k == 1 ? digits : String(digits.prefix(1)) + "." + String(digits.dropFirst(1))) + mark
        }
        return (f < 0 ? "-" : "") + body
    }

    /// The name of `known` closest to `name`, when close enough to be a
    /// misspelling: at most one edit for 3–6 scalar values, two for more,
    /// none for fewer; the smallest distance, then the first in `known`.
    package static func closest(_ name: String, _ known: [String]) -> String? {
        let length = name.unicodeScalars.count
        if length <= 2 { return nil }
        let limit = length <= 6 ? 1 : 2
        var best: (String, Int)?
        for candidate in known where !eq(candidate, name) {
            let d = distance(name, candidate)
            if d <= limit, best == nil || d < best!.1 { best = (candidate, d) }
        }
        return best?.0
    }

    /// Edits (insert, delete, replace, swap two neighbours) between `a` and
    /// `b`, in scalar values.
    package static func distance(_ a: String, _ b: String) -> Int {
        let a = Array(a.unicodeScalars), b = Array(b.unicodeScalars)
        var rows = [[Int]](repeating: [Int](repeating: 0, count: b.count + 1), count: a.count + 1)
        for i in 0 ... a.count { rows[i][0] = i }
        for j in 0 ... b.count { rows[0][j] = j }
        if a.isEmpty || b.isEmpty { return rows[a.count][b.count] }
        for i in 1 ... a.count {
            for j in 1 ... b.count {
                let cost = a[i - 1] == b[j - 1] ? 0 : 1
                var best = min(rows[i - 1][j] + 1, rows[i][j - 1] + 1, rows[i - 1][j - 1] + cost)
                if i > 1, j > 1, a[i - 1] == b[j - 2], a[i - 2] == b[j - 1] { best = min(best, rows[i - 2][j - 2] + 1) }
                rows[i][j] = best
            }
        }
        return rows[a.count][b.count]
    }

    /// Strings by code point (Swift's `<` compares canonically).
    package static func byCodePoint(_ a: String, _ b: String) -> Bool {
        let x = a.unicodeScalars.map(\.value), y = b.unicodeScalars.map(\.value)
        return x.lexicographicallyPrecedes(y)
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

    /// Paths segment by segment: keys by code point, indices as numbers, a
    /// key before an index, a prefix first.
    static func precedes(_ a: [ConfigPathSegment], _ b: [ConfigPathSegment]) -> Bool {
        for (x, y) in zip(a, b) {
            switch (x, y) {
            case let (.index(i), .index(j)): if i != j { return i < j }
            case (.key, .index): return true
            case (.index, .key): return false
            case let (.key(k), .key(l)): if !eq(k, l) { return byCodePoint(k, l) }
            }
        }
        return a.count < b.count
    }

    /// Problems sorted by path (stable), each once.
    static func sorted(_ issues: [ConfigIssue]) -> [ConfigIssue] {
        let ordered = issues.enumerated().sorted { lhs, rhs in
            if precedes(lhs.element.path, rhs.element.path) { return true }
            if precedes(rhs.element.path, lhs.element.path) { return false }
            return lhs.offset < rhs.offset
        }.map(\.element)
        var out: [ConfigIssue] = []
        for problem in ordered where !out.contains(where: { eq($0.message, problem.message) && $0.path.count == problem.path.count
                && zip($0.path, problem.path).allSatisfy(same) })
        {
            out.append(problem)
        }
        return out
    }

    /// Strings scalar by scalar (Swift's `==` takes canonically equal strings
    /// as equal: `"\u{e9}" == "e\u{301}"`).
    static func eq(_ a: String, _ b: String) -> Bool { a.unicodeScalars.elementsEqual(b.unicodeScalars) }

    static func same(_ a: ConfigPathSegment, _ b: ConfigPathSegment) -> Bool {
        switch (a, b) {
        case let (.key(x), .key(y)): return eq(x, y)
        case let (.index(x), .index(y)): return x == y
        default: return false
        }
    }

    /// A key in a JSON Pointer: `~` and `/` escaped.
    static func escaped(_ key: String) -> String {
        key.replacingOccurrences(of: "~", with: "~0").replacingOccurrences(of: "/", with: "~1")
    }

    /// JSON equality: numbers by value, objects regardless of key order.
    static func same(_ a: JSON, _ b: JSON) -> Bool {
        switch (a, b) {
        case let (.string(x), .string(y)): return eq(x, y)
        case let (.number(x), .number(y)):
            if let i = Int64(x), let j = Int64(y), JSONNumber.isIntegerLiteral(x), JSONNumber.isIntegerLiteral(y) { return i == j }
            return Double(x) == Double(y)
        case let (.array(x), .array(y)): return x.count == y.count && zip(x, y).allSatisfy { same($0, $1) }
        case let (.object(x), .object(y)):
            return x.count == y.count && x.allSatisfy { key, value in lookup(y, key).map { same(value, $0) } ?? false }
        default: return a == b
        }
    }

    static func lookup(_ object: [(String, JSON)], _ key: String) -> JSON? {
        object.first { eq($0.0, key) }?.1
    }

    static let typeNames = ["string", "number", "integer", "boolean", "object", "array", "null"]

    static func types(_ type: JSON) -> [String] {
        let names: [String]
        switch type {
        case let .string(name): names = [name]
        case let .array(items): names = items.compactMap { if case let .string(s) = $0 { return s } else { return nil } }
        default: names = []
        }
        return names.filter(typeNames.contains)
    }

    static func isType(_ name: String, _ value: JSON) -> Bool {
        switch (name, value) {
        case (_, .null): return name == "null"
        case ("string", .string), ("boolean", .bool), ("object", .object), ("array", .array): return true
        case let ("number", .number(text)): return Double(text)?.isFinite ?? false
        case let ("integer", .number(text)):
            if JSONNumber.isIntegerLiteral(text) { return true }
            guard let d = Double(text), d.isFinite else { return false }
            return d.rounded() == d
        default: return false
        }
    }

    static func typeText(_ name: String) -> String {
        ["string": "a string", "number": "a number", "integer": "an integer", "boolean": "a boolean",
         "object": "an object", "array": "an array"][name] ?? "null"
    }

    /// `a`, `a or b`, `one of a, b, c`.
    static func joined(_ texts: [String]) -> String {
        switch texts.count {
        case 1: return texts[0]
        case 2: return "\(texts[0]) or \(texts[1])"
        default: return "one of " + texts.joined(separator: ", ")
        }
    }

    /// `<path> is not a config key`, with a close known key suggested.
    static func extra(_ path: [ConfigPathSegment], _ key: String, _ known: [String]) -> ConfigIssue {
        switch closest(key, known) {
        case let like?: return issue(path, "is not a config key (did you mean \(like)?)")
        case nil: return issue(path, "is not a config key")
        }
    }

    struct Sink {
        var errors: [ConfigIssue] = []
        var warnings: [ConfigIssue] = []
        /// Extra keys are warned about (not inside anyOf / oneOf branches).
        var warn: Bool
    }

    /// A branch tried at one value: being tried, failed, or passed with
    /// the value as it passed.
    enum Tried {
        case trying
        case failed
        case passed(JSON)
    }

    /// A schema part: an object schema and where it is in the document (its
    /// JSON Pointer), which tells branches apart.
    struct Part {
        var fields: [(String, JSON)]
        var pointer: String

        subscript(_ key: String) -> JSON? { ConfigSchema.lookup(fields, key) }
        func has(_ key: String) -> Bool { fields.contains { ConfigSchema.eq($0.0, key) } }
    }

    /// A schema and its pointer.
    struct Node {
        var schema: JSON
        var pointer: String
    }

    /// The schema document `$ref`s resolve in.
    struct Document {
        var root: JSON

        /// `#/$defs/…` or `#/definitions/…` in the document.
        func resolve(_ reference: JSON?) -> Node? {
            guard case let .string(text)? = reference, text.hasPrefix("#/$defs/") || text.hasPrefix("#/definitions/") else { return nil }
            var node = root
            for raw in text.dropFirst(2).split(separator: "/", omittingEmptySubsequences: false) {
                let key = raw.replacingOccurrences(of: "~1", with: "/").replacingOccurrences(of: "~0", with: "~")
                switch node {
                // An array's item by its index (no leading zero), as a JSON Pointer.
                case let .array(items):
                    guard let index = Int(key), String(index) == key, index >= 0, index < items.count else { return nil }
                    node = items[index]
                case let .object(fields):
                    guard let next = lookup(fields, key) else { return nil }
                    node = next
                default:
                    return nil
                }
            }
            return Node(schema: node, pointer: String(text.dropFirst()))
        }

        /// `nodes` and what they take in through `$ref` and `allOf`, each
        /// once: a schema, then its `$ref`'s, then its `allOf` members'
        /// (depth-first pre-order).
        func parts(_ nodes: [Node]) -> [Part] {
            var out: [Part] = []
            var seen: Set<String> = []
            func collect(_ node: Node) {
                guard case let .object(fields) = node.schema, seen.insert(node.pointer).inserted else { return }
                let part = Part(fields: fields, pointer: node.pointer)
                out.append(part)
                if let target = resolve(part["$ref"]) { collect(target) }
                if case let .array(members)? = part["allOf"] {
                    for (index, member) in members.enumerated() {
                        collect(Node(schema: member, pointer: "\(node.pointer)/allOf/\(index)"))
                    }
                }
            }
            nodes.forEach(collect)
            return out
        }

        func parts(_ schemas: [JSON]) -> [Part] { parts(schemas.map { Node(schema: $0, pointer: "") }) }

        /// The first `default` of `nodes` (through `$ref` and `allOf`).
        func defaultOf(_ nodes: [Node]) -> JSON? {
            parts(nodes).first { $0.has("default") }?["default"]
        }

        func defaultOf(_ schemas: [JSON]) -> JSON? { defaultOf(schemas.map { Node(schema: $0, pointer: "") }) }

        /// The root's type is `object` or includes it.
        func isObjectRoot(_ schema: JSON) -> Bool {
            guard let type = parts([schema]).first(where: { $0.has("type") })?["type"] else { return false }
            return types(type).contains("object")
        }

        static func properties(_ part: Part) -> [(String, JSON)]? {
            if case let .object(fields)? = part["properties"] { return fields }
            return nil
        }

        /// The schemas a child at `key` is checked against.
        static func property(_ parts: [Part], _ key: String) -> [Node] {
            parts.compactMap { part in
                properties(part).flatMap { lookup($0, key) }.map { Node(schema: $0, pointer: "\(part.pointer)/properties/\(escaped(key))") }
            }
        }

        /// The first `additionalProperties` of `parts`.
        static func additional(_ parts: [Part]) -> Node? {
            guard let part = parts.first(where: { $0.has("additionalProperties") }) else { return nil }
            return Node(schema: part["additionalProperties"]!, pointer: "\(part.pointer)/additionalProperties")
        }

        static func known(_ parts: [Part]) -> [String] {
            var keys: [String] = []
            for part in parts {
                for (key, _) in properties(part) ?? [] where !keys.contains(where: { eq($0, key) }) { keys.append(key) }
            }
            return keys.sorted(by: byCodePoint)
        }

        static func items(_ parts: [Part]) -> [Node] {
            parts.compactMap { part in
                guard case .object? = part["items"] else { return nil }
                return Node(schema: part["items"]!, pointer: "\(part.pointer)/items")
            }
        }

        /// Fill defaults level by level: in an object that is there, each
        /// missing property with a `default`; then into the child objects
        /// and array items. Not into anyOf / oneOf.
        func fill(_ schemas: [JSON], _ value: inout JSON) { fill(schemas.map { Node(schema: $0, pointer: "") }, &value) }

        func fill(_ nodes: [Node], _ value: inout JSON) {
            let parts = parts(nodes)
            switch value {
            case var .object(fields):
                for part in parts {
                    for (key, property) in Document.properties(part) ?? [] where lookup(fields, key) == nil {
                        if let fallback = defaultOf([Node(schema: property, pointer: "\(part.pointer)/properties/\(escaped(key))")]) {
                            fields.append((key, fallback))
                        }
                    }
                }
                let known = Document.known(parts)
                let additional = Document.additional(parts)
                for index in fields.indices {
                    let key = fields[index].0
                    var below: [Node]
                    if known.contains(where: { eq($0, key) }) {
                        below = Document.property(parts, key)
                    } else if let additional, case .object = additional.schema {
                        below = [additional]
                    } else {
                        below = []
                    }
                    if !below.isEmpty { fill(below, &fields[index].1) }
                }
                value = .object(fields)
            case var .array(items):
                let below = Document.items(parts)
                if !below.isEmpty {
                    for index in items.indices { fill(below, &items[index]) }
                }
                value = .array(items)
            default:
                break
            }
        }

        func validate(_ schemas: [JSON], _ value: inout JSON, _ path: [ConfigPathSegment], _ required: Bool,
                      _ sink: inout Sink, _ tried: inout [String: Tried])
        {
            validate(schemas.map { Node(schema: $0, pointer: "") }, &value, path, required, &sink, &tried)
        }

        /// Validate `value` at `path` against every one of `nodes`.
        /// `tried`: the anyOf / oneOf branches tried at this value, by
        /// their pointer.
        func validate(_ nodes: [Node], _ value: inout JSON, _ path: [ConfigPathSegment], _ required: Bool,
                      _ sink: inout Sink, _ tried: inout [String: Tried])
        {
            let parts = parts(nodes)
            // The type first: when it is wrong, nothing else at or below it.
            for part in parts {
                guard let type = part["type"] else { continue }
                let names = types(type)
                if names.isEmpty { continue }
                if !names.contains(where: { isType($0, value) }) {
                    sink.errors.append(issue(path, "should be \(joined(names.map(typeText))), got \(valueText(value))"))
                    return
                }
                // An integral number passing `integer` is an integer.
                if names.contains("integer"), case let .number(text) = value, !JSONNumber.isIntegerLiteral(text),
                   let d = Double(text), abs(d) <= 9_007_199_254_740_992
                {
                    value = .number(String(Int64(d)))
                }
            }
            for part in parts {
                if case let .array(allowed)? = part["enum"], !allowed.contains(where: { same($0, value) }) {
                    sink.errors.append(issue(path, "should be \(joined(allowed.map(valueText))), got \(valueText(value))"))
                }
                if let constant = part["const"], !same(constant, value) {
                    sink.errors.append(issue(path, "should be \(valueText(constant)), got \(valueText(value))"))
                }
                if case let .number(text) = value, let n = Double(text) {
                    if case let .number(bound)? = part["minimum"], let minimum = Double(bound), n < minimum {
                        sink.errors.append(issue(path, "should be at least \(numberText(bound)), got \(valueText(value))"))
                    }
                    if case let .number(bound)? = part["maximum"], let maximum = Double(bound), n > maximum {
                        sink.errors.append(issue(path, "should be at most \(numberText(bound)), got \(valueText(value))"))
                    }
                }
                for keyword in ["anyOf", "oneOf"] {
                    guard case let .array(branches)? = part[keyword] else { continue }
                    var passed: JSON?
                    for (index, branch) in branches.enumerated() {
                        // At one value each branch is tried once: one being
                        // tried (reached again through $ref) does not match,
                        // one tried before gives its result again.
                        let pointer = "\(part.pointer)/\(keyword)/\(index)"
                        var result: JSON?
                        switch tried[pointer] {
                        case .trying?, .failed?: result = nil
                        case let .passed(copy)?: result = copy
                        case nil:
                            var trial = Sink(warn: false)
                            var attempt = value
                            tried[pointer] = .trying
                            validate([Node(schema: branch, pointer: pointer)], &attempt, path, required, &trial, &tried)
                            result = trial.errors.isEmpty ? attempt : nil
                            tried[pointer] = result.map(Tried.passed) ?? .failed
                        }
                        if let result {
                            passed = result
                            break
                        }
                    }
                    if let passed {
                        value = passed
                    } else {
                        sink.errors.append(issue(path, "should be \(joined(branchTexts(branches, part.pointer, keyword))), got \(valueText(value))"))
                    }
                }
            }
            switch value {
            case var .object(fields):
                if required {
                    for part in parts {
                        guard case let .array(keys)? = part["required"] else { continue }
                        for case let .string(key) in keys where lookup(fields, key) == nil {
                            sink.errors.append(issue(path + [.key(key)], "is required"))
                        }
                    }
                }
                let known = Document.known(parts)
                let additional = Document.additional(parts)
                let described = parts.contains { $0.has("properties") }
                for index in fields.indices {
                    let key = fields[index].0
                    let at = path + [.key(key)]
                    var fresh: [String: Tried] = [:]
                    if known.contains(where: { eq($0, key) }) {
                        validate(Document.property(parts, key), &fields[index].1, at, required, &sink, &fresh)
                    } else if let additional {
                        switch additional.schema {
                        case .bool(false): sink.errors.append(extra(at, key, known))
                        case .object: validate([additional], &fields[index].1, at, required, &sink, &fresh)
                        default: break
                        }
                    } else if described, sink.warn {
                        sink.warnings.append(extra(at, key, known))
                    }
                }
                value = .object(fields)
            case var .array(items):
                let below = Document.items(parts)
                if !below.isEmpty {
                    for index in items.indices {
                        var fresh: [String: Tried] = [:]
                        validate(below, &items[index], path + [.index(index)], required, &sink, &fresh)
                    }
                }
                value = .array(items)
            default:
                break
            }
        }

        /// How the branches of a failed anyOf / oneOf are written (§6.5):
        /// by the first of `const`, `enum`, `type` a branch has through
        /// `$ref` and `allOf`, in order, each once.
        func branchTexts(_ branches: [JSON], _ pointer: String, _ keyword: String) -> [String] {
            var texts: [String] = []
            for (index, branch) in branches.enumerated() {
                var found: [String]?
                for part in parts([Node(schema: branch, pointer: "\(pointer)/\(keyword)/\(index)")]) {
                    if let constant = part["const"] {
                        found = [valueText(constant)]
                        break
                    }
                    if case let .array(allowed)? = part["enum"] {
                        found = allowed.map(valueText)
                        break
                    }
                    if let type = part["type"], !types(type).isEmpty {
                        found = types(type).map(typeText)
                        break
                    }
                }
                for text in found ?? ["another accepted value"] where !texts.contains(text) { texts.append(text) }
            }
            return texts
        }
    }
}
