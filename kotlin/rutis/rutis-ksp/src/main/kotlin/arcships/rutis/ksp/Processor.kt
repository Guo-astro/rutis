package arcships.rutis.ksp

import com.google.devtools.ksp.isAbstract
import com.google.devtools.ksp.isPublic
import com.google.devtools.ksp.processing.CodeGenerator
import com.google.devtools.ksp.processing.Dependencies
import com.google.devtools.ksp.processing.KSPLogger
import com.google.devtools.ksp.processing.Resolver
import com.google.devtools.ksp.processing.SymbolProcessor
import com.google.devtools.ksp.processing.SymbolProcessorEnvironment
import com.google.devtools.ksp.processing.SymbolProcessorProvider
import com.google.devtools.ksp.symbol.ClassKind
import com.google.devtools.ksp.symbol.KSAnnotated
import com.google.devtools.ksp.symbol.KSClassDeclaration
import com.google.devtools.ksp.symbol.KSDeclaration
import com.google.devtools.ksp.symbol.KSFunctionDeclaration
import com.google.devtools.ksp.symbol.KSType
import com.google.devtools.ksp.symbol.KSTypeAlias
import com.google.devtools.ksp.symbol.KSTypeParameter
import com.google.devtools.ksp.symbol.Modifier
import com.google.devtools.ksp.symbol.Variance

/**
 * The rutis processor: for each `@RutisService` class a method table
 * (`<Class>_RutisService`), for each `@RutisClient` interface a proxy
 * (`<Interface>_RutisClient`), so that no method is found by reflection at
 * run time (design K6, §3.1). Values are encoded and decoded by RutisFormat
 * with the serializers the compiler chooses for each parameter and result.
 */
class RutisProcessorProvider : SymbolProcessorProvider {
    override fun create(environment: SymbolProcessorEnvironment): SymbolProcessor =
        RutisProcessor(environment.codeGenerator, environment.logger)
}

private const val SERVICE = "arcships.rutis.RutisService"
private const val CLIENT = "arcships.rutis.RutisClient"
private const val G = "arcships.rutis.Generated"
private const val FORMAT = "arcships.rutis.RutisFormat"

/** `Any` methods, never service methods. */
private val anyMethods = setOf("equals", "hashCode", "toString")

class RutisProcessor(private val generator: CodeGenerator, private val logger: KSPLogger) : SymbolProcessor {
    override fun process(resolver: Resolver): List<KSAnnotated> {
        for (symbol in resolver.getSymbolsWithAnnotation(SERVICE)) {
            val type = symbol as? KSClassDeclaration
            if (type == null || type.classKind != ClassKind.CLASS) {
                logger.error("@RutisService marks a class", symbol)
                continue
            }
            service(type)
        }
        for (symbol in resolver.getSymbolsWithAnnotation(CLIENT)) {
            val type = symbol as? KSClassDeclaration
            if (type == null || type.classKind != ClassKind.INTERFACE) {
                logger.error("@RutisClient marks an interface", symbol)
                continue
            }
            client(type)
        }
        return emptyList()
    }

    /** The generated class's simple name: the nesting of `type` joined by `_`, then `suffix`. */
    private fun generatedName(type: KSClassDeclaration, suffix: String): String {
        val names = ArrayList<String>()
        var current: KSDeclaration? = type
        while (current is KSClassDeclaration) {
            names.add(0, current.simpleName.asString())
            current = current.parentDeclaration
        }
        return names.joinToString("_") + suffix
    }

    private fun visibility(type: KSClassDeclaration): String =
        if (Modifier.INTERNAL in type.modifiers || type.parentDeclaration?.modifiers?.contains(Modifier.INTERNAL) == true) "internal " else ""

    /** The methods of a service: public member functions, its superclasses' included, by name. */
    private fun methods(type: KSClassDeclaration): List<KSFunctionDeclaration>? {
        val byName = LinkedHashMap<String, KSFunctionDeclaration>()
        var ok = true
        for (function in type.getAllFunctions()) {
            val name = function.simpleName.asString()
            if (name in anyMethods || name == "<init>" || !function.isPublic()) continue
            if (function.extensionReceiver != null) continue
            if (function.typeParameters.isNotEmpty()) {
                logger.error("rutis: ${type.simpleName.asString()}.$name: a service method cannot be generic", function)
                ok = false
                continue
            }
            val other = byName[name]
            if (other != null) {
                logger.error("rutis: ${type.simpleName.asString()}.$name is overloaded: a method's name is its name on the wire", function)
                ok = false
                continue
            }
            byName[name] = function
        }
        return if (ok) byName.values.toList() else null
    }

    // ── Types ────────────────────────────────────────────────────

    private fun expand(type: KSType): KSType {
        var current = type
        while (true) {
            val alias = current.declaration as? KSTypeAlias ?: return current
            val expanded = alias.type.resolve()
            current = if (current.isMarkedNullable) expanded.makeNullable() else expanded
        }
    }

    private fun isFunction(type: KSType): Boolean {
        val t = expand(type)
        return t.isFunctionType || t.isSuspendFunctionType
    }

    private fun isAny(type: KSType): Boolean = expand(type).declaration.qualifiedName?.asString() == "kotlin.Any"

    /** A plugin definition: it crosses as a `plugin` reference, not by a serializer. */
    private fun isPlugin(type: KSType): Boolean = expand(type).declaration.qualifiedName?.asString() == "arcships.rutis.Plugin"

    /** A `@RutisClient` interface: it takes an object of another process, as its proxy (#687 §四). */
    private fun isClient(type: KSType): Boolean {
        val declaration = expand(type).declaration as? KSClassDeclaration ?: return false
        return declaration.classKind == ClassKind.INTERFACE &&
            declaration.annotations.any { it.annotationType.resolve().declaration.qualifiedName?.asString() == CLIENT }
    }

    /** `type`'s class, without nullability, as Kotlin source. */
    private fun classOf(type: KSType): String = render(expand(type).makeNotNullable()) + "::class.java"

    private fun isUnit(type: KSType): Boolean = expand(type).declaration.qualifiedName?.asString() == "kotlin.Unit" && !type.isMarkedNullable

    /** `type` as Kotlin source. */
    private fun render(type: KSType): String {
        val t = expand(type)
        val nullable = if (t.isMarkedNullable) "?" else ""
        if (t.isFunctionType || t.isSuspendFunctionType) {
            val arguments = t.arguments.map { it.type?.resolve()?.let(::render) ?: "Any?" }
            val parameters = arguments.dropLast(1)
            val result = arguments.last()
            val suspend = if (t.isSuspendFunctionType) "suspend " else ""
            val text = "$suspend(${parameters.joinToString(", ")}) -> $result"
            return if (nullable.isEmpty()) text else "($text)?"
        }
        val declaration = t.declaration
        if (declaration is KSTypeParameter) return declaration.name.asString() + nullable
        val name = declaration.qualifiedName?.asString() ?: declaration.simpleName.asString()
        val arguments = if (t.arguments.isEmpty()) "" else t.arguments.joinToString(", ", "<", ">") { argument ->
            val inner = argument.type?.resolve()
            when {
                argument.variance == Variance.STAR || inner == null -> "*"
                argument.variance == Variance.COVARIANT -> "out " + render(inner)
                argument.variance == Variance.CONTRAVARIANT -> "in " + render(inner)
                else -> render(inner)
            }
        }
        return name + arguments + nullable
    }

    private fun serializer(type: KSType): String = "$FORMAT.serializer<${render(type)}>()"

    /** An expression of a session value `value` as `type`. */
    private fun decode(type: KSType, value: String): String = when {
        isUnit(type) -> "Unit"
        isAny(type) -> if (type.isMarkedNullable) "$G.toNative($value)" else "($G.toNative($value) ?: throw IllegalArgumentException(\"null where a value was expected\"))"
        isFunction(type) -> remoteFunction(type, "$G.remoteResult($value)")
        isPlugin(type) -> if (type.isMarkedNullable) "$G.pluginResult($value, true)" else "$G.pluginResult($value, false)!!"
        isClient(type) -> if (type.isMarkedNullable) "$G.clientResult($value, ${classOf(type)}, true)" else "$G.clientResult($value, ${classOf(type)}, false)!!"
        else -> "$G.decode(${serializer(type)}, $value)"
    }

    /** An expression of the Kotlin value `value` of `type` as a session value. */
    private fun encode(type: KSType, value: String): String = when {
        isUnit(type) -> "null"
        isAny(type) -> "$G.nativeResult($value)"
        isFunction(type) -> if (type.isMarkedNullable) "$value?.let { f -> ${exportFunction(type.makeNotNullable(), "f")} }" else exportFunction(type, value)
        isPlugin(type) -> "$G.plugin($value)"
        isClient(type) -> "$G.clientValue($value)"
        else -> "$G.encode(${serializer(type)}, $value)"
    }

    /** The parameter and result types of a function type. */
    private fun signature(type: KSType): Pair<List<KSType>, KSType> {
        val t = expand(type)
        val arguments = t.arguments.map { it.type!!.resolve() }
        return arguments.dropLast(1) to arguments.last()
    }

    /** A Kotlin function of `type` exported: the far end calls it with session values. */
    private fun exportFunction(type: KSType, value: String): String {
        val (parameters, result) = signature(type)
        val args = parameters.indices.joinToString(", ") { i -> decode(parameters[i], "(if ($i < a.size) a[$i] else null)") }
        return "$G.export($value) { a -> ${encode(result, "$value($args)")} }"
    }

    /** A function of the far end (`function`, an expression of RemoteFunction) as a Kotlin function of `type`. */
    private fun remoteFunction(type: KSType, function: String): String {
        val t = expand(type)
        val (parameters, result) = signature(t)
        val names = parameters.indices.map { "p$it" }
        val params = parameters.indices.joinToString(", ") { "p$it: ${render(parameters[it])}" }
        val encoded = parameters.indices.joinToString(", ") { encode(parameters[it], names[it]) }
        val call = if (t.isSuspendFunctionType) "$G.callSuspend(f, listOf($encoded))" else "$G.callBlocking(f, listOf($encoded))"
        val body = if (isUnit(result)) "$call; Unit" else decode(result, call)
        val lambdaType = render(t.makeNotNullable())
        return "$function.let { f -> val fn: $lambdaType = { $params -> $body }; fn }"
    }

    // ── Services ─────────────────────────────────────────────────

    private fun service(type: KSClassDeclaration) {
        val methods = methods(type) ?: return
        val pkg = type.packageName.asString()
        val name = generatedName(type, "_RutisService")
        val target = type.qualifiedName!!.asString()
        val out = StringBuilder()
        if (pkg.isNotEmpty()) out.append("package $pkg\n\n")
        out.append("// Generated by rutis-ksp: the method table of $target.\n")
        out.append("@Suppress(\"UNCHECKED_CAST\", \"RedundantSuppression\", \"UNUSED_ANONYMOUS_PARAMETER\", \"USELESS_CAST\", \"unused\")\n")
        out.append("${visibility(type)}class $name : arcships.rutis.ServiceTable<$target>() {\n")
        out.append("    override val methods: List<String> = listOf(${methods.joinToString(", ") { "\"${it.simpleName.asString()}\"" }})\n\n")
        out.append("    override suspend fun invoke(target: $target, method: String, args: List<Any?>): Any? = when (method) {\n")
        for (function in methods) {
            val method = function.simpleName.asString()
            val arguments = function.parameters.mapIndexed { i, parameter ->
                val type = parameter.type.resolve()
                when {
                    parameter.isVararg -> {
                        logger.error("rutis: $target.$method: a service method cannot take varargs", parameter)
                        "TODO()"
                    }
                    isAny(type) -> if (type.isMarkedNullable) "$G.native(args, $i)" else "$G.nativeArg(args, $i, \"$method\")"
                    isFunction(type) -> {
                        val local = "$G.local<${render(type.makeNotNullable())}>(args, $i)"
                        val remote = remoteFunction(type, "$G.remote(args, $i, \"$method\")")
                        if (type.isMarkedNullable) "($local ?: if ($i < args.size && args[$i] is arcships.rutis.RemoteFunction) $remote else null)" else "($local ?: $remote)"
                    }
                    isPlugin(type) -> if (type.isMarkedNullable) "$G.pluginArg(args, $i, \"$method\", true)" else "$G.pluginArg(args, $i, \"$method\", false)!!"
                    isClient(type) -> if (type.isMarkedNullable) "$G.clientArg(args, $i, \"$method\", ${classOf(type)}, true)" else "$G.clientArg(args, $i, \"$method\", ${classOf(type)}, false)!!"
                    else -> "$G.arg(args, $i, ${serializer(type)}, \"$method\")"
                }
            }
            val call = "target.$method(${arguments.joinToString(", ")})"
            val result = function.returnType?.resolve()
            val encoded = if (result == null) "run { $call; null }" else if (isUnit(result)) "run { $call; null }" else encode(result, call)
            out.append("        \"$method\" -> $encoded\n")
        }
        out.append("        else -> throw arcships.rutis.RutisException(\"Error\", \"no method \$method\")\n")
        out.append("    }\n}\n")
        write(type, pkg, name, out.toString())
    }

    // ── Clients ──────────────────────────────────────────────────

    private fun client(type: KSClassDeclaration) {
        val pkg = type.packageName.asString()
        val name = generatedName(type, "_RutisClient")
        val target = type.qualifiedName!!.asString()
        if (type.typeParameters.isNotEmpty()) {
            logger.error("rutis: a @RutisClient interface cannot be generic", type)
            return
        }
        for (property in type.getAllProperties()) {
            if (property.isAbstract()) {
                logger.error("rutis: $target.${property.simpleName.asString()}: a @RutisClient interface has functions only", property)
                return
            }
        }
        val functions = type.getAllFunctions().filter { it.isAbstract && it.simpleName.asString() !in anyMethods }.toList()
        val seen = HashSet<String>()
        for (function in functions) {
            if (!seen.add(function.simpleName.asString())) {
                logger.error("rutis: $target.${function.simpleName.asString()} is overloaded: a method's name is its name on the wire", function)
                return
            }
        }
        val out = StringBuilder()
        if (pkg.isNotEmpty()) out.append("package $pkg\n\n")
        out.append("// Generated by rutis-ksp: the proxy of $target.\n")
        out.append("@Suppress(\"UNCHECKED_CAST\", \"RedundantSuppression\", \"UNUSED_ANONYMOUS_PARAMETER\", \"USELESS_CAST\", \"unused\")\n")
        out.append("${visibility(type)}class $name : arcships.rutis.ClientFactory<$target>() {\n")
        out.append("    override val methods: List<String> = listOf(${functions.joinToString(", ") { "\"${it.simpleName.asString()}\"" }})\n\n")
        // The proxy is also an ObjectProxy: handed on, a proxy of another
        // process's object is that object (#687).
        out.append("    override fun create(handle: arcships.rutis.ServiceHandle): $target = object : $target, arcships.rutis.ObjectProxy {\n")
        out.append("        override val rutisHandle: arcships.rutis.ServiceHandle = handle\n")
        for (function in functions) {
            val method = function.simpleName.asString()
            if (function.typeParameters.isNotEmpty()) {
                logger.error("rutis: $target.$method: a client method cannot be generic", function)
                return
            }
            val suspend = Modifier.SUSPEND in function.modifiers
            val params = function.parameters.joinToString(", ") { "${it.name!!.asString()}: ${render(it.type.resolve())}" }
            val encoded = function.parameters.joinToString(", ") { encode(it.type.resolve(), it.name!!.asString()) }
            val call = if (suspend) "handle.callSuspend(\"$method\", listOf($encoded))" else "handle.callBlocking(\"$method\", listOf($encoded))"
            val result = function.returnType!!.resolve()
            val returns = if (isUnit(result)) "" else ": ${render(result)}"
            val body = if (isUnit(result)) "{ $call }" else "= ${decode(result, call)}"
            out.append("        override ${if (suspend) "suspend " else ""}fun $method($params)$returns $body\n")
        }
        out.append("    }\n}\n")
        write(type, pkg, name, out.toString())
    }

    private fun write(type: KSClassDeclaration, pkg: String, name: String, source: String) {
        val file = type.containingFile
        val dependencies = if (file != null) Dependencies(false, file) else Dependencies(false)
        generator.createNewFile(dependencies, pkg, name).bufferedWriter().use { it.write(source) }
    }
}
