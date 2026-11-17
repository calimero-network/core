package com.calimero.mero.tee

import com.calimero.mero.ExportError
import com.calimero.mero.ExportErrorCode
import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.concat
import com.calimero.mero.crypto.sha256
import org.bouncycastle.crypto.digests.SHA256Digest
import org.bouncycastle.crypto.generators.HKDFBytesGenerator
import org.bouncycastle.crypto.params.HKDFParameters
import java.security.MessageDigest

/*
 * Opening a sealed file, byte for byte as core's `calimero-sealed` crate (`file.rs`, `wrap.rs`)
 * writes it (mero-js `sealed/file.ts`): 64 KiB AES-256-GCM STREAM chunks under a data key, cut
 * into segments, and that key wrapped to an X25519 key the requester made for one export.
 */

private const val CHUNK_BYTES = 64 * 1024 // plaintext bytes in every chunk but the last
private const val TAG_BYTES = 16
private const val SEALED_CHUNK_BYTES = CHUNK_BYTES + TAG_BYTES
private const val NONCE_BYTES = 12
private const val KEY_BYTES = 32

/** Stored bytes of every segment but the last: 256 whole sealed chunks. */
internal const val SEGMENT_BYTES: Int = 256 * SEALED_CHUNK_BYTES

private val AAD_PREFIX = "calimero.file.v1".encodeToByteArray() // then the last-chunk flag byte
private val SEGMENT_DOMAIN = "calimero.file.segments.v1\u0000".encodeToByteArray() // prefix of a file id's preimage
private val EXPORT_WRAP_SALT = "calimero.export-wrap.v1".encodeToByteArray()
private val DERIVE_LABEL = "calimero.sealed.derive.v1\u0000".encodeToByteArray() // prefix of every wrap's HKDF info

private fun u32be(value: Int): ByteArray = ByteArray(4) { i -> (value ushr (8 * (3 - i))).toByte() }

private fun prefixed(field: String): ByteArray {
    val bytes = field.encodeToByteArray()
    return concat(u32be(bytes.size), bytes)
}

private fun tampered(message: String): Nothing = throw ExportError(ExportErrorCode.TAMPERED, message)

/** The file id: SHA-256 over the domain, the segment count (u32 big-endian) and the ordered segment hashes. */
@Suppress("SpreadOperator")
internal fun fileId(segments: List<ByteArray>): ByteArray = sha256(concat(SEGMENT_DOMAIN, u32be(segments.size), *segments.toTypedArray()))

/** Open the data key [wrapped] (`nonce || AES-256-GCM`) the node wrapped to [recipient] for app [appId] and file [fileId] (hex). */
internal fun unwrapExport(
    recipient: X25519KeyPair,
    ephemeral: ByteArray,
    appId: String,
    fileId: String,
    wrapped: ByteArray,
): ByteArray {
    if (wrapped.size < NONCE_BYTES) tampered("the wrapped key is truncated")
    val shared =
        try {
            recipient.dh(ephemeral)
        } catch (_: IllegalStateException) {
            // Bouncy Castle refuses an all-zero shared secret: a low-order point.
            throw ExportError(ExportErrorCode.BAD_ANSWER, "the ephemeral key is of low order")
        }
    if (shared.all { it == 0.toByte() }) throw ExportError(ExportErrorCode.BAD_ANSWER, "the ephemeral key is of low order")
    val info = concat(DERIVE_LABEL, ephemeral, recipient.publicKey, prefixed(appId), prefixed(fileId))
    val hkdf = HKDFBytesGenerator(SHA256Digest())
    hkdf.init(HKDFParameters(shared, EXPORT_WRAP_SALT, info))
    val key = ByteArray(KEY_BYTES).also { hkdf.generateBytes(it, 0, KEY_BYTES) }
    val secret =
        try {
            AesGcm.open(key, wrapped.copyOfRange(0, NONCE_BYTES), ByteArray(0), wrapped.copyOfRange(NONCE_BYTES, wrapped.size))
        } catch (_: AeadException) {
            tampered("the wrapped key does not open")
        }
    if (secret.size != KEY_BYTES) tampered("the wrapped key has the wrong length")
    return secret
}

/**
 * Joins a file's segments in order, each checked against the list before it is opened; holds at
 * most one segment and one chunk. Output is authentic but may be a prefix of the file until
 * [finishExpecting] returns.
 */
internal class SegmentReader(
    private val dataKey: ByteArray,
    segments: List<ByteArray>,
) {
    private val segments: List<String> = segments.map(Hex::encode)
    private var pending = ByteArray(0)
    private var index = 0L
    private var next = 0
    private var size = 0L
    private val hash = MessageDigest.getInstance("SHA-256")

    /** Take the next segment; [emit] gets the plaintext of every chunk it completes. */
    suspend fun push(
        segment: ByteArray,
        emit: suspend (ByteArray) -> Unit,
    ) {
        val want = segments.getOrNull(next) ?: tampered("the file has no segment $next")
        if (next + 1 < segments.size && segment.size != SEGMENT_BYTES) tampered("segment $next is not a whole segment")
        if (Hex.encode(sha256(segment)) != want) tampered("segment $next is not the one the file lists")
        next += 1
        // Bytes past a full chunk prove it was not the last, so one chunk is always held back.
        val data = concat(pending, segment)
        var at = 0
        while (data.size - at > SEALED_CHUNK_BYTES) {
            emit(open(data.copyOfRange(at, at + SEALED_CHUNK_BYTES), false))
            at += SEALED_CHUNK_BYTES
        }
        pending = data.copyOfRange(at, data.size)
    }

    /** Open the last chunk and check the whole plaintext's size and SHA-256: GCM does not commit to its key. */
    fun finishExpecting(
        size: Long,
        sha256Hex: String,
    ): ByteArray {
        if (next != segments.size) tampered("the file is missing segments")
        if (pending.size < TAG_BYTES) tampered("the file is truncated")
        val last = open(pending, true)
        if (this.size != size || Hex.encode(hash.digest()) != sha256Hex) tampered("the file is not the one that was sealed")
        return last
    }

    private fun open(
        chunk: ByteArray,
        last: Boolean,
    ): ByteArray {
        val iv = ByteArray(NONCE_BYTES)
        for (i in 0 until 8) iv[4 + i] = (index ushr (8 * (7 - i))).toByte()
        val plain =
            try {
                AesGcm.open(dataKey, iv, concat(AAD_PREFIX, byteArrayOf(if (last) 1 else 0)), chunk)
            } catch (_: AeadException) {
                tampered("chunk $index does not open: the file is altered, truncated or under another key")
            }
        hash.update(plain)
        size += plain.size
        index += 1
        return plain
    }
}
