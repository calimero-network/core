package com.calimero.mero.tee

import com.calimero.mero.crypto.concat
import com.calimero.mero.crypto.sha256
import org.bouncycastle.crypto.InvalidCipherTextException
import org.bouncycastle.crypto.digests.SHA256Digest
import org.bouncycastle.crypto.engines.AESEngine
import org.bouncycastle.crypto.macs.HMac
import org.bouncycastle.crypto.modes.GCMBlockCipher
import org.bouncycastle.crypto.params.AEADParameters
import org.bouncycastle.crypto.params.KeyParameter
import org.bouncycastle.crypto.params.X25519PrivateKeyParameters
import org.bouncycastle.crypto.params.X25519PublicKeyParameters
import java.security.SecureRandom

/*
 * The Noise NK handshake (`Noise_NK_25519_AESGCM_SHA256`) that opens a sealed session —
 * port of mero-js `src/sealed/noise.ts`, written from the Noise specification (revision 34).
 * The node runs the other side with `snow`; core's published vectors check the two against
 * each other (`SealedTransportTest`).
 *
 * NK: the initiator knows the responder's static key in advance — here the transport key the
 * node's quote commits to — and sends nothing but an ephemeral key before both ephemerals are
 * mixed in, so everything after the handshake has forward secrecy.
 */

private const val PROTOCOL_NAME = "Noise_NK_25519_AESGCM_SHA256"
private const val DH_LEN = 32
private const val TAG_LEN = 16

/** A Noise message or a sealed frame that did not authenticate. */
internal class AeadException(
    message: String,
) : Exception(message)

/** AES-256-GCM with a 128-bit tag. Throws [AeadException] when a ciphertext does not open. */
internal object AesGcm {
    fun seal(
        key: ByteArray,
        nonce: ByteArray,
        aad: ByteArray,
        plaintext: ByteArray,
    ): ByteArray = run(true, key, nonce, aad, plaintext)

    fun open(
        key: ByteArray,
        nonce: ByteArray,
        aad: ByteArray,
        ciphertext: ByteArray,
    ): ByteArray = run(false, key, nonce, aad, ciphertext)

    private fun run(
        encrypt: Boolean,
        key: ByteArray,
        nonce: ByteArray,
        aad: ByteArray,
        input: ByteArray,
    ): ByteArray {
        val cipher = GCMBlockCipher.newInstance(AESEngine.newInstance())
        cipher.init(encrypt, AEADParameters(KeyParameter(key), TAG_LEN * 8, nonce, aad))
        val out = ByteArray(cipher.getOutputSize(input.size))
        return try {
            val n = cipher.processBytes(input, 0, input.size, out, 0)
            val total = n + cipher.doFinal(out, n)
            if (total == out.size) out else out.copyOf(total)
        } catch (_: InvalidCipherTextException) {
            throw AeadException("it did not authenticate")
        }
    }
}

/** HMAC-SHA256. */
internal fun hmacSha256(
    key: ByteArray,
    data: ByteArray,
): ByteArray {
    val mac = HMac(SHA256Digest())
    mac.init(KeyParameter(key))
    mac.update(data, 0, data.size)
    return ByteArray(mac.macSize).also { mac.doFinal(it, 0) }
}

/** An X25519 key pair; [secret] is the raw 32 bytes. */
internal class X25519KeyPair(
    val secret: ByteArray,
    val publicKey: ByteArray,
) {
    /** The shared secret with [peer]. Throws for a low-order point, as WebCrypto does. */
    fun dh(peer: ByteArray): ByteArray {
        require(peer.size == DH_LEN) { "an X25519 public key is 32 bytes" }
        val out = ByteArray(DH_LEN)
        X25519PrivateKeyParameters(secret, 0).generateSecret(X25519PublicKeyParameters(peer, 0), out, 0)
        return out
    }

    companion object {
        /** The pair of a fixed [secret], or a fresh one. */
        fun of(secret: ByteArray? = null): X25519KeyPair {
            val key = if (secret == null) X25519PrivateKeyParameters(SecureRandom()) else X25519PrivateKeyParameters(secret, 0)
            return X25519KeyPair(key.encoded, key.generatePublicKey().encoded)
        }
    }
}

/** Noise's `SymmetricState`: the chaining key, the handshake hash and the cipher key. */
internal class SymmetricState {
    private var ck: ByteArray
    private var h: ByteArray
    private var k: ByteArray? = null
    private var n = 0L

    init {
        // A name of 32 bytes or fewer is used as the hash, zero-padded.
        val name = PROTOCOL_NAME.encodeToByteArray()
        h = ByteArray(32).also { name.copyInto(it) }
        ck = h
    }

    fun mixHash(data: ByteArray) {
        h = sha256(concat(h, data))
    }

    fun mixKey(inputKeyMaterial: ByteArray) {
        val (ck2, key) = hkdf2(ck, inputKeyMaterial)
        ck = ck2
        k = key
        n = 0
    }

    fun encryptAndHash(plaintext: ByteArray): ByteArray {
        val key = checkNotNull(k) { "Noise: no cipher key yet" }
        val ciphertext = AesGcm.seal(key, noiseNonce(n), h, plaintext)
        n += 1
        mixHash(ciphertext)
        return ciphertext
    }

    fun decryptAndHash(ciphertext: ByteArray): ByteArray {
        val key = checkNotNull(k) { "Noise: no cipher key yet" }
        val plaintext = AesGcm.open(key, noiseNonce(n), h, ciphertext)
        n += 1
        mixHash(ciphertext)
        return plaintext
    }

    /** The initiator-to-responder key, then the responder-to-initiator key. */
    fun split(): Pair<ByteArray, ByteArray> = hkdf2(ck, ByteArray(0))

    private companion object {
        /** Noise's AESGCM nonce: 32 zero bits, then the counter as a big-endian u64. */
        fun noiseNonce(n: Long): ByteArray = ByteArray(12).also { for (i in 0 until 8) it[4 + i] = (n ushr (8 * (7 - i))).toByte() }

        /** Noise's `HKDF` with two outputs. */
        fun hkdf2(
            chainingKey: ByteArray,
            inputKeyMaterial: ByteArray,
        ): Pair<ByteArray, ByteArray> {
            val temp = hmacSha256(chainingKey, inputKeyMaterial)
            val first = hmacSha256(temp, byteArrayOf(1))
            val second = hmacSha256(temp, concat(first, byteArrayOf(2)))
            return first to second
        }
    }
}

/** What finishing the handshake yields: the responder's payload and the two transport keys. */
internal class NoiseFinished(
    val payload: ByteArray,
    val split: Pair<ByteArray, ByteArray>,
)

/** An NK handshake in progress: [message1] to send, then [finish] with the responder's message 2. */
internal class NoiseInitiator internal constructor(
    private val state: SymmetricState,
    private val ephemeral: X25519KeyPair,
    val message1: ByteArray,
) {
    /** Throws [AeadException] when message 2 is not from the holder of the remote static key. */
    fun finish(message2: ByteArray): NoiseFinished {
        if (message2.size < DH_LEN + TAG_LEN) throw AeadException("Noise: message 2 is too short")
        // <- e, ee
        val remoteEphemeral = message2.copyOfRange(0, DH_LEN)
        state.mixHash(remoteEphemeral)
        state.mixKey(ephemeral.dh(remoteEphemeral))
        val payload = state.decryptAndHash(message2.copyOfRange(DH_LEN, message2.size))
        return NoiseFinished(payload, state.split())
    }

    companion object {
        /**
         * Start an NK handshake to [remoteStatic]. [fixedEphemeral] pins the ephemeral
         * secret, for the published test vectors only.
         */
        fun initiate(
            remoteStatic: ByteArray,
            prologue: ByteArray,
            fixedEphemeral: ByteArray? = null,
        ): NoiseInitiator {
            val state = SymmetricState()
            state.mixHash(prologue)
            // <- s (known in advance)
            state.mixHash(remoteStatic)
            // -> e, es
            val ephemeral = X25519KeyPair.of(fixedEphemeral)
            state.mixHash(ephemeral.publicKey)
            state.mixKey(ephemeral.dh(remoteStatic))
            return NoiseInitiator(state, ephemeral, concat(ephemeral.publicKey, state.encryptAndHash(ByteArray(0))))
        }
    }
}
