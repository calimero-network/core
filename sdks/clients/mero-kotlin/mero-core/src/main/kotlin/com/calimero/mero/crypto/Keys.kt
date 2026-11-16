package com.calimero.mero.crypto

import org.bouncycastle.crypto.params.Ed25519PrivateKeyParameters
import org.bouncycastle.crypto.params.Ed25519PublicKeyParameters
import org.bouncycastle.crypto.params.X25519PrivateKeyParameters
import org.bouncycastle.crypto.signers.Ed25519Signer
import org.bouncycastle.util.encoders.Base64
import java.security.SecureRandom

/**
 * Ed25519 over Bouncy Castle's lightweight API.
 *
 * The JCA gained Ed25519 at API 33 and the SDK supports 24, so this goes through BC
 * directly (no provider registration, so no clash with the platform's repackaged
 * copy). Ed25519 is deterministic: the same seed and message always give the same
 * signature, which is what lets the golden vectors pin exact bytes.
 */
object Ed25519 {
    const val SEED_BYTES = 32
    const val PUBLIC_KEY_BYTES = 32
    const val SIGNATURE_BYTES = 64

    /** The public half of a 32-byte seed. */
    fun publicKey(seed: ByteArray): ByteArray {
        require(seed.size == SEED_BYTES) { "an Ed25519 seed is 32 bytes, got ${seed.size}" }
        return Ed25519PrivateKeyParameters(seed, 0).generatePublicKey().encoded
    }

    /** Sign [message] with the key derived from [seed]. */
    fun sign(
        seed: ByteArray,
        message: ByteArray,
    ): ByteArray {
        require(seed.size == SEED_BYTES) { "an Ed25519 seed is 32 bytes, got ${seed.size}" }
        val signer = Ed25519Signer()
        signer.init(true, Ed25519PrivateKeyParameters(seed, 0))
        signer.update(message, 0, message.size)
        return signer.generateSignature()
    }

    /** Check [signature] over [message] against a raw 32-byte [publicKey]. Never throws on a bad signature. */
    fun verify(
        publicKey: ByteArray,
        signature: ByteArray,
        message: ByteArray,
    ): Boolean {
        if (publicKey.size != PUBLIC_KEY_BYTES || signature.size != SIGNATURE_BYTES) return false
        return try {
            val verifier = Ed25519Signer()
            verifier.init(false, Ed25519PublicKeyParameters(publicKey, 0))
            verifier.update(message, 0, message.size)
            verifier.verifySignature(signature)
        } catch (_: IllegalArgumentException) {
            false
        }
    }
}

/** X25519 key generation: the device's key-delivery ("KEM") pair, where group keys are sealed to. */
object X25519 {
    /** A fresh pair as `(secret, public)`, 32 bytes each. */
    fun generate(random: SecureRandom = SecureRandom()): Pair<ByteArray, ByteArray> {
        val secret = X25519PrivateKeyParameters(random)
        return secret.encoded to secret.generatePublicKey().encoded
    }

    /** The public half of a 32-byte X25519 secret. */
    fun publicKey(secret: ByteArray): ByteArray {
        require(secret.size == 32) { "an X25519 secret is 32 bytes, got ${secret.size}" }
        return X25519PrivateKeyParameters(secret, 0).generatePublicKey().encoded
    }
}

/**
 * Something that signs as a device: a key it may use and need not export.
 *
 * Mirrors mero-js's `Signer`. The SDK's own implementation is [SeedSigner]; an app
 * holding the device key in hardware can supply its own.
 */
interface DeviceSigner {
    /** The Ed25519 public key, 64 lowercase hex. */
    val publicKey: String

    /** A 64-byte Ed25519 signature over [message], exactly as given (no pre-hash). */
    fun sign(message: ByteArray): ByteArray
}

/**
 * A [DeviceSigner] over a 32-byte seed held in memory.
 *
 * @param label what to call the seed in an error — `deviceSecret`, `rootSecret`.
 */
class SeedSigner(
    secretHex: String,
    label: String = "deviceSecret",
) : DeviceSigner {
    private val seed = Hex.decode(secretHex, label, Ed25519.SEED_BYTES)

    override val publicKey: String = Hex.encode(Ed25519.publicKey(seed))

    override fun sign(message: ByteArray): ByteArray = Ed25519.sign(seed, message)

    override fun toString(): String = "SeedSigner(publicKey=$publicKey)"
}

/**
 * A signer over a raw 32-byte Ed25519 seed, hex — mero-js `signerFromSecret`. The public
 * half is derived from the seed, never accepted beside it.
 *
 * @param label what to call it in an error — `deviceSecret`, `rootSecret`.
 */
fun signerFromSecret(
    secret: String,
    label: String = "secret",
): DeviceSigner = SeedSigner(secret, label)

/**
 * The signer an input describes, from either form — mero-js `resolveSigner`. Exactly one
 * of [secret] and [signer] must be given: both is refused rather than silently preferring
 * one, because they could name different keys.
 *
 * @throws IllegalArgumentException with both or neither.
 */
fun resolveSigner(
    secret: String?,
    signer: DeviceSigner?,
    label: String,
): DeviceSigner {
    require(signer == null || secret == null) { "pass either $label or signer, not both — they may name different keys" }
    return signer ?: secret?.let { signerFromSecret(it, label) } ?: throw IllegalArgumentException("$label or signer is required")
}

/** Check a signer's output is a 64-byte Ed25519 signature before it goes into a structure. */
internal fun checkedSignature(signature: ByteArray): ByteArray {
    require(signature.size == Ed25519.SIGNATURE_BYTES) {
        "signer returned ${signature.size} bytes, expected a 64-byte Ed25519 signature"
    }
    return signature
}

/** Standard base64 (with padding). `java.util.Base64` is API 26+, so BC's encoder is used. */
fun base64(bytes: ByteArray): String = Base64.toBase64String(bytes)

/** Decode standard base64. */
fun fromBase64(value: String): ByteArray = Base64.decode(value)

/** [count] cryptographically random bytes. */
fun randomBytes(count: Int): ByteArray = ByteArray(count).also { SecureRandom().nextBytes(it) }
