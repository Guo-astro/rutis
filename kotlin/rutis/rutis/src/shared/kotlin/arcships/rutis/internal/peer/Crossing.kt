package arcships.rutis.internal.peer

// What a call cannot send (#675): a value the far end cannot take, or a
// message over the limit, fails only that call, on the side that would send
// it, and the error says which call, which argument (or the result) and why.
// The session goes on. The words are the same in every language.

/**
 * What a call is, for messages: `service.method` for an invocation (its
 * target without a `host:` prefix or a scope label), the method alone for a
 * control call (no target).
 */
internal fun invocation(target: String, method: String): String {
    var service = target.removePrefix("host:")
    val nul = service.indexOf('\u0000')
    if (nul >= 0) service = service.substring(0, nul)
    return if (service.isEmpty()) method else "$service.$method"
}

/** Why a message of `size` bytes is not sent over a channel taking at most `limit`. */
internal fun overLimit(size: Int, limit: Int = MAX_MESSAGE): String {
    val mib = if (limit % (1 shl 20) == 0) " (${limit shr 20} MiB)" else ""
    return "the message would be $size bytes, over the limit of $limit bytes$mib"
}

/**
 * `reason` as one of argument `index` (from 0) of `call`: `<call>: argument
 * <i> cannot be sent: <why>`, code bad-value with `{argument, part, limit?}`
 * (#677).
 */
internal fun ofArgument(reason: String?, index: Int, call: String, limit: Int? = null): ValueException =
    ValueException("$call: argument ${index + 1} cannot be sent: $reason", "bad-value",
        listOfNotNull("part" to "argument", "argument" to index + 1, limit?.let { "limit" to it }).toMap())

/** `reason` as one of the result of `call`, when known: `<call>: the result cannot be sent: <why>`. */
internal fun ofResult(reason: String?, call: String?, limit: Int? = null): ValueException =
    ValueException(if (call != null) "$call: the result cannot be sent: $reason" else "the result cannot be sent: $reason", "bad-value",
        listOfNotNull("part" to "result", limit?.let { "limit" to it }).toMap())

/** `reason` as one of the arguments of `call` together. */
internal fun ofArguments(reason: String?, call: String, limit: Int? = null): ValueException =
    ValueException("$call: the arguments cannot be sent: $reason", "bad-value",
        listOfNotNull("part" to "arguments", limit?.let { "limit" to it }).toMap())

/** An object with methods sent to a far end that did not declare `objects`. */
internal fun objectRefused(original: Any?, far: String?): ValueException {
    val type = original?.javaClass?.simpleName?.takeIf { it.isNotEmpty() }?.let { " ($it)" } ?: ""
    return ValueException("an object with methods$type cannot cross to ${far ?: "the far end"}; pass plain data or a function instead")
}

/** An object reference received from the far end (an older host's, say): only its call fails. */
internal fun objectReceived(): ValueException =
    ValueException("received an object with methods, which Kotlin cannot take; pass plain data or a function instead")
