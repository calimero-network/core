package com.calimero.mero.tee

import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.sha256
import com.calimero.mero.http.MeroException
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.longOrNull

/**
 * A TEE trust check said no: a signature, certificate, release, quote or binding that does
 * not hold. The message names what failed. Retrying cannot change the answer. (Swift
 * `TeeTrustError`; mero-js throws a plain `Error` with the same messages.)
 */
class TeeTrustException(
    message: String,
    cause: Throwable? = null,
) : MeroException(message, cause)

internal fun refuse(message: String): Nothing = throw TeeTrustException(message)

/** Who must have signed: the certificate's SAN URI and its OIDC issuer, each matched exactly. */
data class SignerIdentity(
    val identity: String,
    val issuer: String,
)

private const val SUBJECT_ALT_NAME = "2.5.29.17"
private const val EXTENDED_KEY_USAGE = "2.5.29.37"
private const val CODE_SIGNING = "1.3.6.1.5.5.7.3.3"

/** Fulcio's OIDC issuer extension, a DER UTF8String. */
private const val FULCIO_ISSUER_V2 = "1.3.6.1.4.1.57264.1.8"

/** Its deprecated predecessor, the raw issuer bytes; read only when the other is absent. */
private const val FULCIO_ISSUER_V1 = "1.3.6.1.4.1.57264.1.1"
private const val SAN_URI = 0x86
private const val UTF8_STRING = 0x0c
private const val MAX_SAFE_INTEGER = 9_007_199_254_740_991L

private val bundleJson = Json { ignoreUnknownKeys = true }
private val LOG_ID = Regex("^[0-9a-fA-F]{64}$")

/** A legacy cosign bundle (`cosign sign-blob --bundle`), as the fields verified here. */
private class CosignBundle(
    val base64Signature: String,
    val cert: String,
    val signedEntryTimestamp: String,
    val body: String,
    val integratedTime: Long,
    val logIndex: Long,
    val logId: String,
)

/**
 * Verifying a cosign keyless signature, as `cosign verify-blob --bundle` does, against the
 * Sigstore trust root embedded in this SDK ([FULCIO_AUTHORITIES], [REKOR_LOGS]). Port of
 * mero-js `src/sealed/sigstore.ts`; the same checks core runs in
 * `crates/tee-release/src/sigstore_verify.rs`.
 *
 * mero-tee signs each release asset with cosign keyless: there is no release key, only a
 * Fulcio certificate that lives ten minutes, issued to one GitHub Actions workflow run, and a
 * Rekor log entry that proves the signature was made while the certificate was valid.
 *
 * Nothing here reads the clock. Every time is Rekor's `integratedTime`, which its signature
 * covers, so a verification that passes once passes forever, whatever the device's clock says.
 */
object Sigstore {
    /**
     * Verify a cosign keyless [bundle] (JSON text or a parsed [JsonElement]) over [body],
     * signed by [signer]. Returns the time Rekor logged it (seconds); throws
     * [TeeTrustException] with the reason otherwise. [signature], when given, is the detached
     * `.sig` published beside the file, and must be the bundle's.
     */
    fun verifyCosignBundle(
        body: ByteArray,
        bundle: Any,
        signer: SignerIdentity,
        signature: String? = null,
    ): Long {
        val b = cosignBundle(bundle)
        if (signature != null && signature.trim() != b.base64Signature.trim()) {
            refuse("The detached signature is not the one in the bundle")
        }
        val sig = strictBase64(b.base64Signature, "The bundle signature")
        val leafDer = pemCertificate(utf8(strictBase64(b.cert, "The bundle certificate")), "The bundle certificate")
        val leaf = parseCertificate(leafDer)

        // Rekor vouches for the entry and its time: nothing below is trusted before this.
        val log =
            REKOR_LOGS.firstOrNull { it.logId == b.logId.lowercase() }
                ?: refuse("The bundle names Rekor log ${b.logId}, which the Sigstore trust root does not know")
        val canonical =
            (
                "{\"body\":${jsonString(b.body)},\"integratedTime\":${b.integratedTime}," +
                    "\"logID\":${jsonString(b.logId)},\"logIndex\":${b.logIndex}}"
            ).encodeToByteArray()
        val setVerifies =
            verifyEcdsa(
                strictBase64(log.publicKey, "The Rekor key"),
                Curve.P256,
                Hash.SHA256,
                strictBase64(b.signedEntryTimestamp, "The signed entry timestamp"),
                canonical,
            )
        if (!setVerifies) {
            refuse("Rekor's signed entry timestamp does not verify: the log entry, its time or its index was altered")
        }
        val at = b.integratedTime * 1000
        if (!log.validFor.covers(at)) refuse("Rekor log ${log.baseUrl} was not in use at ${isoString(at)}, when the entry says it was logged")

        // The entry Rekor signed is this signature, over these bytes, by this certificate.
        val entry = hashedRekord(b.body)
        if (entry.hash != Hex.encode(sha256(body))) {
            refuse("The file's SHA-256 is not the one Rekor logged: the file is not the one signed")
        }
        if (!strictBase64(entry.signature, "The logged signature").contentEquals(sig)) {
            refuse("The bundle's signature is not the one Rekor logged")
        }
        val loggedCert = pemCertificate(utf8(strictBase64(entry.publicKey, "The logged certificate")), "The logged certificate")
        if (!loggedCert.contentEquals(leafDer)) refuse("The bundle's certificate is not the one Rekor logged")

        if (leaf.curve != Curve.P256 || !verifyEcdsa(leaf.spki, Curve.P256, Hash.SHA256, sig, body)) {
            refuse("The signature over the file does not verify with the bundle's certificate")
        }

        chainsToFulcio(leaf, at)
        checkSigner(leaf, signer)
        return b.integratedTime
    }

    /**
     * Check that [leaf] was issued by a Fulcio CA in the trust root, and that it, the CA's
     * certificates and the CA itself (by the trust root's `validFor`) were all valid at [at]
     * (milliseconds): when Rekor logged the signature, not now, since a Fulcio certificate
     * lives ten minutes. Every signature on the way is checked, with the algorithm its
     * certificate names.
     */
    internal fun chainsToFulcio(
        leaf: Certificate,
        at: Long,
    ) {
        if (!validAt(leaf, at)) {
            refuse(
                "The signing certificate was valid ${isoString(leaf.notBefore)} to ${isoString(leaf.notAfter)}, " +
                    "not at ${isoString(at)} when Rekor logged it",
            )
        }
        val reasons = mutableListOf<String>()
        for (authority in FULCIO_AUTHORITIES) {
            val chain = authority.certChain.map { parseCertificate(strictBase64(it, "A Fulcio certificate")) }
            if (!issuedBy(leaf, chain[0])) continue
            // The issuer is found; from here a failure is this CA's, so say which.
            val reason = authorityRefusal(chain, authority.validFor, at) ?: return
            reasons += "${authority.uri} (valid from ${authority.validFor.start}): $reason"
        }
        refuse(
            if (reasons.isNotEmpty()) {
                "The signing certificate's CA is not trusted at ${isoString(at)}: ${reasons.joinToString("; ")}"
            } else {
                "The signing certificate was not issued by a Fulcio CA in the Sigstore trust root"
            },
        )
    }

    private fun authorityRefusal(
        chain: List<Certificate>,
        validFor: ValidFor,
        at: Long,
    ): String? {
        if (!validFor.covers(at)) return "the trust root says it was not issuing certificates then"
        chain.forEachIndexed { index, certificate ->
            if (!validAt(certificate, at)) return "its certificate $index was not valid then"
            val parent = chain.getOrNull(index + 1) ?: certificate // the root signs itself
            if (!issuedBy(certificate, parent)) return "its certificate $index is not signed by the next"
        }
        return null
    }

    private fun issuedBy(
        child: Certificate,
        parent: Certificate,
    ): Boolean =
        child.issuer.contentEquals(parent.subject) &&
            verifyEcdsa(parent.spki, parent.curve, child.signatureHash, child.signature, child.tbs)

    private fun checkSigner(
        leaf: Certificate,
        signer: SignerIdentity,
    ) {
        val san = leaf.extensions[SUBJECT_ALT_NAME] ?: refuse("The signing certificate names no identity")
        val names = der { expectTag(parseDer(san), DER_SEQUENCE, "The subject alternative name").children }
        val uris = names.filter { it.tag == SAN_URI }.map { utf8(it.contents) }
        if (uris.size != 1 || names.size != 1 || uris[0] != signer.identity) {
            val named = if (uris.size == 1) jsonString(uris[0]) else uris.joinToString(",", "[", "]") { jsonString(it) }
            refuse("The release was signed by $named, not by ${signer.identity}")
        }

        val v2 = leaf.extensions[FULCIO_ISSUER_V2]
        val v1 = leaf.extensions[FULCIO_ISSUER_V1]
        val issuer =
            when {
                v2 != null -> utf8(der { expectTag(parseDer(v2), UTF8_STRING, "The OIDC issuer").contents })
                v1 != null -> utf8(v1)
                else -> null
            }
        if (issuer != signer.issuer) {
            refuse(
                "The signing certificate's OIDC issuer is ${if (issuer == null) "missing" else jsonString(issuer)}, not ${signer.issuer}",
            )
        }

        val usage = leaf.extensions[EXTENDED_KEY_USAGE]
        if (usage == null || CODE_SIGNING !in der { objectIdentifiers(usage) }) {
            refuse("The signing certificate is not for code signing")
        }
    }

    @Suppress("ComplexCondition")
    private fun cosignBundle(value: Any): CosignBundle {
        val element =
            when (value) {
                is JsonElement -> value
                is String ->
                    try {
                        bundleJson.parseToJsonElement(value)
                    } catch (_: Exception) {
                        refuse("The bundle is not JSON")
                    }
                else -> refuse("The bundle is not JSON")
            }
        val b = element as? JsonObject
        val rekor = b?.get("rekorBundle") as? JsonObject
        val payload = rekor?.get("Payload") as? JsonObject
        val base64Signature = b.string("base64Signature")
        val cert = b.string("cert")
        val set = rekor.string("SignedEntryTimestamp")
        val body = payload.string("body")
        val logId = payload.string("logID")
        val integratedTime = payload.safeInteger("integratedTime")
        val logIndex = payload.safeInteger("logIndex")
        if (base64Signature == null ||
            cert == null ||
            set == null ||
            body == null ||
            logId == null ||
            !LOG_ID.matches(logId) ||
            integratedTime == null ||
            logIndex == null
        ) {
            refuse("The bundle is not a cosign bundle with a Rekor entry")
        }
        return CosignBundle(base64Signature, cert, set, body, integratedTime, logIndex, logId)
    }

    private class LoggedEntry(
        val hash: String,
        val signature: String,
        val publicKey: String,
    )

    /** The fields of a logged `hashedrekord` entry that are compared. */
    @Suppress("ComplexCondition")
    private fun hashedRekord(body: String): LoggedEntry {
        val entry =
            try {
                bundleJson.parseToJsonElement(utf8(strictBase64(body, "The logged entry"))) as? JsonObject
            } catch (e: TeeTrustException) {
                throw e
            } catch (_: Exception) {
                refuse("The logged entry is not JSON")
            }
        val spec = entry?.get("spec") as? JsonObject
        val hash = ((spec?.get("data") as? JsonObject)?.get("hash")) as? JsonObject
        val signature = spec?.get("signature") as? JsonObject
        val hashValue = hash.string("value")
        val content = signature.string("content")
        val publicKey = (signature?.get("publicKey") as? JsonObject).string("content")
        if (entry.string("kind") != "hashedrekord" ||
            hash.string("algorithm") != "sha256" ||
            hashValue == null ||
            content == null ||
            publicKey == null
        ) {
            refuse("The logged entry is not a SHA-256 hashedrekord")
        }
        return LoggedEntry(hashValue.lowercase(), content, publicKey)
    }

    private fun validAt(
        certificate: Certificate,
        at: Long,
    ): Boolean = certificate.notBefore <= at && at <= certificate.notAfter
}

/** Run a DER read, turning the parser's refusal into a [TeeTrustException]. */
internal inline fun <T> der(read: () -> T): T =
    try {
        read()
    } catch (e: DerException) {
        throw TeeTrustException(e.message ?: "malformed DER")
    }

internal fun utf8(bytes: ByteArray): String = String(bytes, Charsets.UTF_8)

internal fun JsonObject?.string(key: String): String? = ((this?.get(key)) as? JsonPrimitive)?.takeIf { it.isString }?.content

private fun JsonObject?.safeInteger(key: String): Long? {
    val p = (this?.get(key) as? JsonPrimitive)?.takeIf { !it.isString } ?: return null
    val v = p.longOrNull ?: return null
    return v.takeIf { it in 0..MAX_SAFE_INTEGER && p.contentOrNull == v.toString() }
}
