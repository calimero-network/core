package com.calimero.mero

import com.calimero.mero.crypto.Ed25519
import com.calimero.mero.crypto.randomBytes
import kotlinx.serialization.Serializable

/*
 * An organization's recovery key (core 1.0.0-rc.10, `docs/specs/org-recovery.md`; mero-js
 * `cloud-v2/recovery.ts`): an Ed25519 key whose 32-byte seed the owner keeps offline. The node
 * stores only the public half; the seed signs, here and never on a node, the statement that hands
 * the organization to one account. Byte-for-byte core's `calimero_identity::recovery`.
 */

/** What a recovery signature covers before the organization, the account and the nonce. */
const val RECOVERY_DOMAIN: String = "calimero.org-recovery.v1\u0000"

/** The longest nonce a recovery carries, in bytes (core's `MAX_RECOVERY_NONCE_BYTES`). */
const val MAX_RECOVERY_NONCE_BYTES: Int = 64

/** A new random recovery seed (32 bytes). */
fun newRecoverySeed(): ByteArray = randomBytes(Ed25519.SEED_BYTES)

/** The seed as people write it down: 64 hex digits in eight groups of eight, joined by `-`. */
fun recoverySeedText(seed: ByteArray): String {
    checkSeed(seed)
    return seed.joinToString("") { "%02x".format(it) }.chunked(8).joinToString("-")
}

/** A seed from its written form: 64 hex digits; `-`, whitespace and case are ignored. */
fun parseRecoverySeed(text: String): ByteArray {
    val digits = text.filterNot { it == '-' || it.isWhitespace() }
    require(digits.length == 64 && digits.all { it in '0'..'9' || it.lowercaseChar() in 'a'..'f' }) { "a recovery seed is 64 hex digits" }
    return ByteArray(32) { i -> digits.substring(2 * i, 2 * i + 2).toInt(16).toByte() }
}

/** The bytes a recovery signs: `"calimero.org-recovery.v1\0" || org || 0 || account || 0 || nonce`. */
fun recoveryMessage(
    org: String,
    account: String,
    nonce: String,
): ByteArray = "$RECOVERY_DOMAIN$org\u0000$account\u0000$nonce".encodeToByteArray()

/** The seed's public key, base64url: what the organization stores ([CalimeroClient.setRecoveryKey]). */
fun recoveryPublicKey(seed: ByteArray): String {
    checkSeed(seed)
    return b64url(Ed25519.publicKey(seed))
}

/** The seed's signature (base64url) handing [org] to [account] under [nonce]. */
fun signRecovery(
    seed: ByteArray,
    org: String,
    account: String,
    nonce: String,
): String {
    checkSeed(seed)
    val n = nonce.encodeToByteArray().size
    require(n in 1..MAX_RECOVERY_NONCE_BYTES) { "a recovery nonce is 1 to $MAX_RECOVERY_NONCE_BYTES bytes" }
    return b64url(Ed25519.sign(seed, recoveryMessage(org, account, nonce)))
}

private fun checkSeed(seed: ByteArray) = require(seed.size == Ed25519.SEED_BYTES) { "a recovery seed is 32 bytes" }

/** An organization's recovery key ([OrgInfo.recovery]). */
@Serializable
data class OrgRecovery(
    /** The key's Ed25519 public half, base64url. */
    val publicKey: String,
    /** When the owner set it (Unix seconds). */
    val setAt: Long = 0,
    /** A recovery under way: warn every member, the owner first, who may cancel it. */
    val pending: PendingRecovery? = null,
)

/** A recovery started with the seed and not finished or cancelled yet. */
@Serializable
data class PendingRecovery(
    /** The account the organization goes to. */
    val account: String,
    val startedAt: Long = 0,
    val readyAt: Long = 0,
)

/** When a recovery started and the earliest it may finish (Unix seconds). */
@Serializable
data class RecoveryTimes(
    val startedAt: Long = 0,
    val readyAt: Long = 0,
)

/** What finishing a recovery answers: the organization and the owners it removed. */
@Serializable
data class RecoveryFinished(
    val org: String = "",
    val previousOwners: List<String> = emptyList(),
)
