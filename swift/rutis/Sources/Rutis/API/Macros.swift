/// Makes a type's instances services: generates its method table (each
/// non-private instance method of the declaration, by its base name) and its
/// conformance to RutisExportable. Its instances cross by reference.
@attached(member, names: named(rutisMethodNames), named(rutisMethods))
@attached(extension, conformances: RutisExportable)
public macro RutisService() = #externalMacro(module: "RutisMacros", type: "ServiceMacro")

/// On a protocol listing the methods of a service a plugin uses (each
/// `async throws`): generates `<Protocol>Proxy` for `ctx.use`.
@attached(peer, names: suffixed(Proxy))
public macro RutisClient() = #externalMacro(module: "RutisMacros", type: "ClientMacro")

/// On a configuration type: generates its JSON Schema from its stored
/// properties (`///` comments become descriptions).
@attached(extension, conformances: RutisConfigurable, names: named(rutisSchema))
public macro RutisConfig() = #externalMacro(module: "RutisMacros", type: "ConfigMacro")
