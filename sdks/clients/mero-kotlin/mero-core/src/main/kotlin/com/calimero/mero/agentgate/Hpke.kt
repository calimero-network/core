package com.calimero.mero.agentgate

import com.calimero.mero.crypto.concat
import com.calimero.mero.tee.X25519KeyPair
import com.calimero.mero.tee.hmacSha256
import org.bouncycastle.crypto.InvalidCipherTextException
import org.bouncycastle.crypto.modes.ChaCha20Poly1305
import org.bouncycastle.crypto.params.AEADParameters
import org.bouncycastle.crypto.params.KeyParameter

/*
 * RFC 9180 HPKE, the one suite and mode mero-agent-gate takes: Auth mode, DHKEM(X25519,
 * HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305, single-shot (one message per context, sequence
 * number 0) — port of mero-js `src/agent-gate/hpke.ts`. Auth mode proves the sender: the gate
 * opens a bundle only under the X25519 key it names, so nobody can seal in an owner's name.
 *
 * X25519, HMAC and ChaCha20-Poly1305 are Bouncy Castle's. The tests open what mero-agent-gate
 * (Rust, the `hpke` crate) sealed, and Bouncy Castle's own HPKE opens what this seals.
 */

private const val KEM_ID = 0x0020 // DHKEM(X25519, HKDF-SHA256)
private const val KDF_ID = 0x0001 // HKDF-SHA256
private const val AEAD_ID = 0x0003 // ChaCha20-Poly1305
private const val MODE_AUTH = 0x02
private const val N_SECRET = 32
private const val N_KEY = 32
private const val N_NONCE = 12
private const val TAG_BITS = 128

private fun i2osp2(n: Int): ByteArray = byteArrayOf((n shr 8).toByte(), n.toByte())

private val KEM_SUITE = concat("KEM".encodeToByteArray(), i2osp2(KEM_ID))
private val HPKE_SUITE = concat("HPKE".encodeToByteArray(), i2osp2(KEM_ID), i2osp2(KDF_ID), i2osp2(AEAD_ID))
private val HPKE_V1 = "HPKE-v1".encodeToByteArray()

/** HMAC with an empty salt read as HashLen zeros (RFC 5869). */
private fun hmac(
    key: ByteArray,
    data: ByteArray,
): ByteArray = hmacSha256(if (key.isEmpty()) ByteArray(32) else key, data)

private fun expand(
    prk: ByteArray,
    info: ByteArray,
    length: Int,
): ByteArray {
    val out = ByteArray(length)
    var previous = ByteArray(0)
    var offset = 0
    var i = 1
    while (offset < length) {
        previous = hmac(prk, concat(previous, info, byteArrayOf(i.toByte())))
        previous.copyInto(out, offset, 0, minOf(previous.size, length - offset))
        offset += previous.size
        i += 1
    }
    return out
}

private fun labeledExtract(
    suite: ByteArray,
    salt: ByteArray,
    label: String,
    ikm: ByteArray,
): ByteArray = hmac(salt, concat(HPKE_V1, suite, label.encodeToByteArray(), ikm))

private fun labeledExpand(
    suite: ByteArray,
    prk: ByteArray,
    label: String,
    info: ByteArray,
    length: Int,
): ByteArray = expand(prk, concat(i2osp2(length), HPKE_V1, suite, label.encodeToByteArray(), info), length)

private fun extractAndExpand(
    dh: ByteArray,
    kemContext: ByteArray,
): ByteArray {
    val prk = labeledExtract(KEM_SUITE, ByteArray(0), "eae_prk", dh)
    return labeledExpand(KEM_SUITE, prk, "shared_secret", kemContext, N_SECRET)
}

/** `(key, base_nonce)` of the Auth-mode key schedule. */
private fun keySchedule(
    sharedSecret: ByteArray,
    info: ByteArray,
): Pair<ByteArray, ByteArray> {
    val empty = ByteArray(0)
    val pskIdHash = labeledExtract(HPKE_SUITE, empty, "psk_id_hash", empty)
    val infoHash = labeledExtract(HPKE_SUITE, empty, "info_hash", info)
    val context = concat(byteArrayOf(MODE_AUTH.toByte()), pskIdHash, infoHash)
    val secret = labeledExtract(HPKE_SUITE, sharedSecret, "secret", empty)
    return labeledExpand(HPKE_SUITE, secret, "key", context, N_KEY) to labeledExpand(HPKE_SUITE, secret, "base_nonce", context, N_NONCE)
}

private fun chacha(
    encrypt: Boolean,
    key: ByteArray,
    nonce: ByteArray,
    input: ByteArray,
    aad: ByteArray,
): ByteArray {
    val cipher = ChaCha20Poly1305()
    cipher.init(encrypt, AEADParameters(KeyParameter(key), TAG_BITS, nonce, aad))
    val out = ByteArray(cipher.getOutputSize(input.size))
    val n = cipher.processBytes(input, 0, input.size, out, 0)
    val total = n + cipher.doFinal(out, n)
    return if (total == out.size) out else out.copyOf(total)
}

/** ChaCha20-Poly1305 (RFC 8439): `ciphertext ‖ tag`. */
internal fun chacha20Poly1305Seal(
    key: ByteArray,
    nonce: ByteArray,
    plaintext: ByteArray,
    aad: ByteArray = ByteArray(0),
): ByteArray = chacha(true, key, nonce, plaintext, aad)

/** Open [chacha20Poly1305Seal]'s output; throws when it does not authenticate. */
internal fun chacha20Poly1305Open(
    key: ByteArray,
    nonce: ByteArray,
    sealed: ByteArray,
    aad: ByteArray = ByteArray(0),
): ByteArray =
    try {
        chacha(false, key, nonce, sealed, aad)
    } catch (_: InvalidCipherTextException) {
        throw AgentGateException("The sealed message did not authenticate")
    }

/** An HPKE message: the encapsulated key (the sender's ephemeral X25519 public key) and the ciphertext. */
internal class HpkeSealed(
    val enc: ByteArray,
    val ciphertext: ByteArray,
)

/** Seal [plaintext] to [recipientPublic] in Auth mode as the holder of [senderSecret] (32 raw X25519 bytes). */
@Suppress("LongParameterList")
internal fun hpkeSealAuth(
    recipientPublic: ByteArray,
    senderSecret: ByteArray,
    info: ByteArray,
    plaintext: ByteArray,
    aad: ByteArray = ByteArray(0),
    ephemeralSecret: ByteArray? = null,
): HpkeSealed {
    if (recipientPublic.size != 32) throw AgentGateException("The recipient key is not 32 bytes of X25519")
    val ephemeral = X25519KeyPair.of(ephemeralSecret)
    val sender = X25519KeyPair.of(senderSecret)
    val dh = concat(ephemeral.dh(recipientPublic), sender.dh(recipientPublic))
    val shared = extractAndExpand(dh, concat(ephemeral.publicKey, recipientPublic, sender.publicKey))
    val (key, nonce) = keySchedule(shared, info)
    return HpkeSealed(ephemeral.publicKey, chacha20Poly1305Seal(key, nonce, plaintext, aad))
}

/** Open what [hpkeSealAuth] (or the gate's own sealer) made, as the recipient. */
internal fun hpkeOpenAuth(
    recipientSecret: ByteArray,
    senderPublic: ByteArray,
    info: ByteArray,
    sealed: HpkeSealed,
    aad: ByteArray = ByteArray(0),
): ByteArray {
    val recipient = X25519KeyPair.of(recipientSecret)
    val dh = concat(recipient.dh(sealed.enc), recipient.dh(senderPublic))
    val shared = extractAndExpand(dh, concat(sealed.enc, recipient.publicKey, senderPublic))
    val (key, nonce) = keySchedule(shared, info)
    return chacha20Poly1305Open(key, nonce, sealed.ciphertext, aad)
}
