import RutisMacrosCore
import SwiftParser
import SwiftSyntax
import SwiftSyntaxMacroExpansion
import SwiftSyntaxMacros

/// `source` with the macros expanded, and the diagnostics they gave.
func expand(_ source: String, file path: String? = nil) -> (String, [String]) {
    let file = Parser.parse(source: source)
    // With a path, the macros know where declarations are.
    let context = path.map { BasicMacroExpansionContext(sourceFiles: [file: .init(moduleName: "Bank", fullFilePath: $0)]) }
        ?? BasicMacroExpansionContext()
    let expanded = file.expand(macroSpecs: [
        "RutisService": MacroSpec(type: ServiceMacro.self, conformances: ["RutisExportable"]),
        "RutisClient": MacroSpec(type: ClientMacro.self),
        "RutisConfig": MacroSpec(type: ConfigMacro.self, conformances: ["RutisConfigurable"]),
    ], contextGenerator: { _ in context }, indentationWidth: .spaces(4))
    return (expanded.description, context.diagnostics.map(\.message))
}

/// `text` without its whitespace, to compare generated code by content.
func squeezed(_ text: String) -> String { text.filter { !$0.isWhitespace } }

struct ServiceMacroTests {
    func aMethodTableFromTheTypesMethods() {
        let (out, diagnostics) = expand("""
        @RutisService
        public final class Weather {
            public func today(_ question: String, days n: Int) async throws -> String { "" }
            func unit() -> String { "c" }
            func each(_ f: @escaping (String) async throws -> String) async throws -> [String] { [] }
            func reset() {}
            private func hidden() {}
            static func make() -> Weather { Weather() }
            var city = "Oslo"
        }
        """)
        expect(diagnostics.isEmpty, "\(diagnostics)")
        let code = squeezed(out)
        expect(code.contains(squeezed("public static var rutisMethodNames: [String] { [\"today\", \"unit\", \"each\", \"reset\"] }")), "\(out)")
        expect(code.contains(squeezed("let a0 = try args.decode(0, as: (String).self)")), "\(out)")
        expect(code.contains(squeezed("return try Rutis.encode(try await self.today(a0, days: a1))")), "\(out)")
        expect(code.contains(squeezed("return try Rutis.encode(self.unit())")), "\(out)")
        expect(code.contains(squeezed("let a0: (String) async throws -> String = try args.function(0)")), "\(out)")
        expect(code.contains(squeezed("self.reset()\nreturn try Rutis.encode(())")), "\(out)")
        expect(!code.contains("\"hidden\"") && !code.contains("\"make\""), "\(out)")
        expect(code.contains(squeezed("extension Weather: RutisExportable {}")), "\(out)")
    }

    func anActorsMethodsAreAwaited() {
        let (out, diagnostics) = expand("""
        @RutisService
        actor Counter {
            func add(_ n: Int) -> Int { n }
        }
        """)
        expect(diagnostics.isEmpty, "\(diagnostics)")
        expect(squeezed(out).contains(squeezed("nonisolated func rutisMethods()")), "\(out)")
        expect(squeezed(out).contains(squeezed("return try Rutis.encode(await self.add(a0))")), "\(out)")
    }

    func aThrowingMethodIsLocated() {
        let (out, diagnostics) = expand("""
        @RutisService
        final class Account {
            func balance() -> Int { 3 }

            /// Takes money out.
            @discardableResult
            func withdraw(_ amount: Int) async throws -> Int { amount }
            func close() throws {}
        }
        """, file: "/src/Bank/Account.swift")
        expect(diagnostics.isEmpty, "\(diagnostics)")
        let code = squeezed(out)
        expect(code.contains(squeezed("return try Rutis.encode(self.balance())")), "\(out)")
        expect(code.contains(squeezed("return try Rutis.encode(try await Rutis.located(\"Account.withdraw\", \"Bank/Account.swift\", 7) { try await self.withdraw(a0) })")), "\(out)")
        expect(code.contains(squeezed("try await Rutis.located(\"Account.close\", \"Bank/Account.swift\", 8) { try self.close() }")), "\(out)")
    }

    func overloadsAreAnError() {
        let (_, diagnostics) = expand("""
        @RutisService
        final class Weather {
            func today() -> String { "" }
            func today(_ city: String) -> String { "" }
        }
        """)
        expect(diagnostics.contains { $0.contains("two methods are named today") }, "\(diagnostics)")
    }

    func aFunctionParameterMustBeAsyncThrows() {
        let (_, diagnostics) = expand("""
        @RutisService
        final class Weather {
            func each(_ f: @escaping (String) -> String) -> [String] { [] }
        }
        """)
        expect(diagnostics.contains { $0.contains("declare it `async throws`") }, "\(diagnostics)")
    }
}

struct ClientMacroTests {
    func aProxyOfTheProtocol() {
        let (out, diagnostics) = expand("""
        @RutisClient
        public protocol LLM {
            func ask(_ question: String, model: String?) async throws -> String
            func each(_ f: @escaping (String) async throws -> String) async throws
        }
        """)
        expect(diagnostics.isEmpty, "\(diagnostics)")
        let code = squeezed(out)
        expect(code.contains(squeezed("public struct LLMProxy: LLM, RutisProxy {")), "\(out)")
        expect(code.contains(squeezed("public static var rutisMethodNames: [String] { [\"ask\", \"each\"] }")), "\(out)")
        expect(code.contains(squeezed("try await rutisClient.call(\"ask\", [try Rutis.encode(question), try Rutis.encode(model)], as: (String).self)")), "\(out)")
        expect(code.contains(squeezed("try await rutisClient.call(\"each\", [Rutis.export(f)], as: (Void).self)")), "\(out)")
    }

    func aMethodMustBeAsyncThrows() {
        let (_, diagnostics) = expand("""
        @RutisClient
        protocol LLM {
            func ask(_ question: String) -> String
        }
        """)
        expect(diagnostics.contains { $0.contains("must be `async throws`") }, "\(diagnostics)")
    }
}

struct ConfigMacroTests {
    func aSchemaFromStoredProperties() {
        let (out, diagnostics) = expand("""
        @RutisConfig
        struct Config: Decodable {
            /// the city to report on
            var city: String
            var days: Int?
            let fixed = 3
            var computed: Int { 1 }
            var units = "metric"
            var limit: Int = -5
            var tags: [String] = ["a", "b\\"c"]
            var made: String = String(repeating: "x", count: 2)
        }
        """)
        expect(diagnostics.isEmpty, "\(diagnostics)")
        let code = squeezed(out)
        expect(code.contains(squeezed("extension Config: RutisConfigurable")), "\(out)")
        expect(code.contains(squeezed(".init(\"city\", .of((String).self), required: true, description: \"the city to report on\")")), "\(out)")
        expect(code.contains(squeezed(".init(\"days\", .of((Int?).self), required: false, description: nil)")), "\(out)")
        expect(!code.contains("\"fixed\"") && !code.contains("\"computed\""), "\(out)")
        // A literal initial value is the default, and not required; any
        // other initial value is no default (config schema design §3.3).
        expect(code.contains(squeezed(#".init("units", .of((String).self), required: false, description: nil, default: "\"metric\"")"#)), "\(out)")
        expect(code.contains(squeezed(#".init("limit", .of((Int).self), required: false, description: nil, default: "-5")"#)), "\(out)")
        expect(code.contains(squeezed(#".init("tags", .of(([String]).self), required: false, description: nil, default: "[\"a\",\"b\\\"c\"]")"#)), "\(out)")
        expect(code.contains(squeezed(".init(\"made\", .of((String).self), required: true, description: nil)")), "\(out)")
    }
}
