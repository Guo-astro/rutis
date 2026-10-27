package arcships.rutis.internal

import java.math.BigDecimal
import java.math.BigInteger
import arcships.rutis.internal.peer.Undefined

/**
 * A row's config against its plugin's JSON Schema: defaults filled, then
 * validated (config schema design, docs/design-config-schema-2026-10-27.en.md
 * §3–§6). The rules every language's SDK has; the shared case list
 * specs/config-schema/cases.json keeps them the same word for word, and its
 * README states the details settled while implementing.
 *
 * Values are JSON as Kotlin holds it here: `null`, [Boolean], [String],
 * [Num] (a number as its JSON text), [MutableList] and [LinkedHashMap].
 *
 * Keywords checked: type, properties, required, additionalProperties, items,
 * enum, const, minimum, maximum, default, anyOf, oneOf, allOf, $ref (to
 * #/$defs/… and #/definitions/…). Others are neither checked nor warned about.
 */
object ConfigSchema {
    /** A number as its JSON text. */
    data class Num(val text: String) {
        val isIntegerLiteral: Boolean get() = text.none { it == '.' || it == 'e' || it == 'E' }
        val double: Double get() = text.toDouble()
        val decimal: BigDecimal get() = BigDecimal(text)
    }

    /** One problem: where it is (keys and indices), and its sentence. */
    data class Issue(val path: List<Any>, val message: String)

    /** What checking a config gives. */
    class Checked(val config: Any?, val errors: List<Issue>, val warnings: List<Issue>)

    /** At most this many problems are written; the rest as one last line. */
    private const val SHOWN = 20
    private val TYPES = listOf("string", "number", "integer", "boolean", "object", "array", "null")
    private val MISSING = Any()

    /** Fill [config] (`null`: no config) with the defaults of [schema], then validate it. */
    fun check(schema: Any?, config: Any?, required: Boolean = true): Checked {
        val doc = Document(schema)
        val none = config == null
        var value: Any? = when {
            !none -> copy(config)
            doc.defaultOf(listOf(Node(schema, ""))) !== MISSING -> copy(doc.defaultOf(listOf(Node(schema, ""))))
            doc.isObjectRoot(schema) -> LinkedHashMap<String, Any?>()
            else -> null
        }
        doc.fill(listOf(Node(schema, "")), value)
        val sink = Sink(warn = true)
        val holder = arrayOf(value)
        doc.validate(listOf(Node(schema, "")), holder, emptyList(), required, sink, HashMap())
        value = holder[0]
        if (value == null && none && sink.errors.isNotEmpty()) {
            sink.errors.clear()
            sink.errors.add(issue(emptyList(), "is required"))
        }
        return Checked(value, sorted(sink.errors), sorted(sink.warnings))
    }

    /** The lines a config error is written as: `config: <problem>`, at most 20, then `config: and N more problems`. */
    fun lines(issues: List<Issue>): List<String> {
        val out = issues.take(SHOWN).map { "config: ${it.message}" }.toMutableList()
        if (issues.size > SHOWN) out.add("config: and ${issues.size - SHOWN} more problems")
        return out
    }

    /** How a path is written: `home.lat`, `cities[2]`, `headers["x-api"]`; `config` for the root. */
    fun pathText(path: List<Any>): String {
        val out = StringBuilder()
        path.forEachIndexed { position, segment ->
            val first = if (position == 0) "config" else ""
            when {
                segment is Int -> out.append("$first[$segment]")
                identifier(segment as String) -> out.append(if (position == 0) segment else ".$segment")
                else -> out.append("$first[${quoted(segment, null)}]")
            }
        }
        return if (out.isEmpty()) "config" else out.toString()
    }

    /** How a value is written after `got`. */
    fun valueText(value: Any?): String = when (value) {
        null -> "null"
        is Boolean -> value.toString()
        is Num -> numberText(value.text)
        is String -> quoted(value, 40)
        is List<*> -> "an array of ${value.size}"
        else -> "an object"
    }

    /** A number as JavaScript's `String(number)` writes it; an integral value of magnitude at most 2^53 without a decimal point. */
    fun numberText(text: String): String {
        if (Num(text).isIntegerLiteral) text.toBigIntegerOrNull()?.let { return it.toString() }
        val f = text.toDoubleOrNull() ?: return text
        return doubleText(f)
    }

    /** [f] as JavaScript's `String(number)` writes it. */
    fun doubleText(f: Double): String {
        if (!f.isFinite()) return "null"
        if (f == 0.0) return "0"
        if (f == Math.rint(f) && Math.abs(f) <= 9007199254740992.0) return f.toLong().toString()
        // Shortest round-trip digits and the exponent (Java's `Double.toString`
        // gives the shortest digits since JDK 19).
        val shortest = BigDecimal(java.lang.Double.toString(Math.abs(f))).stripTrailingZeros()
        val digits = shortest.unscaledValue().toString()
        val n = digits.length - shortest.scale()
        val k = digits.length
        val body = when {
            n in k..21 -> digits + "0".repeat(n - k)
            n in 1..21 -> digits.substring(0, n) + "." + digits.substring(n)
            n in -5..0 -> "0." + "0".repeat(-n) + digits
            else -> {
                val e = n - 1
                val mark = if (e >= 0) "e+$e" else "e-${-e}"
                (if (k == 1) digits else digits.substring(0, 1) + "." + digits.substring(1)) + mark
            }
        }
        return (if (f < 0) "-" else "") + body
    }

    /** The name of [known] closest to [name], when close enough to be a misspelling. */
    fun closest(name: String, known: List<String>): String? {
        val length = name.codePointCount(0, name.length)
        if (length <= 2) return null
        val limit = if (length <= 6) 1 else 2
        var best: String? = null
        var bestDistance = Int.MAX_VALUE
        for (candidate in known) {
            if (candidate == name) continue
            val d = distance(name, candidate)
            if (d <= limit && d < bestDistance) {
                best = candidate
                bestDistance = d
            }
        }
        return best
    }

    /** Edits (insert, delete, replace, swap two neighbours) between [a] and [b], in scalar values. */
    fun distance(a: String, b: String): Int {
        val x = a.codePoints().toArray()
        val y = b.codePoints().toArray()
        val rows = Array(x.size + 1) { i -> IntArray(y.size + 1) { j -> if (i == 0) j else if (j == 0) i else 0 } }
        for (i in 1..x.size) {
            for (j in 1..y.size) {
                val cost = if (x[i - 1] == y[j - 1]) 0 else 1
                var best = minOf(rows[i - 1][j] + 1, rows[i][j - 1] + 1, rows[i - 1][j - 1] + cost)
                if (i > 1 && j > 1 && x[i - 1] == y[j - 2] && x[i - 2] == y[j - 1]) best = minOf(best, rows[i - 2][j - 2] + 1)
                rows[i][j] = best
            }
        }
        return rows[x.size][y.size]
    }

    /** Strings by code point (Kotlin's `compareTo` compares UTF-16 units). */
    val byCodePoint: Comparator<String> = Comparator { a, b ->
        val x = a.codePoints().toArray()
        val y = b.codePoints().toArray()
        for (i in 0 until minOf(x.size, y.size)) if (x[i] != y[i]) return@Comparator x[i].compareTo(y[i])
        x.size.compareTo(y.size)
    }

    private fun identifier(key: String): Boolean =
        key.isNotEmpty() && (key[0] in 'A'..'Z' || key[0] in 'a'..'z' || key[0] == '_') &&
            key.all { it in 'A'..'Z' || it in 'a'..'z' || it in '0'..'9' || it == '_' }

    /** [s] as a JSON string, its content cut to [limit] scalar values with `…`. */
    private fun quoted(s: String, limit: Int?): String {
        val count = s.codePointCount(0, s.length)
        val shown = if (limit == null || count <= limit) s else s.substring(0, s.offsetByCodePoints(0, limit))
        val out = StringBuilder("\"")
        for (c in shown) {
            when {
                c == '"' -> out.append("\\\"")
                c == '\\' -> out.append("\\\\")
                c == '\n' -> out.append("\\n")
                c == '\r' -> out.append("\\r")
                c == '\t' -> out.append("\\t")
                c == '\b' -> out.append("\\b")
                c == '\u000c' -> out.append("\\f")
                c.code < 0x20 -> out.append(String.format("\\u%04x", c.code))
                else -> out.append(c)
            }
        }
        if (limit != null && count > limit) out.append('…')
        return out.append('"').toString()
    }

    private fun issue(path: List<Any>, reason: String) = Issue(path.toList(), "${pathText(path)} $reason")

    private val pathOrder: Comparator<List<Any>> = Comparator { a, b ->
        for (i in 0 until minOf(a.size, b.size)) {
            val x = a[i]
            val y = b[i]
            val order = when {
                x is Int && y is Int -> x.compareTo(y)
                x is Int -> 1
                y is Int -> -1
                else -> byCodePoint.compare(x as String, y as String)
            }
            if (order != 0) return@Comparator order
        }
        a.size.compareTo(b.size)
    }

    /** Problems sorted by path (stable), each once. */
    private fun sorted(issues: List<Issue>): List<Issue> = issues.sortedWith { a, b -> pathOrder.compare(a.path, b.path) }.distinct()

    /** A JSON value copied: its arrays and objects new. */
    private fun copy(value: Any?): Any? = when (value) {
        is List<*> -> value.mapTo(ArrayList()) { copy(it) }
        is Map<*, *> -> LinkedHashMap<String, Any?>().also { out -> for ((k, v) in value) out[k as String] = copy(v) }
        else -> value
    }

    /** JSON equality: numbers by value, objects regardless of key order. */
    private fun same(a: Any?, b: Any?): Boolean = when {
        a is Num && b is Num -> a.decimal.compareTo(b.decimal) == 0
        a is List<*> && b is List<*> -> a.size == b.size && a.indices.all { same(a[it], b[it]) }
        a is Map<*, *> && b is Map<*, *> -> a.size == b.size && a.all { (k, v) -> b.containsKey(k) && same(v, b[k]) }
        else -> a == b
    }

    private fun types(type: Any?): List<String> {
        val names = when (type) {
            is String -> listOf(type)
            is List<*> -> type.filterIsInstance<String>()
            else -> emptyList()
        }
        return names.filter { it in TYPES }
    }

    private fun isType(name: String, value: Any?): Boolean = when (name) {
        "string" -> value is String
        "number" -> value is Num && value.double.isFinite()
        "integer" -> value is Num && (value.isIntegerLiteral || value.double.let { it.isFinite() && it == Math.rint(it) })
        "boolean" -> value is Boolean
        "object" -> value is Map<*, *>
        "array" -> value is List<*>
        else -> value == null
    }

    private fun typeText(name: String): String = when (name) {
        "string" -> "a string"
        "number" -> "a number"
        "integer" -> "an integer"
        "boolean" -> "a boolean"
        "object" -> "an object"
        "array" -> "an array"
        else -> "null"
    }

    private fun joined(texts: List<String>): String = when (texts.size) {
        1 -> texts[0]
        2 -> "${texts[0]} or ${texts[1]}"
        else -> "one of " + texts.joinToString(", ")
    }

    private fun extra(path: List<Any>, key: String, known: List<String>): Issue {
        val like = closest(key, known)
        return issue(path, if (like == null) "is not a config key" else "is not a config key (did you mean $like?)")
    }

    private class Sink(val warn: Boolean) {
        val errors = ArrayList<Issue>()
        val warnings = ArrayList<Issue>()
    }

    /** A schema and where it is in the document (its JSON Pointer), which tells branches apart. */
    private class Node(val schema: Any?, val pointer: String)

    private class Part(val fields: Map<*, *>, val pointer: String) {
        operator fun get(key: String): Any? = fields[key]
        fun has(key: String) = fields.containsKey(key)
    }

    private sealed class Tried {
        object Trying : Tried()
        object Failed : Tried()
        class Passed(val value: Any?) : Tried()
    }

    private class Document(val root: Any?) {
        /** `#/$defs/…` or `#/definitions/…` in the document. */
        fun resolve(reference: Any?): Node? {
            if (reference !is String || !(reference.startsWith("#/\$defs/") || reference.startsWith("#/definitions/"))) return null
            var node = root
            for (raw in reference.substring(2).split("/")) {
                val key = raw.replace("~1", "/").replace("~0", "~")
                node = when {
                    node is List<*> && key.matches(Regex("0|[1-9][0-9]*")) && key.toBigInteger() < node.size.toBigInteger() -> node[key.toInt()]
                    node is Map<*, *> && node.containsKey(key) -> node[key]
                    else -> return null
                }
            }
            return Node(node, reference.substring(1))
        }

        /** [nodes] and what they take in through `$ref` and `allOf`, each once (depth-first pre-order). */
        fun parts(nodes: List<Node>): List<Part> {
            val out = ArrayList<Part>()
            val seen = HashSet<String>()
            fun collect(node: Node) {
                val fields = node.schema as? Map<*, *> ?: return
                if (!seen.add(node.pointer)) return
                val part = Part(fields, node.pointer)
                out.add(part)
                resolve(part["\$ref"])?.let(::collect)
                (part["allOf"] as? List<*>)?.forEachIndexed { index, member -> collect(Node(member, "${node.pointer}/allOf/$index")) }
            }
            nodes.forEach(::collect)
            return out
        }

        /** The first `default` of [nodes] (through `$ref` and `allOf`); [MISSING] when none. */
        fun defaultOf(nodes: List<Node>): Any? {
            val part = parts(nodes).firstOrNull { it.has("default") } ?: return MISSING
            return part["default"]
        }

        fun isObjectRoot(schema: Any?): Boolean {
            val part = parts(listOf(Node(schema, ""))).firstOrNull { it.has("type") } ?: return false
            return "object" in types(part["type"])
        }

        fun properties(part: Part): Map<*, *>? = part["properties"] as? Map<*, *>

        fun property(parts: List<Part>, key: String): List<Node> =
            parts.mapNotNull { part -> properties(part)?.takeIf { it.containsKey(key) }?.let { Node(it[key], "${part.pointer}/properties/$key") } }

        fun additional(parts: List<Part>): Node? = parts.firstOrNull { it.has("additionalProperties") }?.let { Node(it["additionalProperties"], "${it.pointer}/additionalProperties") }

        fun known(parts: List<Part>): List<String> =
            parts.flatMap { (properties(it)?.keys ?: emptySet()).map { key -> key as String } }.distinct().sortedWith(byCodePoint)

        fun items(parts: List<Part>): List<Node> = parts.mapNotNull { part -> (part["items"] as? Map<*, *>)?.let { Node(it, "${part.pointer}/items") } }

        /** Fill defaults level by level; not into anyOf / oneOf. */
        fun fill(nodes: List<Node>, value: Any?) {
            val parts = parts(nodes)
            @Suppress("UNCHECKED_CAST")
            when (value) {
                is MutableMap<*, *> -> {
                    val map = value as MutableMap<String, Any?>
                    for (part in parts) {
                        for ((key, property) in properties(part) ?: continue) {
                            key as String
                            if (map.containsKey(key)) continue
                            val fallback = defaultOf(listOf(Node(property, "${part.pointer}/properties/$key")))
                            if (fallback !== MISSING) map[key] = copy(fallback)
                        }
                    }
                    val known = known(parts)
                    val additional = additional(parts)
                    for ((key, child) in map) {
                        val below = when {
                            key in known -> property(parts, key)
                            additional != null && additional.schema is Map<*, *> -> listOf(additional)
                            else -> emptyList()
                        }
                        if (below.isNotEmpty()) fill(below, child)
                    }
                }
                is List<*> -> {
                    val below = items(parts)
                    if (below.isNotEmpty()) value.forEach { fill(below, it) }
                }
            }
        }

        /** Validate `holder[0]` at [path] against every one of [nodes]; [tried]: branches tried at this value, by pointer. */
        fun validate(nodes: List<Node>, holder: Array<Any?>, path: List<Any>, required: Boolean, sink: Sink, tried: HashMap<String, Tried>) {
            val parts = parts(nodes)
            // The type first: when it is wrong, nothing else at or below it.
            for (part in parts) {
                if (!part.has("type")) continue
                val names = types(part["type"])
                if (names.isEmpty()) continue
                if (names.none { isType(it, holder[0]) }) {
                    sink.errors.add(issue(path, "should be ${joined(names.map(::typeText))}, got ${valueText(holder[0])}"))
                    return
                }
                // An integral number passing `integer` is an integer.
                val number = holder[0]
                if ("integer" in names && number is Num && !number.isIntegerLiteral && Math.abs(number.double) <= 9007199254740992.0) {
                    holder[0] = Num(number.double.toLong().toString())
                }
            }
            for (part in parts) {
                val value = holder[0]
                val allowed = part["enum"]
                if (allowed is List<*> && allowed.none { same(it, value) }) {
                    sink.errors.add(issue(path, "should be ${joined(allowed.map(::valueText))}, got ${valueText(value)}"))
                }
                if (part.has("const") && !same(part["const"], value)) {
                    sink.errors.add(issue(path, "should be ${valueText(part["const"])}, got ${valueText(value)}"))
                }
                if (value is Num) {
                    val minimum = part["minimum"]
                    val maximum = part["maximum"]
                    if (minimum is Num && value.decimal < minimum.decimal) sink.errors.add(issue(path, "should be at least ${valueText(minimum)}, got ${valueText(value)}"))
                    if (maximum is Num && value.decimal > maximum.decimal) sink.errors.add(issue(path, "should be at most ${valueText(maximum)}, got ${valueText(value)}"))
                }
                for (keyword in listOf("anyOf", "oneOf")) {
                    val branches = part[keyword] as? List<*> ?: continue
                    var passed: Any? = MISSING
                    for ((index, branch) in branches.withIndex()) {
                        // At one value each branch is tried once: one being tried
                        // (reached again through $ref) does not match, one tried
                        // before gives its result again.
                        val pointer = "${part.pointer}/$keyword/$index"
                        val result: Any? = when (val memo = tried[pointer]) {
                            Tried.Trying, Tried.Failed -> MISSING
                            is Tried.Passed -> copy(memo.value)
                            null -> {
                                val trial = Sink(warn = false)
                                val attempt = arrayOf(copy(holder[0]))
                                tried[pointer] = Tried.Trying
                                validate(listOf(Node(branch, pointer)), attempt, path, required, trial, tried)
                                val outcome = if (trial.errors.isEmpty()) attempt[0] else MISSING
                                tried[pointer] = if (outcome === MISSING) Tried.Failed else Tried.Passed(copy(outcome))
                                outcome
                            }
                        }
                        if (result !== MISSING) {
                            passed = result
                            break
                        }
                    }
                    if (passed !== MISSING) {
                        holder[0] = passed
                    } else {
                        sink.errors.add(issue(path, "should be ${joined(branchTexts(branches, part.pointer, keyword))}, got ${valueText(holder[0])}"))
                    }
                }
            }
            @Suppress("UNCHECKED_CAST")
            when (val value = holder[0]) {
                is MutableMap<*, *> -> {
                    val map = value as MutableMap<String, Any?>
                    if (required) {
                        for (part in parts) {
                            for (key in part["required"] as? List<*> ?: continue) {
                                if (key is String && !map.containsKey(key)) sink.errors.add(issue(path + key, "is required"))
                            }
                        }
                    }
                    val known = known(parts)
                    val additional = additional(parts)
                    val described = parts.any { it.has("properties") }
                    for (key in map.keys.toList()) {
                        val at = path + key
                        val child = arrayOf(map[key])
                        when {
                            key in known -> validate(property(parts, key), child, at, required, sink, HashMap())
                            additional != null && additional.schema == false -> sink.errors.add(extra(at, key, known))
                            additional != null && additional.schema is Map<*, *> -> validate(listOf(additional), child, at, required, sink, HashMap())
                            additional == null && described && sink.warn -> sink.warnings.add(extra(at, key, known))
                        }
                        map[key] = child[0]
                    }
                }
                is MutableList<*> -> {
                    val list = value as MutableList<Any?>
                    val below = items(parts)
                    if (below.isNotEmpty()) {
                        for (index in list.indices) {
                            val child = arrayOf(list[index])
                            validate(below, child, path + index, required, sink, HashMap())
                            list[index] = child[0]
                        }
                    }
                }
                else -> {}
            }
        }

        /** How the branches of a failed anyOf / oneOf are written (§6.5). */
        fun branchTexts(branches: List<*>, pointer: String, keyword: String): List<String> {
            val texts = ArrayList<String>()
            for ((index, branch) in branches.withIndex()) {
                var found: List<String>? = null
                for (part in parts(listOf(Node(branch, "$pointer/$keyword/$index")))) {
                    if (part.has("const")) {
                        found = listOf(valueText(part["const"]))
                        break
                    }
                    val allowed = part["enum"]
                    if (allowed is List<*>) {
                        found = allowed.map(::valueText)
                        break
                    }
                    if (part.has("type") && types(part["type"]).isNotEmpty()) {
                        found = types(part["type"]).map(::typeText)
                        break
                    }
                }
                for (text in found ?: listOf("another accepted value")) if (text !in texts) texts.add(text)
            }
            return texts
        }
    }

    /** A value that is not JSON data (a date, bytes, a reference): such a config is decoded as it is. */
    class NotData : RuntimeException("not JSON data")

    /** A session value or JSON tree as this module's JSON values. */
    fun fromValue(value: Any?): Any? = when (value) {
        null, kotlinx.serialization.json.JsonNull, Undefined -> null
        is kotlinx.serialization.json.JsonPrimitive -> when {
            value.isString -> value.content
            value.content == "true" -> true
            value.content == "false" -> false
            else -> Num(value.content)
        }
        is kotlinx.serialization.json.JsonArray -> value.mapTo(ArrayList()) { fromValue(it) }
        is kotlinx.serialization.json.JsonObject -> LinkedHashMap<String, Any?>().also { out -> for ((k, v) in value) out[k] = fromValue(v) }
        is String -> value
        is Boolean -> value
        is Int, is Long, is Short, is Byte, is BigInteger -> Num(value.toString())
        is Double -> if (value.isFinite()) Num(doubleText(value)) else throw NotData()
        is Float -> if (value.isFinite()) Num(doubleText(value.toDouble())) else throw NotData()
        is BigDecimal -> Num(value.toString())
        is List<*> -> value.mapTo(ArrayList()) { fromValue(it) }
        is Map<*, *> -> LinkedHashMap<String, Any?>().also { out ->
            for ((k, v) in value) out[k as? String ?: throw NotData()] = fromValue(v)
        }
        else -> throw NotData()
    }

    /** This module's JSON values as a JSON tree. */
    fun toJson(value: Any?): kotlinx.serialization.json.JsonElement = when (value) {
        null -> kotlinx.serialization.json.JsonNull
        is Boolean -> kotlinx.serialization.json.JsonPrimitive(value)
        is String -> kotlinx.serialization.json.JsonPrimitive(value)
        is Num -> if (value.isIntegerLiteral) kotlinx.serialization.json.JsonPrimitive(value.text.toBigInteger()) else kotlinx.serialization.json.JsonPrimitive(value.double)
        is List<*> -> kotlinx.serialization.json.JsonArray(value.map(::toJson))
        is Map<*, *> -> kotlinx.serialization.json.JsonObject(value.entries.associate { (k, v) -> k as String to toJson(v) })
        else -> throw NotData()
    }
}
