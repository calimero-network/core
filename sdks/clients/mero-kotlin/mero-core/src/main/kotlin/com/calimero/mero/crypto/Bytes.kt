package com.calimero.mero.crypto

import java.io.ByteArrayOutputStream
import java.security.MessageDigest

/**
 * Byte-level primitives every signer in the account layer shares.
 *
 * Port of mero-js `src/crypto/internal.ts`. These have to stay byte-identical with
 * core: a slip here does not fail at the edge, it fails as a node refusing a
 * signature, far from the code that produced it. One copy, shared by every signer,
 * is the point.
 */
object Hex {
    private const val DIGITS = "0123456789abcdef"

    /** Lowercase hex, two characters per byte. */
    fun encode(bytes: ByteArray): String {
        val out = StringBuilder(bytes.size * 2)
        for (b in bytes) {
            val v = b.toInt() and 0xFF
            out.append(DIGITS[v ushr 4]).append(DIGITS[v and 0x0F])
        }
        return out.toString()
    }

    /**
     * Decode hex of a known width. Every key and digest in the protocol has one, and
     * checking it catches a truncated paste here rather than deep inside a signature
     * check.
     */
    fun decode(
        value: String,
        label: String,
        expectedBytes: Int,
    ): ByteArray {
        val clean = value.trim()
        require(isHex(clean) && clean.length == expectedBytes * 2) {
            "$label must be ${expectedBytes * 2} hex characters, got ${clean.length}"
        }
        return decodeChecked(clean)
    }

    /** Decode hex of a variable length (an already-encoded structure). Still refuses odd and non-hex input. */
    fun decodeUnsized(
        value: String,
        label: String,
    ): ByteArray {
        val clean = value.trim()
        require(isHex(clean) && clean.length % 2 == 0) { "$label must be an even number of hex characters" }
        return decodeChecked(clean)
    }

    /** True for exactly 64 lowercase hex characters: the spelling of every 32-byte id here. */
    fun isHex32(value: String?): Boolean = value != null && value.length == 64 && value.all { it in DIGITS }

    private fun isHex(value: String): Boolean = value.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' }

    private fun decodeChecked(clean: String): ByteArray =
        ByteArray(clean.length / 2) { i ->
            ((Character.digit(clean[i * 2], 16) shl 4) or Character.digit(clean[i * 2 + 1], 16)).toByte()
        }
}

/** Little-endian u32, for borsh fixed-width fields, lengths and epochs. */
fun u32le(value: Long): ByteArray {
    require(value in 0..0xFFFF_FFFFL) { "u32 out of range: $value" }
    return ByteArray(4) { i -> ((value ushr (8 * i)) and 0xFF).toByte() }
}

/** Little-endian u32. */
fun u32le(value: Int): ByteArray = u32le(value.toLong())

/** Little-endian u64. Unsigned on purpose: nonces live past `Long.MAX_VALUE` in principle. */
fun u64le(value: ULong): ByteArray = ByteArray(8) { i -> ((value shr (8 * i)) and 0xFFu).toByte() }

/** Little-endian u64 of a non-negative [Long]. */
fun u64le(value: Long): ByteArray {
    require(value >= 0) { "u64 cannot be negative: $value" }
    return u64le(value.toULong())
}

/** Read a little-endian u32 at [offset]. */
fun readU32le(
    bytes: ByteArray,
    offset: Int,
): Long {
    var v = 0L
    for (i in 3 downTo 0) v = (v shl 8) or (bytes[offset + i].toLong() and 0xFF)
    return v
}

/** Read a little-endian u64 at [offset]. */
fun readU64le(
    bytes: ByteArray,
    offset: Int,
): ULong {
    var v = 0uL
    for (i in 7 downTo 0) v = (v shl 8) or (bytes[offset + i].toULong() and 0xFFu)
    return v
}

/** Concatenate byte arrays in order. */
fun concat(vararg parts: ByteArray): ByteArray {
    val out = ByteArrayOutputStream(parts.sumOf { it.size })
    for (p in parts) out.write(p)
    return out.toByteArray()
}

/** SHA-256. Available on every Android API level through the platform JCA. */
fun sha256(bytes: ByteArray): ByteArray = MessageDigest.getInstance("SHA-256").digest(bytes)

/**
 * core's `domain_hash`: SHA-256 over the u64-LE length-prefixed domain, then each
 * u64-LE length-prefixed part. The lengths are what stop two different field splits
 * from hashing alike.
 *
 * (`crates/primitives/src/identity.rs`, mero-js `domainHash`.)
 */
fun domainHash(
    domain: ByteArray,
    parts: List<ByteArray>,
): ByteArray {
    val digest = MessageDigest.getInstance("SHA-256")
    digest.update(u64le(domain.size.toLong()))
    digest.update(domain)
    for (part in parts) {
        digest.update(u64le(part.size.toLong()))
        digest.update(part)
    }
    return digest.digest()
}

/** [domainHash] with a UTF-8 domain string. */
fun domainHash(
    domain: String,
    parts: List<ByteArray>,
): ByteArray = domainHash(domain.encodeToByteArray(), parts)

/**
 * A minimal borsh writer: fixed-width integers little-endian, a `String` / `Vec<u8>`
 * as a u32 length then the bytes, an `Option` as a 0/1 tag. Enough for every
 * structure the account layer encodes; everything else is concatenation.
 */
class BorshWriter {
    private val out = ByteArrayOutputStream()

    fun u8(value: Int): BorshWriter = apply { out.write(value and 0xFF) }

    fun u32(value: Long): BorshWriter = apply { out.write(u32le(value)) }

    fun u32(value: Int): BorshWriter = u32(value.toLong())

    fun u64(value: ULong): BorshWriter = apply { out.write(u64le(value)) }

    fun u64(value: Long): BorshWriter = apply { out.write(u64le(value)) }

    /** Raw bytes, no length: a fixed-width array such as `[u8; 32]`. */
    fun raw(bytes: ByteArray): BorshWriter = apply { out.write(bytes) }

    /** A borsh `Vec<u8>`: u32 length, then the bytes. */
    fun bytes(bytes: ByteArray): BorshWriter = apply { u32(bytes.size).raw(bytes) }

    /** A borsh `String`: u32 byte length, then the UTF-8. */
    fun string(value: String): BorshWriter = bytes(value.encodeToByteArray())

    /** A borsh `Option<T>`: tag 0, or tag 1 then [write]. */
    fun <T> option(
        value: T?,
        write: BorshWriter.(T) -> Unit,
    ): BorshWriter =
        apply {
            if (value == null) {
                u8(0)
            } else {
                u8(1)
                write(value)
            }
        }

    fun toByteArray(): ByteArray = out.toByteArray()
}
