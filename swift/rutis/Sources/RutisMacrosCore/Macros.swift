// The macros of the Swift SDK (design §4.2–§4.4): Swift has no method
// reflection, so a service's method table, a client protocol's proxy and a
// configuration's schema are generated at compile time. They run in the
// compiler only, never in the binary.
import Foundation
import SwiftDiagnostics
import SwiftSyntax
import SwiftSyntaxBuilder
import SwiftSyntaxMacros

struct MacroError: Error, DiagnosticMessage {
    let message: String
    var diagnosticID: MessageID { MessageID(domain: "RutisMacros", id: "error") }
    var severity: DiagnosticSeverity { .error }
    init(_ message: String) { self.message = message }
}

/// The access modifier generated members need: as visible as the type.
func access(of modifiers: DeclModifierListSyntax) -> String {
    for modifier in modifiers {
        switch modifier.name.tokenKind {
        case .keyword(.public), .keyword(.open): return "public "
        case .keyword(.package): return "package "
        default: continue
        }
    }
    return ""
}

func has(_ modifiers: DeclModifierListSyntax, _ keywords: Keyword...) -> Bool {
    modifiers.contains { modifier in keywords.contains { modifier.name.tokenKind == .keyword($0) } }
}

/// A parameter's type without attributes (`@escaping`, `@Sendable`).
func bareType(_ type: TypeSyntax) -> TypeSyntax {
    if let attributed = type.as(AttributedTypeSyntax.self) { return bareType(attributed.baseType) }
    return type
}

/// Whether a type is a function type, and whether optional.
func functionType(_ type: TypeSyntax) -> (FunctionTypeSyntax, optional: Bool)? {
    let bare = bareType(type)
    if let function = bare.as(FunctionTypeSyntax.self) { return (function, false) }
    var wrapped: TypeSyntax?
    if let optional = bare.as(OptionalTypeSyntax.self) { wrapped = optional.wrappedType }
    if let optional = bare.as(ImplicitlyUnwrappedOptionalTypeSyntax.self) { wrapped = optional.wrappedType }
    if let wrapped {
        var inner = bareType(wrapped)
        if let tuple = inner.as(TupleTypeSyntax.self), tuple.elements.count == 1, let only = tuple.elements.first {
            inner = bareType(only.type)
        }
        if let function = inner.as(FunctionTypeSyntax.self) { return (function, true) }
    }
    return nil
}

func isAsyncThrows(_ function: FunctionTypeSyntax) -> Bool {
    function.effectSpecifiers?.asyncSpecifier != nil && function.effectSpecifiers?.throwsClause != nil
}

func trimmed(_ syntax: some SyntaxProtocol) -> String { syntax.trimmedDescription }

// ── @RutisService ───────────────────────────────────────────────────────

/// `@RutisService`: the type's method table (wire name → decode the
/// arguments, call, encode the result) and the conformance to
/// RutisExportable. Its methods: the non-private, non-static instance
/// methods of the type's declaration; the wire name is the base name.
public struct ServiceMacro: MemberMacro, ExtensionMacro {
    struct Method {
        let name: String
        let body: String
    }

    static func methods(of declaration: some DeclGroupSyntax, in context: some MacroExpansionContext) -> [Method]? {
        let isActor = declaration.is(ActorDeclSyntax.self)
        let typeName = declaration.as(ClassDeclSyntax.self)?.name.text ?? declaration.as(StructDeclSyntax.self)?.name.text
            ?? declaration.as(ActorDeclSyntax.self)?.name.text ?? declaration.as(EnumDeclSyntax.self)?.name.text
        var methods: [Method] = []
        var seen: [String: FunctionDeclSyntax] = [:]
        var failed = false
        for member in declaration.memberBlock.members {
            guard let function = member.decl.as(FunctionDeclSyntax.self) else { continue }
            if has(function.modifiers, .private, .fileprivate, .static, .class) { continue }
            let name = function.name.text
            if name.hasPrefix("rutis") { continue }
            if let other = seen[name] {
                context.diagnose(Diagnostic(node: function.name, message: MacroError(
                    "@RutisService: two methods are named \(name) (\(trimmed(other.signature)) and \(trimmed(function.signature))): rutis calls a method by its base name; rename one or make it private")))
                failed = true
                continue
            }
            seen[name] = function
            if function.genericParameterClause != nil {
                context.diagnose(Diagnostic(node: function.name, message: MacroError(
                    "@RutisService: \(name) is generic: rutis cannot decode its arguments; make it non-generic or private")))
                failed = true
                continue
            }
            var lines: [String] = []
            var arguments: [String] = []
            for (index, parameter) in function.signature.parameterClause.parameters.enumerated() {
                let label = parameter.firstName.tokenKind == .wildcard ? "" : "\(parameter.firstName.text): "
                if parameter.ellipsis != nil || bareType(parameter.type).is(SomeOrAnyTypeSyntax.self) && trimmed(parameter.type).hasPrefix("some") {
                    context.diagnose(Diagnostic(node: parameter, message: MacroError(
                        "@RutisService: \(name): parameter \(trimmed(parameter)) cannot be decoded (variadic or `some`): take an array or a concrete type")))
                    failed = true
                    continue
                }
                if let attributed = parameter.type.as(AttributedTypeSyntax.self), attributed.specifiers.contains(where: { trimmed($0) == "inout" }) {
                    context.diagnose(Diagnostic(node: parameter, message: MacroError("@RutisService: \(name): an inout parameter cannot cross")))
                    failed = true
                    continue
                }
                if let (function, optional) = functionType(parameter.type) {
                    guard isAsyncThrows(function) else {
                        context.diagnose(Diagnostic(node: parameter, message: MacroError(
                            "@RutisService: \(name): a function parameter is called across processes: declare it `async throws` (\(trimmed(function)))")))
                        failed = true
                        continue
                    }
                    let type = optional ? "(\(trimmed(function)))?" : trimmed(function)
                    lines.append("let a\(index): \(type) = try args.\(optional ? "optionalFunction" : "function")(\(index))")
                } else {
                    lines.append("let a\(index) = try args.decode(\(index), as: (\(trimmed(bareType(parameter.type)))).self)")
                }
                arguments.append("\(label)a\(index)")
            }
            let effects = function.signature.effectSpecifiers
            let throwing = effects?.throwsClause != nil
            let tryWord = throwing ? "try " : ""
            let awaitWord = (effects?.asyncSpecifier != nil || isActor) ? "await " : ""
            var call = "\(tryWord)\(awaitWord)self.\(name)(\(arguments.joined(separator: ", ")))"
            // A method that throws: where it is declared crosses with what it
            // throws (Swift errors carry no stack, #677 §4.4).
            if throwing, let typeName, let site = context.location(of: function.funcKeyword, at: .afterLeadingTrivia, filePathMode: .fileID) {
                call = "try await Rutis.located(\"\(typeName).\(name)\", \(site.file), \(site.line)) { \(call) }"
            }
            if let output = function.signature.returnClause, trimmed(output.type) != "Void", trimmed(output.type) != "()" {
                lines.append("return try Rutis.encode(\(call))")
            } else {
                lines.append(call)
                lines.append("return try Rutis.encode(())")
            }
            methods.append(Method(name: name, body: lines.joined(separator: "\n")))
        }
        return failed ? nil : methods
    }

    public static func expansion(of _: AttributeSyntax, providingMembersOf declaration: some DeclGroupSyntax,
                                 conformingTo _: [TypeSyntax], in context: some MacroExpansionContext) throws -> [DeclSyntax]
    {
        guard declaration.is(ClassDeclSyntax.self) || declaration.is(StructDeclSyntax.self) || declaration.is(ActorDeclSyntax.self)
            || declaration.is(EnumDeclSyntax.self)
        else {
            throw MacroError("@RutisService applies to a class, struct, enum or actor")
        }
        guard let methods = methods(of: declaration, in: context) else { return [] }
        let access = access(of: declaration.modifiers)
        let isolation = declaration.is(ActorDeclSyntax.self) ? "nonisolated " : ""
        let names = methods.map { "\"\($0.name)\"" }.joined(separator: ", ")
        let entries = methods.map { method in
            "\"\(method.name)\": { args in\n\(method.body)\n}"
        }
        let table = entries.isEmpty ? "[:]" : "[\n" + entries.joined(separator: ",\n") + "\n]"
        return [
            "\(raw: access)static var rutisMethodNames: [String] { [\(raw: names)] }",
            "\(raw: access)\(raw: isolation)func rutisMethods() -> [String: Rutis.Method] {\n\(raw: table)\n}",
        ]
    }

    public static func expansion(of _: AttributeSyntax, attachedTo _: some DeclGroupSyntax, providingExtensionsOf type: some TypeSyntaxProtocol,
                                 conformingTo protocols: [TypeSyntax], in _: some MacroExpansionContext) throws -> [ExtensionDeclSyntax]
    {
        guard !protocols.isEmpty else { return [] }
        return [try ExtensionDeclSyntax("extension \(type.trimmed): RutisExportable {}")]
    }
}

// ── @RutisClient ────────────────────────────────────────────────────────

/// `@RutisClient` on a protocol: `<Protocol>Proxy`, a value of the protocol
/// whose methods call a service's (`ctx.use("llm", LLMProxy.self)`). Its
/// methods are `async throws`: a call to another process waits.
public struct ClientMacro: PeerMacro {
    public static func expansion(of _: AttributeSyntax, providingPeersOf declaration: some DeclSyntaxProtocol,
                                 in context: some MacroExpansionContext) throws -> [DeclSyntax]
    {
        guard let proto = declaration.as(ProtocolDeclSyntax.self) else {
            throw MacroError("@RutisClient applies to a protocol: the methods of the service it uses")
        }
        let access = access(of: proto.modifiers)
        let name = proto.name.text
        var methods: [String] = []
        var names: [String] = []
        var failed = false
        for member in proto.memberBlock.members {
            if let function = member.decl.as(FunctionDeclSyntax.self) {
                if has(function.modifiers, .static) {
                    context.diagnose(Diagnostic(node: function, message: MacroError("@RutisClient: \(function.name.text) is static: a proxy calls instance methods")))
                    failed = true
                    continue
                }
                let effects = function.signature.effectSpecifiers
                guard effects?.asyncSpecifier != nil, effects?.throwsClause != nil else {
                    context.diagnose(Diagnostic(node: function.name, message: MacroError(
                        "@RutisClient: \(function.name.text) must be `async throws`: calling another process waits, and can fail")))
                    failed = true
                    continue
                }
                if function.genericParameterClause != nil {
                    context.diagnose(Diagnostic(node: function.name, message: MacroError("@RutisClient: \(function.name.text) is generic")))
                    failed = true
                    continue
                }
                var arguments: [String] = []
                for (index, parameter) in function.signature.parameterClause.parameters.enumerated() {
                    let local = (parameter.secondName ?? parameter.firstName).text
                    let value = local == "_" ? "p\(index)" : local
                    if let (type, _) = functionType(parameter.type) {
                        guard isAsyncThrows(type) else {
                            context.diagnose(Diagnostic(node: parameter, message: MacroError(
                                "@RutisClient: \(function.name.text): a function argument is called from another process: declare it `async throws`")))
                            failed = true
                            continue
                        }
                        arguments.append("Rutis.export(\(value))")
                    } else {
                        arguments.append("try Rutis.encode(\(value))")
                    }
                }
                // The parameters as declared, each given a usable name.
                let parameters = function.signature.parameterClause.parameters.enumerated().map { index, parameter -> String in
                    var text = trimmed(parameter)
                    if text.hasSuffix(",") { text.removeLast() }
                    let local = (parameter.secondName ?? parameter.firstName).text
                    if local == "_" {
                        // `_: T` has no name to use: name it.
                        text = "_ p\(index): \(trimmed(parameter.type))"
                    }
                    return text
                }.joined(separator: ", ")
                let returns = function.signature.returnClause.map { trimmed($0.type) } ?? "Void"
                names.append("\"\(function.name.text)\"")
                methods.append("""
                \(access)func \(function.name.text)(\(parameters)) async throws -> \(returns) {
                    try await rutisClient.call("\(function.name.text)", [\(arguments.joined(separator: ", "))], as: (\(returns)).self)
                }
                """)
            } else if member.decl.is(VariableDeclSyntax.self) || member.decl.is(AssociatedTypeDeclSyntax.self)
                || member.decl.is(SubscriptDeclSyntax.self)
            {
                context.diagnose(Diagnostic(node: member, message: MacroError(
                    "@RutisClient: a client protocol lists methods only (properties, subscripts and associated types cannot be called across processes)")))
                failed = true
            }
        }
        if failed { return [] }
        return ["""
        \(raw: access)struct \(raw: name)Proxy: \(raw: name), RutisProxy {
            \(raw: access)typealias Service = any \(raw: name)
            \(raw: access)static var rutisMethodNames: [String] { [\(raw: names.joined(separator: ", "))] }
            \(raw: access)static func rutisMake(_ client: Rutis.Client) -> any \(raw: name) { \(raw: name)Proxy(rutisClient: client) }
            let rutisClient: Rutis.Client
            \(raw: access)init(rutisClient: Rutis.Client) { self.rutisClient = rutisClient }
        \(raw: methods.joined(separator: "\n"))
        }
        """]
    }
}

// ── @RutisConfig ────────────────────────────────────────────────────────

/// `@RutisConfig` on a configuration type: its JSON Schema from the stored
/// properties, each `///` comment its description; optional properties are
/// not required, and neither are `var`s with a literal initial value
/// (`var units = "metric"`), whose literal is their `default` (config schema
/// design §3.3): rutis fills it in before decoding.
public struct ConfigMacro: ExtensionMacro {
    public static func expansion(of _: AttributeSyntax, attachedTo declaration: some DeclGroupSyntax,
                                 providingExtensionsOf type: some TypeSyntaxProtocol, conformingTo protocols: [TypeSyntax],
                                 in context: some MacroExpansionContext) throws -> [ExtensionDeclSyntax]
    {
        guard declaration.is(StructDeclSyntax.self) || declaration.is(ClassDeclSyntax.self) else {
            throw MacroError("@RutisConfig applies to a struct or class: the configuration's type")
        }
        guard !protocols.isEmpty else { return [] }
        let access = access(of: declaration.modifiers)
        var properties: [String] = []
        for member in declaration.memberBlock.members {
            guard let variable = member.decl.as(VariableDeclSyntax.self), !has(variable.modifiers, .static, .class) else { continue }
            let isLet = variable.bindingSpecifier.tokenKind == .keyword(.let)
            let description = docComment(variable.leadingTrivia)
            for binding in variable.bindings {
                guard let pattern = binding.pattern.as(IdentifierPatternSyntax.self) else { continue }
                // Computed properties, and constants with a value, are not
                // decoded.
                if binding.accessorBlock != nil { continue }
                if isLet, binding.initializer != nil { continue }
                // The type as written, or that of a literal initial value
                // (`var units = "metric"`).
                let typeSyntax: TypeSyntax
                if let annotation = binding.typeAnnotation {
                    typeSyntax = bareType(annotation.type)
                } else if let inferred = binding.initializer.flatMap({ literalType($0.value) }) {
                    typeSyntax = TypeSyntax(stringLiteral: inferred)
                } else {
                    context.diagnose(Diagnostic(node: binding, message: MacroError(
                        "@RutisConfig: \(pattern.identifier.text) needs a type annotation for its schema")))
                    continue
                }
                let optional = typeSyntax.is(OptionalTypeSyntax.self) || typeSyntax.is(ImplicitlyUnwrappedOptionalTypeSyntax.self)
                    || trimmed(typeSyntax).hasPrefix("Optional<")
                let quoted = description.map(swiftString) ?? "nil"
                // A literal initial value is the default; any other initial
                // value is not, and the property stays required.
                let literal = binding.initializer.flatMap { jsonLiteral($0.value) }
                let defaulted = literal.map { ", default: \(swiftString($0))" } ?? ""
                properties.append(".init(\"\(pattern.identifier.text)\", .of((\(trimmed(typeSyntax))).self), required: \(!optional && literal == nil), description: \(quoted)\(defaulted))")
            }
        }
        return [try ExtensionDeclSyntax("""
        extension \(type.trimmed): RutisConfigurable {
            \(raw: access)static var rutisSchema: Rutis.Schema {
                .object([\(raw: properties.joined(separator: ", "))])
            }
        }
        """)]
    }

    /// The type Swift gives a plain literal: `String`, `Int`, `Double`,
    /// `Bool`; `nil` for anything else.
    static func literalType(_ expression: ExprSyntax) -> String? {
        if expression.is(StringLiteralExprSyntax.self) { return "String" }
        if expression.is(IntegerLiteralExprSyntax.self) { return "Int" }
        if expression.is(FloatLiteralExprSyntax.self) { return "Double" }
        if expression.is(BooleanLiteralExprSyntax.self) { return "Bool" }
        if let prefix = expression.as(PrefixOperatorExprSyntax.self), prefix.operator.text == "-" {
            let inner = literalType(prefix.expression)
            return inner == "Int" || inner == "Double" ? inner : nil
        }
        return nil
    }

    /// `text` as a Swift string literal.
    static func swiftString(_ text: String) -> String {
        "\"" + text.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
            .replacingOccurrences(of: "\n", with: "\\n") + "\""
    }

    /// The JSON text of `expression` when it is a literal: a string without
    /// interpolation, a number, a boolean, or an array or string-keyed
    /// dictionary of them. `nil` for anything else.
    static func jsonLiteral(_ expression: ExprSyntax) -> String? {
        if let string = expression.as(StringLiteralExprSyntax.self) {
            var text = ""
            for segment in string.segments {
                guard let part = segment.as(StringSegmentSyntax.self) else { return nil }
                text += part.content.text
            }
            // Swift escapes as written are JSON's for the usual ones.
            if text.contains("\\(") || text.contains("\\u{") { return nil }
            let unescaped = text.replacingOccurrences(of: "\\\"", with: "\"")
            var out = "\""
            for c in unescaped.unicodeScalars {
                switch c {
                case "\"": out += "\\\""
                case "\n": out += "\\n"
                case "\t": out += "\\t"
                default: out.unicodeScalars.append(c)
                }
            }
            return out + "\""
        }
        if let integer = expression.as(IntegerLiteralExprSyntax.self) {
            return Int(integer.literal.text.replacingOccurrences(of: "_", with: "")).map(String.init)
        }
        if let float = expression.as(FloatLiteralExprSyntax.self) {
            return Double(float.literal.text.replacingOccurrences(of: "_", with: "")).map { "\($0)" }
        }
        if let boolean = expression.as(BooleanLiteralExprSyntax.self) {
            return boolean.literal.tokenKind == .keyword(.true) ? "true" : "false"
        }
        if let prefix = expression.as(PrefixOperatorExprSyntax.self), prefix.operator.text == "-" {
            guard let number = jsonLiteral(prefix.expression), number.first?.isNumber == true else { return nil }
            return "-" + number
        }
        if let array = expression.as(ArrayExprSyntax.self) {
            var items: [String] = []
            for element in array.elements {
                guard let item = jsonLiteral(element.expression) else { return nil }
                items.append(item)
            }
            return "[" + items.joined(separator: ",") + "]"
        }
        if let dictionary = expression.as(DictionaryExprSyntax.self) {
            guard case let .elements(elements) = dictionary.content else { return "{}" }
            var fields: [String] = []
            for element in elements {
                guard let key = jsonLiteral(element.key), key.hasPrefix("\""), let value = jsonLiteral(element.value) else { return nil }
                fields.append(key + ":" + value)
            }
            return "{" + fields.joined(separator: ",") + "}"
        }
        return nil
    }

    /// The text of `///` comments before a declaration.
    static func docComment(_ trivia: Trivia) -> String? {
        var lines: [String] = []
        for piece in trivia {
            if case let .docLineComment(text) = piece {
                lines.append(String(text.dropFirst(3)).trimmingCharacters(in: .whitespaces))
            }
        }
        return lines.isEmpty ? nil : lines.joined(separator: " ")
    }
}

