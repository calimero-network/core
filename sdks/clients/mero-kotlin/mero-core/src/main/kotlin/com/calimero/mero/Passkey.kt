package com.calimero.mero

import com.calimero.mero.crypto.concat
import com.calimero.mero.crypto.randomBytes
import com.calimero.mero.crypto.sha256
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.ByteArrayOutputStream
import java.math.BigInteger
import java.security.KeyFactory
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.PrivateKey
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.security.spec.PKCS8EncodedKeySpec
import java.security.spec.X509EncodedKeySpec

/*
 * Passkeys: the platform's WebAuthn authenticator, or anything that answers like it (mero-js
 * `cloud-v2/passkey.ts`). Every binary field crosses the wire as base64url, matching
 * `calimero-identity`'s `Registration` and `Assertion`.
 */

/** What a passkey's registration answers (`navigator.credentials.create`), base64url. */
@Serializable
data class RegistrationJSON(
    val attestationObject: String,
    val clientDataJson: String,
)

/** What a passkey's assertion answers (`navigator.credentials.get`), base64url. */
@Serializable
data class AssertionJSON(
    val credentialId: String,
    val authenticatorData: String,
    val clientDataJson: String,
    val signature: String,
)

/**
 * Something that holds a passkey. mero-compose's `PlatformPasskey` is Android's Credential
 * Manager; [SoftPasskey] is a software one for development clusters and tests.
 *
 * A prompt the person dismissed must throw [PasskeyCancelled], so a confirmation reads it as
 * declined ([ConfirmCancelled]).
 */
interface PasskeyProvider {
    /** Answer `navigator.credentials.create` for the node's [options] (WebAuthn's JSON form). */
    suspend fun create(options: JsonObject): RegistrationJSON

    /** Sign [challenge] (an assertion), optionally limited to [allowCredentials] (base64url ids). */
    suspend fun get(
        challenge: ByteArray,
        rpId: String,
        allowCredentials: List<String>? = null,
    ): AssertionJSON
}

/** The person dismissed the passkey prompt (WebAuthn's `NotAllowedError`). */
class PasskeyCancelled(
    message: String = "the passkey prompt was cancelled",
    cause: Throwable? = null,
) : Exception(message, cause)

/** Bytes as base64url without padding. */
fun b64url(bytes: ByteArray): String =
    com.calimero.mero.crypto
        .base64(bytes)
        .replace('+', '-')
        .replace('/', '_')
        .trimEnd('=')

/** base64url (padded or not) as bytes. Throws [IllegalArgumentException] for anything else. */
fun fromB64url(value: String): ByteArray {
    val plain = value.trimEnd('=').replace('-', '+').replace('_', '/')
    require(plain.all { it.isLetterOrDigit() || it == '+' || it == '/' }) { "not base64url" }
    return try {
        com.calimero.mero.crypto
            .fromBase64(plain + "===".take((4 - plain.length % 4) % 4))
    } catch (e: Exception) {
        throw IllegalArgumentException("not base64url", e)
    }
}

/**
 * A software passkey (ES256, attestation `none`): answers registration and assertion ceremonies
 * the way a platform authenticator would, as client-py's `SoftPasskey` and meroctl's
 * `--dev-passkey` do. Only a development cluster (`merod local`, `--dev`) should ever see one: it
 * is a key in app memory, not in secure hardware, and its client data claims whatever [origin] it
 * is given. Keep it with [export] / [restore] to sign in again later.
 *
 * @param rpId the node's relying-party id (`localhost` for `merod local`).
 * @param origin the origin the node allows (`http://localhost:3000` for `merod local`).
 */
class SoftPasskey private constructor(
    val rpId: String,
    val origin: String,
    private val credentialId: ByteArray,
    private var keys: KeyPair?,
    private var counter: Long,
) : PasskeyProvider {
    constructor(rpId: String, origin: String) : this(rpId, origin, randomBytes(16), null, 0)

    /** The credential id, base64url. */
    val id: String get() = b64url(credentialId)

    /** Whether it holds a key yet ([create] made one, or it was [restore]d). */
    val hasKey: Boolean get() = keys != null

    override suspend fun create(options: JsonObject): RegistrationJSON {
        val generator = KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }
        val pair = generator.generateKeyPair()
        keys = pair
        val point = (pair.public as ECPublicKey).w
        val cose =
            Cbor.map(
                listOf(
                    Cbor.int(1) to Cbor.int(2),
                    Cbor.int(3) to Cbor.int(-7),
                    Cbor.int(-1) to Cbor.int(1),
                    Cbor.int(-2) to Cbor.bytes(fixed32(point.affineX)),
                    Cbor.int(-3) to Cbor.bytes(fixed32(point.affineY)),
                ),
            )
        val attested = concat(ByteArray(16), byteArrayOf(0, credentialId.size.toByte()), credentialId, cose)
        val authData = authData(FLAG_UP or FLAG_UV or FLAG_AT, attested)
        val att =
            Cbor.map(
                listOf(
                    Cbor.text("fmt") to Cbor.text("none"),
                    Cbor.text("attStmt") to Cbor.map(emptyList()),
                    Cbor.text("authData") to Cbor.bytes(authData),
                ),
            )
        val challenge = (options["challenge"] as? JsonPrimitive)?.content ?: throw IllegalArgumentException("options carry no challenge")
        return RegistrationJSON(b64url(att), b64url(clientData("webauthn.create", fromB64url(challenge))))
    }

    override suspend fun get(
        challenge: ByteArray,
        rpId: String,
        allowCredentials: List<String>?,
    ): AssertionJSON {
        val pair = checkNotNull(keys) { "no passkey yet: create one first" }
        counter += 1
        val clientData = clientData("webauthn.get", challenge)
        val authData = authData(FLAG_UP or FLAG_UV, null)
        val signer = Signature.getInstance("SHA256withECDSA").apply { initSign(pair.private) }
        signer.update(concat(authData, sha256(clientData)))
        // The JCA signs in ASN.1 DER already, which WebAuthn ES256 signatures use.
        return AssertionJSON(id, b64url(authData), b64url(clientData), b64url(signer.sign()))
    }

    /** The key, credential id and counter, to keep and [restore] later. Secret: store it encrypted. */
    fun export(): String {
        val pair = checkNotNull(keys) { "no passkey yet: create one first" }
        return buildJsonObject {
            put("rpId", rpId)
            put("origin", origin)
            put("credentialId", id)
            put("privateKey", b64url(pair.private.encoded))
            put("publicKey", b64url(pair.public.encoded))
            put("counter", counter)
        }.toString()
    }

    private fun authData(
        flags: Int,
        attested: ByteArray?,
    ): ByteArray {
        val count = ByteArray(4) { i -> (counter ushr (8 * (3 - i))).toByte() }
        val head = concat(sha256(rpId.encodeToByteArray()), byteArrayOf(flags.toByte()), count)
        return if (attested == null) head else concat(head, attested)
    }

    private fun clientData(
        type: String,
        challenge: ByteArray,
    ): ByteArray =
        buildJsonObject {
            put("type", type)
            put("challenge", b64url(challenge))
            put("origin", origin)
            put("crossOrigin", false)
        }.toString().encodeToByteArray()

    companion object {
        private const val FLAG_UP = 0x01
        private const val FLAG_UV = 0x04
        private const val FLAG_AT = 0x40

        /** A passkey [SoftPasskey.export] wrote. */
        fun restore(exported: String): SoftPasskey {
            val o =
                kotlinx.serialization.json.Json
                    .parseToJsonElement(exported) as JsonObject

            fun s(key: String) = (o[key] as? JsonPrimitive)?.content ?: throw IllegalArgumentException("missing $key")
            val factory = KeyFactory.getInstance("EC")
            val private: PrivateKey = factory.generatePrivate(PKCS8EncodedKeySpec(fromB64url(s("privateKey"))))
            val public = factory.generatePublic(X509EncodedKeySpec(fromB64url(s("publicKey"))))
            return SoftPasskey(s("rpId"), s("origin"), fromB64url(s("credentialId")), KeyPair(public, private), s("counter").toLong())
        }

        private fun fixed32(value: BigInteger): ByteArray {
            val raw = value.toByteArray()
            return when {
                raw.size == 32 -> raw
                raw.size > 32 -> raw.copyOfRange(raw.size - 32, raw.size)
                else -> ByteArray(32 - raw.size) + raw
            }
        }
    }
}

/** The minimal CBOR encoder a [SoftPasskey] needs: ints, byte and text strings, maps. */
internal object Cbor {
    fun head(
        major: Int,
        n: Long,
    ): ByteArray {
        val m = major shl 5
        return when {
            n < 24 -> byteArrayOf((m or n.toInt()).toByte())
            n < 256 -> byteArrayOf((m or 24).toByte(), n.toByte())
            n < 65536 -> byteArrayOf((m or 25).toByte(), (n shr 8).toByte(), n.toByte())
            else -> byteArrayOf((m or 26).toByte(), (n shr 24).toByte(), (n shr 16).toByte(), (n shr 8).toByte(), n.toByte())
        }
    }

    fun int(v: Long): ByteArray = if (v >= 0) head(0, v) else head(1, -1 - v)

    fun int(v: Int): ByteArray = int(v.toLong())

    fun bytes(v: ByteArray): ByteArray = head(2, v.size.toLong()) + v

    fun text(v: String): ByteArray = v.encodeToByteArray().let { head(3, it.size.toLong()) + it }

    fun map(entries: List<Pair<ByteArray, ByteArray>>): ByteArray {
        val out = ByteArrayOutputStream()
        out.write(head(5, entries.size.toLong()))
        entries.forEach { (k, v) ->
            out.write(k)
            out.write(v)
        }
        return out.toByteArray()
    }
}
