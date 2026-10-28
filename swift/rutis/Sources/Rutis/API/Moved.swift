// The names that moved to `Rutis.Advanced` and `Rutis._Generated` in 0.19
// (#692). They still work, as the same types and functions, with a warning
// that names the new place; they are removed when the plugin API version
// changes.

extension Rutis {
    @available(*, deprecated, renamed: "Rutis.Advanced.sdkVersion")
    public static var sdkVersion: String { Advanced.sdkVersion }
    @available(*, deprecated, renamed: "Rutis.Advanced.pluginAPI")
    public static var pluginAPI: Int { Advanced.pluginAPI }
    @available(*, deprecated, renamed: "Rutis.Advanced.implementation")
    public static var implementation: String { Advanced.implementation }

    @available(*, deprecated, renamed: "Rutis.Advanced.CalendarDate")
    public typealias CalendarDate = Advanced.CalendarDate
    @available(*, deprecated, renamed: "Rutis.Advanced.TimeOfDay")
    public typealias TimeOfDay = Advanced.TimeOfDay
    @available(*, deprecated, renamed: "Rutis.Advanced.DateTime")
    public typealias DateTime = Advanced.DateTime
    @available(*, deprecated, renamed: "Rutis.Advanced.BigInt")
    public typealias BigInt = Advanced.BigInt

    @available(*, deprecated, renamed: "Rutis.Advanced.serveArgs")
    public static func serveArgs(_ args: [String], _ plugins: [Definition]) throws {
        try serveArguments(args, plugins)
    }
    @available(*, deprecated, renamed: "Rutis.Advanced.manifest")
    public static func manifest(_ plugins: [Definition]) -> String { manifestText(plugins) }

    @available(*, deprecated, renamed: "Rutis.Advanced.Emitter")
    public typealias Emitter = Advanced.Emitter
    @available(*, deprecated, renamed: "Rutis.Advanced.Scope")
    public typealias Scope = Advanced.Scope
    @available(*, deprecated, renamed: "Rutis.Advanced.Listener")
    public typealias Listener = Advanced.Listener
    @available(*, deprecated, renamed: "Rutis.Advanced.Next")
    public typealias Next<R> = Advanced.Next<R>

    @available(*, deprecated, renamed: "Rutis.Advanced.Client")
    public typealias Client = Advanced.Client
    @available(*, deprecated, renamed: "Rutis.Advanced.RemoteFunction")
    public typealias RemoteFunction = Advanced.RemoteFunction

    @available(*, deprecated, renamed: "Rutis.Advanced.MethodKind")
    public typealias MethodKind = Advanced.MethodKind
    @available(*, deprecated, renamed: "Rutis.Advanced.Shape")
    public typealias Shape = Advanced.Shape

    @available(*, deprecated, renamed: "Rutis._Generated.Encoded")
    public typealias Encoded = _Generated.Encoded
    @available(*, deprecated, renamed: "Rutis._Generated.Arguments")
    public typealias Arguments = _Generated.Arguments
    @available(*, deprecated, renamed: "Rutis._Generated.Method")
    public typealias Method = _Generated.Method
    @available(*, deprecated, renamed: "Rutis._Generated.Schema")
    public typealias Schema = _Generated.Schema

    @available(*, deprecated, renamed: "Rutis._Generated.encode")
    public static func encode<T>(_ value: T) throws -> _Generated.Encoded { try _Generated.encode(value) }
    @available(*, deprecated, renamed: "Rutis._Generated.export")
    public static func export<each A, R>(_ function: @escaping (repeat each A) async throws -> R) -> _Generated.Encoded {
        _Generated.export(function)
    }
    @available(*, deprecated, renamed: "Rutis._Generated.export")
    public static func export<each A, R>(_ function: ((repeat each A) async throws -> R)?) -> _Generated.Encoded {
        _Generated.export(function)
    }
    @available(*, deprecated, renamed: "Rutis._Generated.located")
    public static func located<T>(_ method: String, _ file: String, _ line: Int,
                                  _ body: () async throws -> T) async throws -> T
    {
        try await _Generated.located(method, file, line, body)
    }
}
