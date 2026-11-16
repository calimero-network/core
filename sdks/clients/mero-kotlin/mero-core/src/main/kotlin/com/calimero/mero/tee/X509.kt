package com.calimero.mero.tee

import org.bouncycastle.asn1.x9.ECNamedCurveTable
import org.bouncycastle.crypto.digests.SHA256Digest
import org.bouncycastle.crypto.digests.SHA384Digest
import org.bouncycastle.crypto.params.ECDomainParameters
import org.bouncycastle.crypto.params.ECPublicKeyParameters
import org.bouncycastle.crypto.signers.ECDSASigner
import org.bouncycastle.util.encoders.Base64
import java.math.BigInteger
import java.util.Calendar
import java.util.GregorianCalendar
import java.util.TimeZone

/*
 * Just enough DER and X.509 to check a Sigstore signing certificate: read a certificate's
 * signed part, validity, key and extensions, and verify the ECDSA signatures (P-256 and
 * P-384) Sigstore uses. Port of mero-js `src/sealed/x509.ts`: it parses strictly and throws
 * on anything it does not expect; it is not a general X.509 implementation. The curve
 * arithmetic is Bouncy Castle's.
 */

internal const val DER_SEQUENCE = 0x30
internal const val DER_OID = 0x06
internal const val DER_INTEGER = 0x02
internal const val DER_BIT_STRING = 0x03
internal const val DER_OCTET_STRING = 0x04
internal const val DER_BOOLEAN = 0x01
private const val UTC_TIME = 0x17
private const val GENERALIZED_TIME = 0x18
private const val CONTEXT_0 = 0xa0
private const val CONTEXT_3 = 0xa3

private const val EC_PUBLIC_KEY = "1.2.840.10045.2.1"

/** The two curves Sigstore signs with. */
internal enum class Curve(
    val size: Int,
    val bcName: String,
) {
    P256(32, "P-256"),
    P384(48, "P-384"),
}

/** The two hashes a certificate here may be signed with. */
internal enum class Hash {
    SHA256,
    SHA384,
}

private val CURVES = mapOf("1.2.840.10045.3.1.7" to Curve.P256, "1.3.132.0.34" to Curve.P384)

/** The certificate signature algorithms accepted: ECDSA with SHA-256 or SHA-384. */
private val SIGNATURE_HASHES = mapOf("1.2.840.10045.4.3.2" to Hash.SHA256, "1.2.840.10045.4.3.3" to Hash.SHA384)

/** Thrown for DER the parser does not read; callers wrap it with what was being parsed. */
internal class DerException(
    message: String,
) : Exception(message)

private fun malformed(message: String): Nothing = throw DerException(message)

/** One DER element: its tag, and where it and its contents lie in [bytes]. */
internal class DerElement(
    val tag: Int,
    val bytes: ByteArray,
    /** Offset of the element's first (tag) byte. */
    val offset: Int,
    /** Offset of its contents. */
    val start: Int,
    /** Offset just past it. */
    val end: Int,
) {
    val contents: ByteArray get() = bytes.copyOfRange(start, end)

    /** The whole element, header included. */
    val whole: ByteArray get() = bytes.copyOfRange(offset, end)

    val children: List<DerElement>
        get() {
            val out = mutableListOf<DerElement>()
            var at = start
            while (at < end) {
                val child = readDer(bytes, at, end)
                out += child
                at = child.end
            }
            return out
        }
}

private fun readDer(
    bytes: ByteArray,
    offset: Int,
    limit: Int,
): DerElement {
    if (offset + 2 > limit) malformed("it is truncated")
    val tag = bytes[offset].toInt() and 0xff
    if (tag and 0x1f == 0x1f) malformed("it uses a tag this parser does not read")
    var length = bytes[offset + 1].toInt() and 0xff
    var start = offset + 2
    if (length and 0x80 != 0) {
        val count = length and 0x7f
        if (count == 0 || count > 4 || start + count > limit) malformed("it has a length this parser does not read")
        var long = 0L
        for (i in 0 until count) long = long * 256 + (bytes[start + i].toInt() and 0xff)
        if (long > Int.MAX_VALUE) malformed("it is truncated")
        length = long.toInt()
        start += count
    }
    val end = start.toLong() + length
    if (end > limit) malformed("it is truncated")
    return DerElement(tag, bytes, offset, start, end.toInt())
}

/** Parse one complete DER element, refusing trailing bytes. */
internal fun parseDer(bytes: ByteArray): DerElement {
    val element = readDer(bytes, 0, bytes.size)
    if (element.end != bytes.size) malformed("it has trailing bytes")
    return element
}

/** [element] if it has [tag]; otherwise throws naming [what]. */
internal fun expectTag(
    element: DerElement?,
    tag: Int,
    what: String,
): DerElement {
    if (element == null || element.tag != tag) malformed("$what is not where it should be")
    return element
}

internal fun oidOf(element: DerElement?): String {
    val value = expectTag(element, DER_OID, "An object identifier").contents
    if (value.isEmpty() || value.last().toInt() and 0x80 != 0) malformed("it has a malformed object identifier")
    val arcs = mutableListOf<Long>()
    var arc = 0L
    for (byte in value) {
        val b = byte.toInt() and 0xff
        arc = arc * 128 + (b and 0x7f)
        if (b and 0x80 == 0) {
            arcs += arc
            arc = 0
        }
    }
    val first = arcs.removeAt(0)
    val head = if (first < 80) listOf(first / 40, first % 40) else listOf(2L, first - 80)
    return (head + arcs).joinToString(".")
}

private val UTC_PATTERN = Regex("^(\\d{2})(\\d{2})(\\d{2})(\\d{2})(\\d{2})(\\d{2})Z$")
private val GENERALIZED_PATTERN = Regex("^(\\d{4})(\\d{2})(\\d{2})(\\d{2})(\\d{2})(\\d{2})Z$")

private fun timeOf(element: DerElement): Long {
    val text = String(element.contents, Charsets.UTF_8)
    val parts =
        when (element.tag) {
            UTC_TIME -> UTC_PATTERN.find(text)
            GENERALIZED_TIME -> GENERALIZED_PATTERN.find(text)
            else -> null
        } ?: malformed("it holds the time ${jsonString(text)}, which is not one a certificate may hold")
    val values = parts.groupValues.drop(1).map { it.toInt() }
    var year = values[0]
    if (element.tag == UTC_TIME) year += if (year < 50) 2000 else 1900
    return utcMillis(year, values[1], values[2], values[3], values[4], values[5], 0)
}

/** A parsed DER X.509 certificate with an ECDSA key, signed with ECDSA. */
@Suppress("LongParameterList")
internal class Certificate(
    val der: ByteArray,
    /** The signed part, exactly as the issuer signed it. */
    val tbs: ByteArray,
    /** The hash of the ECDSA signature the issuer made over [tbs]. */
    val signatureHash: Hash,
    /** That signature, DER. */
    val signature: ByteArray,
    /** The issuer and subject names, DER, to compare byte for byte. */
    val issuer: ByteArray,
    val subject: ByteArray,
    /** Validity, in milliseconds since the epoch, both ends inclusive. */
    val notBefore: Long,
    val notAfter: Long,
    /** The subject's key, DER SubjectPublicKeyInfo, and its curve. */
    val spki: ByteArray,
    val curve: Curve,
    /** Each extension's value (the contents of its OCTET STRING), by object identifier. */
    val extensions: Map<String, ByteArray>,
) {
    fun copy(
        notBefore: Long = this.notBefore,
        notAfter: Long = this.notAfter,
    ): Certificate = Certificate(der, tbs, signatureHash, signature, issuer, subject, notBefore, notAfter, spki, curve, extensions)
}

/** Parse a DER X.509 certificate with an ECDSA key, signed with ECDSA. Throws naming what is wrong. */
@Suppress("CyclomaticComplexMethod", "LongMethod", "NestedBlockDepth")
internal fun parseCertificate(der: ByteArray): Certificate {
    try {
        val top = expectTag(parseDer(der), DER_SEQUENCE, "The certificate").children
        if (top.size > 3) malformed("it has fields after its signature")
        val tbsElement = top.getOrNull(0)
        val algorithm = top.getOrNull(1)
        val signature = top.getOrNull(2)
        val fields = expectTag(tbsElement, DER_SEQUENCE, "The signed part").children
        // `version` is [0] EXPLICIT; everything after it is positional.
        val first = if (fields.firstOrNull()?.tag == CONTEXT_0) 1 else 0
        val positional = fields.drop(first)
        val serial = positional.getOrNull(0)
        val innerAlgorithm = positional.getOrNull(1)
        val issuer = positional.getOrNull(2)
        val validity = positional.getOrNull(3)
        val subject = positional.getOrNull(4)
        val spki = positional.getOrNull(5)
        val optional = positional.drop(6)
        expectTag(serial, DER_INTEGER, "The serial number")

        val algorithmId = oidOf(expectTag(algorithm, DER_SEQUENCE, "The signature algorithm").children.firstOrNull())
        val innerId = oidOf(expectTag(innerAlgorithm, DER_SEQUENCE, "The signature algorithm").children.firstOrNull())
        if (innerId != algorithmId) malformed("it names two different signature algorithms")
        val signatureHash =
            SIGNATURE_HASHES[algorithmId] ?: malformed("it is signed with $algorithmId, not ECDSA with SHA-256 or SHA-384")

        val times = expectTag(validity, DER_SEQUENCE, "The validity").children.map(::timeOf)
        if (times.size < 2) malformed("its validity is incomplete")

        val keyParts = expectTag(spki, DER_SEQUENCE, "The public key").children
        expectTag(keyParts.getOrNull(1), DER_BIT_STRING, "The public key")
        val keyAlgorithm = expectTag(keyParts.getOrNull(0), DER_SEQUENCE, "The key algorithm").children
        if (oidOf(keyAlgorithm.getOrNull(0)) != EC_PUBLIC_KEY) malformed("its key is not an elliptic-curve key")
        val curve = CURVES[oidOf(keyAlgorithm.getOrNull(1))] ?: malformed("its key is on a curve other than P-256 or P-384")

        val extensions = LinkedHashMap<String, ByteArray>()
        optional.firstOrNull { it.tag == CONTEXT_3 }?.let { field ->
            val list = field.children.firstOrNull()
            for (extension in expectTag(list, DER_SEQUENCE, "The extensions").children) {
                val parts = expectTag(extension, DER_SEQUENCE, "An extension").children
                if (parts.size == 3) {
                    expectTag(parts[1], DER_BOOLEAN, "An extension's critical flag")
                } else if (parts.size != 2) {
                    malformed("it has a malformed extension")
                }
                val id = oidOf(parts[0])
                if (id in extensions) malformed("it has extension $id twice")
                extensions[id] = expectTag(parts.last(), DER_OCTET_STRING, "An extension value").contents
            }
        }

        val signatureBits = expectTag(signature, DER_BIT_STRING, "The signature").contents
        if (signatureBits.isEmpty() || signatureBits[0].toInt() != 0) malformed("its signature is not a whole number of bytes")
        return Certificate(
            der = der,
            tbs = tbsElement!!.whole,
            signatureHash = signatureHash,
            signature = signatureBits.copyOfRange(1, signatureBits.size),
            issuer = expectTag(issuer, DER_SEQUENCE, "The issuer").whole,
            subject = expectTag(subject, DER_SEQUENCE, "The subject").whole,
            notBefore = times[0],
            notAfter = times[1],
            spki = spki!!.whole,
            curve = curve,
            extensions = extensions,
        )
    } catch (e: DerException) {
        throw TeeTrustException("The certificate is malformed: ${e.message}")
    }
}

/** The object identifiers in a DER `SEQUENCE OF OBJECT IDENTIFIER` (an extended key usage). */
internal fun objectIdentifiers(der: ByteArray): List<String> = expectTag(parseDer(der), DER_SEQUENCE, "The list").children.map(::oidOf)

/** `(r, s)` from the DER `SEQUENCE { r INTEGER, s INTEGER }` X.509 and cosign use, each at most the curve's size. */
internal fun ecdsaSignatureParts(
    der: ByteArray,
    curve: Curve,
): Pair<BigInteger, BigInteger> {
    val integers = expectTag(parseDer(der), DER_SEQUENCE, "The signature").children
    if (integers.size != 2) malformed("The signature is not an ECDSA signature")
    val values =
        integers.map { integer ->
            var value = expectTag(integer, DER_INTEGER, "The signature").contents
            // r and s are positive: a set top bit with no leading zero is a negative number.
            if (value.isEmpty() || value[0].toInt() and 0x80 != 0) malformed("The signature is not an ECDSA signature")
            while (value.size > 1 && value[0].toInt() == 0) value = value.copyOfRange(1, value.size)
            if (value.size > curve.size) malformed("The signature is not an ECDSA signature")
            BigInteger(1, value)
        }
    return values[0] to values[1]
}

/** The EC point of a DER SubjectPublicKeyInfo on [curve]. */
internal fun ecPublicKey(
    spki: ByteArray,
    curve: Curve,
): ECPublicKeyParameters {
    val parts = expectTag(parseDer(spki), DER_SEQUENCE, "The public key").children
    val bits = expectTag(parts.getOrNull(1), DER_BIT_STRING, "The public key").contents
    if (bits.isEmpty() || bits[0].toInt() != 0) malformed("The public key is malformed")
    val params = ECNamedCurveTable.getByName(curve.bcName)
    val domain = ECDomainParameters(params.curve, params.g, params.n, params.h)
    return ECPublicKeyParameters(params.curve.decodePoint(bits.copyOfRange(1, bits.size)), domain)
}

internal fun digest(
    hash: Hash,
    data: ByteArray,
): ByteArray {
    val d = if (hash == Hash.SHA256) SHA256Digest() else SHA384Digest()
    d.update(data, 0, data.size)
    return ByteArray(d.digestSize).also { d.doFinal(it, 0) }
}

/** Verify a DER ECDSA [signature] over [data] with a DER SubjectPublicKeyInfo key. False for any malformed input. */
internal fun verifyEcdsa(
    spki: ByteArray,
    curve: Curve,
    hash: Hash,
    signature: ByteArray,
    data: ByteArray,
): Boolean {
    val (r, s) =
        try {
            ecdsaSignatureParts(signature, curve)
        } catch (_: DerException) {
            return false
        }
    return verifyEcdsaRaw(ecPublicKey(spki, curve), hash, r, s, data)
}

/** Verify `(r, s)` over [data] with [key]. False, never a throw, for a signature that does not verify. */
internal fun verifyEcdsaRaw(
    key: ECPublicKeyParameters,
    hash: Hash,
    r: BigInteger,
    s: BigInteger,
    data: ByteArray,
): Boolean =
    try {
        ECDSASigner().run {
            init(false, key)
            verifySignature(digest(hash, data), r, s)
        }
    } catch (_: IllegalArgumentException) {
        false
    }

private val BASE64_PATTERN = Regex("^[A-Za-z0-9+/]*={0,2}$")
private val WHITESPACE = Regex("\\s+")

/** Decode base64 (standard alphabet), refusing anything else. */
internal fun strictBase64(
    value: String,
    what: String,
): ByteArray {
    val clean = value.replace(WHITESPACE, "")
    if (!BASE64_PATTERN.matches(clean) || clean.length % 4 != 0) throw TeeTrustException("$what is not base64")
    return Base64.decode(clean)
}

private val PEM_BLOCK = Regex("-----BEGIN CERTIFICATE-----([^-]*)-----END CERTIFICATE-----")

/** The DER of the one `CERTIFICATE` block a PEM holds. */
internal fun pemCertificate(
    pem: String,
    what: String,
): ByteArray {
    val blocks = PEM_BLOCK.findAll(pem).toList()
    if (blocks.size != 1 || pem.replaceFirst(blocks[0].value, "").trim().isNotEmpty()) {
        throw TeeTrustException("$what is not exactly one PEM certificate")
    }
    return strictBase64(blocks[0].groupValues[1], what)
}

/** Milliseconds since the epoch of a UTC calendar time. */
internal fun utcMillis(
    year: Int,
    month: Int,
    day: Int,
    hour: Int,
    minute: Int,
    second: Int,
    millis: Int,
): Long =
    GregorianCalendar(TimeZone.getTimeZone("UTC")).run {
        clear()
        isLenient = true
        set(year, month - 1, day, hour, minute, second)
        set(Calendar.MILLISECOND, millis)
        timeInMillis
    }

private val ISO_PATTERN = Regex("^(\\d{4})-(\\d{2})-(\\d{2})T(\\d{2}):(\\d{2}):(\\d{2})(?:\\.(\\d{1,3})\\d*)?(Z|[+-]\\d{2}:\\d{2})$")

/** `Date.parse` of an ISO-8601 instant (`2022-12-31T23:59:59.999Z`), without java.time (minSdk 24). */
internal fun parseIsoMillis(value: String): Long {
    val m = ISO_PATTERN.find(value.trim()) ?: throw TeeTrustException("${jsonString(value)} is not an ISO-8601 time")
    val g = m.groupValues
    val millis = g[7].takeIf { it.isNotEmpty() }?.padEnd(3, '0')?.toInt() ?: 0
    val base = utcMillis(g[1].toInt(), g[2].toInt(), g[3].toInt(), g[4].toInt(), g[5].toInt(), g[6].toInt(), millis)
    val zone = g[8]
    if (zone == "Z") return base
    val sign = if (zone[0] == '-') -1 else 1
    val offset = (zone.substring(1, 3).toInt() * 60 + zone.substring(4, 6).toInt()) * 60_000L
    return base - sign * offset
}

/** `new Date(ms).toISOString()`: `2026-09-28T10:29:25.000Z`. */
internal fun isoString(ms: Long): String {
    val c = GregorianCalendar(TimeZone.getTimeZone("UTC")).apply { timeInMillis = ms }
    return String.format(
        java.util.Locale.ROOT,
        "%04d-%02d-%02dT%02d:%02d:%02d.%03dZ",
        c.get(Calendar.YEAR),
        c.get(Calendar.MONTH) + 1,
        c.get(Calendar.DAY_OF_MONTH),
        c.get(Calendar.HOUR_OF_DAY),
        c.get(Calendar.MINUTE),
        c.get(Calendar.SECOND),
        c.get(Calendar.MILLISECOND),
    )
}

/** `JSON.stringify` of a string, for messages that quote what they refuse. */
internal fun jsonString(value: String): String =
    kotlinx.serialization.json
        .JsonPrimitive(value)
        .toString()
