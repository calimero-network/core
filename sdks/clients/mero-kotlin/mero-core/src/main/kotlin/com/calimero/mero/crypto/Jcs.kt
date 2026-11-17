package com.calimero.mero.crypto

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import java.math.BigDecimal
import java.math.MathContext
import java.math.RoundingMode

/**
 * RFC 8785 (JCS) canonical JSON, as mero-js's `jcs(value, true)` writes it and core's
 * canonicalizer reads it: keys sorted by UTF-16 code unit, strings as `JSON.stringify` writes
 * them, and every number converted to an f64 first and written as ECMAScript's `Number::toString`
 * does. A lone surrogate or a number that is not finite cannot be canonicalized.
 */
object Jcs {
    private val JSON_NUMBER = Regex("-?(0|[1-9][0-9]*)(\\.[0-9]+)?([eE][+-]?[0-9]+)?")

    fun canonicalize(value: JsonElement): String = StringBuilder().also { write(value, it) }.toString()

    private fun write(
        value: JsonElement,
        out: StringBuilder,
    ) {
        when (value) {
            JsonNull -> out.append("null")
            is JsonPrimitive ->
                when {
                    value.isString -> string(value.content, out)
                    value.content == "true" || value.content == "false" -> out.append(value.content)
                    else -> out.append(number(value.content))
                }
            is JsonArray -> {
                out.append('[')
                value.forEachIndexed { i, item ->
                    if (i > 0) out.append(',')
                    write(item, out)
                }
                out.append(']')
            }
            is JsonObject -> {
                out.append('{')
                value.keys.sorted().forEachIndexed { i, key ->
                    if (i > 0) out.append(',')
                    string(key, out)
                    out.append(':')
                    write(value.getValue(key), out)
                }
                out.append('}')
            }
        }
    }

    private fun string(
        text: String,
        out: StringBuilder,
    ) {
        out.append('"')
        var i = 0
        while (i < text.length) {
            val c = text[i]
            when {
                c == '"' -> out.append("\\\"")
                c == '\\' -> out.append("\\\\")
                c == '\b' -> out.append("\\b")
                c == '\u000c' -> out.append("\\f")
                c == '\n' -> out.append("\\n")
                c == '\r' -> out.append("\\r")
                c == '\t' -> out.append("\\t")
                c < ' ' -> out.append("\\u").append(String.format(java.util.Locale.ROOT, "%04x", c.code))
                c.isHighSurrogate() -> {
                    require(i + 1 < text.length && text[i + 1].isLowSurrogate()) { "a lone surrogate cannot be canonicalized" }
                    out.append(c).append(text[i + 1])
                    i += 1
                }
                c.isLowSurrogate() -> throw IllegalArgumentException("a lone surrogate cannot be canonicalized")
                else -> out.append(c)
            }
            i += 1
        }
        out.append('"')
    }

    /** A JSON number literal as ECMAScript writes the f64 it parses to. */
    fun number(literal: String): String {
        require(JSON_NUMBER.matches(literal)) { "$literal is not a JSON value" }
        val value = literal.toDouble()
        require(value.isFinite()) { "a non-finite number cannot be canonicalized" }
        return ecmaScript(value)
    }

    /** ECMAScript's `Number::toString` of [value] (finite): the shortest digits that read back as it. */
    @Suppress("MagicNumber", "ReturnCount")
    fun ecmaScript(value: Double): String {
        if (value == 0.0) return "0" // -0 too
        if (value < 0) return "-" + ecmaScript(-value)
        val exact = BigDecimal(value)
        var shortest = exact
        for (precision in 1..17) {
            val rounded = exact.round(MathContext(precision, RoundingMode.HALF_EVEN))
            if (rounded.toDouble() == value) {
                shortest = rounded
                break
            }
        }
        val stripped = shortest.stripTrailingZeros()
        val digits = stripped.unscaledValue().toString()
        val k = digits.length
        val n = k - stripped.scale() // value = 0.digits × 10^n
        return when {
            n in k..21 -> digits + "0".repeat(n - k)
            n in 1..21 -> digits.substring(0, n) + "." + digits.substring(n)
            n in -5..0 -> "0." + "0".repeat(-n) + digits
            else -> {
                val e = n - 1
                val sign = if (e < 0) "-" else "+"
                val mantissa = if (k == 1) digits else digits.substring(0, 1) + "." + digits.substring(1)
                "${mantissa}e$sign${kotlin.math.abs(e)}"
            }
        }
    }
}
