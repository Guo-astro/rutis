package arcships.rutis.internal.peer

import java.math.BigDecimal
import java.math.BigInteger

/** Reading JSON number text. */
object JsonValues {
    /** The integer a JSON number is, when it is one (`2`, `-3`, `2.0`, `1e3`); null otherwise. */
    fun integer(text: String): BigInteger? {
        if (text.none { it == '.' || it == 'e' || it == 'E' }) return text.toBigIntegerOrNull()
        val decimal = text.toBigDecimalOrNull() ?: return null
        return try {
            decimal.toBigIntegerExact()
        } catch (_: ArithmeticException) {
            null
        }
    }

    /** A JSON number as a native Kotlin number: a Long when it is an integer literal that fits, else a Double. */
    fun number(text: String): Any {
        if (text.none { it == '.' || it == 'e' || it == 'E' }) {
            text.toLongOrNull()?.let { return it }
            text.toBigIntegerOrNull()?.let { return it }
        }
        return text.toDoubleOrNull() ?: BigDecimal(text).toDouble()
    }
}
