// What a call cannot send (#675): a value the far end cannot take, or a
// message over the limit, fails only that call, on the side that would send
// it, and the error says which call, which argument (or the result) and why.
// The session goes on. The words are the same in every language.

/// What a call is, for messages: `service.method` for an invocation (its
/// target without a `host:` prefix or a scope label), the method alone for a
/// control call (no target).
func describeInvocation(target: String, method: String) -> String {
    var service = Substring(target)
    if service.hasPrefix("host:") { service = service.dropFirst(5) }
    if let nul = service.firstIndex(of: "\0") { service = service[..<nul] }
    return service.isEmpty ? method : "\(service).\(method)"
}

/// Why a message of `size` bytes is not sent over a channel taking at most
/// `limit`.
func overLimit(_ size: Int, _ limit: Int = maxMessage) -> String {
    let mib = limit % (1 << 20) == 0 ? " (\(limit >> 20) MiB)" : ""
    return "the message would be \(size) bytes, over the limit of \(limit) bytes\(mib)"
}

/// `reason` as one of the result of `call`, when known: `<call>: the
/// result cannot be sent: <why>`, code bad-value (#677).
func ofResult(_ reason: String, _ call: String?, limit: Int? = nil) -> ValueError {
    var data: [String: Any] = ["part": "result"]
    if let limit { data["limit"] = limit }
    return ValueError(call.map { "\($0): the result cannot be sent: \(reason)" } ?? "the result cannot be sent: \(reason)", data: data)
}

/// `reason` as one of argument `index` (from 0) of `call`: `<call>:
/// argument <i> cannot be sent: <why>`, code bad-value.
func ofArgument(_ reason: String, _ index: Int, _ call: String, limit: Int? = nil) -> ValueError {
    var data: [String: Any] = ["part": "argument", "argument": index + 1]
    if let limit { data["limit"] = limit }
    return ValueError("\(call): argument \(index + 1) cannot be sent: \(reason)", data: data)
}

/// An object reference received from the far end (an older host's, say):
/// only its call fails.
let objectReceived = "received an object with methods, which Swift cannot take; pass plain data or a function instead"
