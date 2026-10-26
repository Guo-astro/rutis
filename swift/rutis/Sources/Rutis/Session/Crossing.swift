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

/// `reason` as one of the result of `call`, when known.
func ofResult(_ reason: String, _ call: String?) -> ValueError {
    ValueError(call.map { "the result of \($0): \(reason)" } ?? "the result: \(reason)")
}

/// An object reference received from the far end (an older host's, say):
/// only its call fails.
let objectReceived = "received an object with methods, which Swift cannot take; pass plain data or a function instead"
