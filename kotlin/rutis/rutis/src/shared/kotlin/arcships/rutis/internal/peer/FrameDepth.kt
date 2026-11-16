package arcships.rutis.internal.peer

/** What a frame says at its top: the two fields that can be read without parsing it. */
internal class FrameHead(val op: String?, val id: String?)

/**
 * Whether JSON `text` nests arrays and objects deeper than `limit`, found
 * without parsing it (so without recursing): what keeps a deep frame from
 * overflowing the reader's stack before a parser sees it. Malformed text is
 * only measured, not judged.
 */
internal fun frameDeeperThan(text: CharSequence, limit: Int): Boolean {
    var depth = 0
    var string = false
    var escaped = false
    for (c in text) {
        if (string) {
            when {
                escaped -> escaped = false
                c == '\\' -> escaped = true
                c == '"' -> string = false
            }
            continue
        }
        when (c) {
            '"' -> string = true
            '[', '{' -> {
                depth++
                if (depth > limit) return true
            }
            ']', '}' -> if (depth > 0) depth--
        }
    }
    return false
}

/**
 * The top-level `op` and `id` of a frame too deep to parse, read from its
 * text without recursing into the value: a frame that names a call can fail
 * that call alone, while one that names none ends the session. Null when
 * there is no top-level object to read them from.
 */
internal fun frameHead(text: String): FrameHead? {
    var at = 0

    fun space() {
        while (at < text.length && (text[at] == ' ' || text[at] == '\t' || text[at] == '\n' || text[at] == '\r')) at++
    }

    /** A JSON string at [at], unescaped; null when it is malformed. */
    fun string(): String? {
        if (at >= text.length || text[at] != '"') return null
        at++
        val out = StringBuilder()
        while (at < text.length) {
            val c = text[at++]
            when (c) {
                '"' -> return out.toString()
                '\\' -> {
                    if (at >= text.length) return null
                    when (val escape = text[at++]) {
                        '"', '\\', '/' -> out.append(escape)
                        'b' -> out.append('\b')
                        'f' -> out.append('\u000C')
                        'n' -> out.append('\n')
                        'r' -> out.append('\r')
                        't' -> out.append('\t')
                        'u' -> {
                            if (at + 4 > text.length) return null
                            val code = text.substring(at, at + 4).toIntOrNull(16) ?: return null
                            at += 4
                            out.append(code.toChar())
                        }
                        else -> return null
                    }
                }
                else -> out.append(c)
            }
        }
        return null
    }

    /**
     * Skips one JSON value at [at], of any depth, by counting brackets rather
     * than recursing; false when it is malformed or unterminated.
     */
    fun value(): Boolean {
        var depth = 0
        var inString = false
        var escaped = false
        var kind = 0 // 0: none yet, 1: a primitive, 2: a string, 3: an array or object
        while (at < text.length) {
            val c = text[at]
            if (inString) {
                when {
                    escaped -> escaped = false
                    c == '\\' -> escaped = true
                    c == '"' -> {
                        inString = false
                        at++
                        if (depth == 0) return true
                        continue
                    }
                }
                at++
                continue
            }
            when (c) {
                '"' -> {
                    inString = true
                    kind = 2
                    at++
                }
                '{', '[' -> {
                    depth++
                    kind = 3
                    at++
                }
                '}', ']' -> {
                    // A primitive ends where the object or array holding it does.
                    if (depth == 0) return kind != 0
                    depth--
                    at++
                    if (depth == 0) return true
                }
                ',' -> {
                    if (depth == 0) return kind != 0
                    at++
                }
                ' ', '\t', '\n', '\r' -> {
                    if (depth == 0 && kind != 0) return true
                    at++
                }
                else -> {
                    kind = 1
                    at++
                }
            }
        }
        return false
    }

    space()
    if (at >= text.length || text[at] != '{') return null
    at++
    var op: String? = null
    var id: String? = null
    while (true) {
        space()
        if (at >= text.length) return null
        if (text[at] == '}') {
            at++
            return FrameHead(op, id)
        }
        val key = string() ?: return null
        space()
        if (at >= text.length || text[at] != ':') return null
        at++
        space()
        when (key) {
            "op" -> op = string() ?: return null
            "id" -> id = string() ?: return null
            else -> if (!value()) return null
        }
        space()
        if (at >= text.length) return null
        when (text[at]) {
            ',' -> at++
            '}' -> {
                at++
                return FrameHead(op, id)
            }
            else -> return null
        }
    }
}
