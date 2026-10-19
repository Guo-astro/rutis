// The compiler plugin of the SDK's macros: each macro forwards to
// RutisMacrosCore, which generates the code (a library, so the SDK's tests
// check it too).
import RutisMacrosCore
import SwiftCompilerPlugin
import SwiftSyntax
import SwiftSyntaxMacros

@main
struct RutisMacrosPlugin: CompilerPlugin {
    let providingMacros: [Macro.Type] = [ServiceMacro.self, ClientMacro.self, ConfigMacro.self]
}

public struct ServiceMacro: MemberMacro, ExtensionMacro {
    public static func expansion(of node: AttributeSyntax, providingMembersOf declaration: some DeclGroupSyntax,
                                 conformingTo protocols: [TypeSyntax], in context: some MacroExpansionContext) throws -> [DeclSyntax]
    {
        try RutisMacrosCore.ServiceMacro.expansion(of: node, providingMembersOf: declaration, conformingTo: protocols, in: context)
    }

    public static func expansion(of node: AttributeSyntax, attachedTo declaration: some DeclGroupSyntax,
                                 providingExtensionsOf type: some TypeSyntaxProtocol, conformingTo protocols: [TypeSyntax],
                                 in context: some MacroExpansionContext) throws -> [ExtensionDeclSyntax]
    {
        try RutisMacrosCore.ServiceMacro.expansion(of: node, attachedTo: declaration, providingExtensionsOf: type,
                                                   conformingTo: protocols, in: context)
    }
}

public struct ClientMacro: PeerMacro {
    public static func expansion(of node: AttributeSyntax, providingPeersOf declaration: some DeclSyntaxProtocol,
                                 in context: some MacroExpansionContext) throws -> [DeclSyntax]
    {
        try RutisMacrosCore.ClientMacro.expansion(of: node, providingPeersOf: declaration, in: context)
    }
}

public struct ConfigMacro: ExtensionMacro {
    public static func expansion(of node: AttributeSyntax, attachedTo declaration: some DeclGroupSyntax,
                                 providingExtensionsOf type: some TypeSyntaxProtocol, conformingTo protocols: [TypeSyntax],
                                 in context: some MacroExpansionContext) throws -> [ExtensionDeclSyntax]
    {
        try RutisMacrosCore.ConfigMacro.expansion(of: node, attachedTo: declaration, providingExtensionsOf: type,
                                                  conformingTo: protocols, in: context)
    }
}
