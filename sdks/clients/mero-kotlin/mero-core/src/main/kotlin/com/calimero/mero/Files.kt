package com.calimero.mero

import com.calimero.mero.crypto.Hex
import com.calimero.mero.tee.SEGMENT_BYTES
import com.calimero.mero.tee.SegmentReader
import com.calimero.mero.tee.X25519KeyPair
import com.calimero.mero.tee.fileId
import com.calimero.mero.tee.unwrapExport
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.ensureActive
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.longOrNull
import okhttp3.Response
import java.io.ByteArrayOutputStream
import java.io.OutputStream
import kotlin.coroutines.coroutineContext

/*
 * Downloading a sealed file, as core's `Client::export_file_to` does (mero-js `client/files.ts`):
 * the node answers only ciphertext and the file's data key wrapped to a key made here for this
 * call, and nothing it says is believed until it checks out.
 */

/** Why an export was refused locally (mero-js `ExportErrorCode`). A node's own refusal stays its [CalimeroError]. */
enum class ExportErrorCode {
    /** The node's answer is malformed. */
    BAD_ANSWER,

    /** The node answered segments that are not the file asked for. */
    NOT_THE_FILE,

    /** A segment, chunk, the wrapped key, the size or the plaintext hash does not check out. */
    TAMPERED,

    /** A segment is longer than a whole segment. */
    TOO_LARGE,
}

/** An export that failed a check here, never one the node refused; messages carry no plaintext or keys. */
class ExportError(
    val code: ExportErrorCode,
    message: String,
) : Exception(message)

/** Where an export writes the plaintext, in order. */
fun interface FileSink {
    suspend fun write(chunk: ByteArray)

    companion object {
        /** A sink writing to [out] (a file, say). [out] is not closed. */
        fun of(out: OutputStream): FileSink = FileSink { out.write(it) }
    }
}

/** The largest file sent as one request, and the part size a session is asked for (mero-js `FILE_PART_BYTES`). */
const val FILE_PART_BYTES: Int = 16 * 1024 * 1024

/** A sealed file: [id] and [sha256] hex, [blobId] base58 (what apps print), [index] the log index that recorded it. */
@kotlinx.serialization.Serializable
data class FileInfo(
    val id: String,
    val blobId: String,
    val size: Long,
    val sha256: String,
    val index: Long = 0,
)

/** What an export wrote, as proven against the file's mark: its [size] and plaintext SHA-256 ([sha256], hex). */
data class ExportedFile(
    val size: Long,
    val sha256: String,
)

/** The two routes an export takes: `POST apps/{app}/files/{id}/export` and its segments, streamed. */
internal interface ExportTransport {
    /** The export answer, as JSON. */
    suspend fun export(
        app: String,
        id: String,
        recipient: String,
    ): JsonElement

    /** `GET apps/{app}/files/{id}/segments/{k}`: a successful response whose body is read here. */
    suspend fun segment(
        app: String,
        id: String,
        k: Int,
    ): Response
}

private class Answer(
    val app: String,
    val size: Long,
    val named: ByteArray,
    val segments: List<ByteArray>,
    val sha256: String,
    val ephemeral: ByteArray,
    val wrapped: ByteArray,
)

private fun bad(what: String): Nothing = throw ExportError(ExportErrorCode.BAD_ANSWER, "the node answered a bad $what")

private fun hexField(
    value: JsonElement?,
    label: String,
    bytes: Int? = null,
): ByteArray {
    val text = (value as? JsonPrimitive)?.takeIf { it.isString }?.content ?: bad(label)
    return try {
        if (bytes == null) Hex.decodeUnsized(text, label) else Hex.decode(text, label, bytes)
    } catch (_: IllegalArgumentException) {
        bad(label)
    }
}

/** The answer's fields, checked for shape; whether they name the requested file is checked after. */
private fun checked(answer: JsonElement): Answer {
    val a = answer as? JsonObject ?: bad("export")
    val app = (a["app"] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: bad("app")
    val size =
        (a["size"] as? JsonPrimitive)?.takeIf { !it.isString }?.longOrNull?.takeIf { it in 0..MAX_SAFE_INTEGER }
            ?: bad("size")
    val segments = a["segments"] as? JsonArray ?: bad("segment list")
    return Answer(
        app = app,
        size = size,
        named = hexField(a["id"], "file id", 32),
        segments = segments.map { hexField(it, "segment hash", 32) },
        sha256 = Hex.encode(hexField(a["sha256"], "sha256", 32)),
        ephemeral = hexField(a["ephemeral"], "ephemeral key", 32),
        wrapped = hexField(a["wrapped"], "wrapped key"),
    )
}

/** The largest size mero-js reads (`Number.isSafeInteger`), so both SDKs refuse the same answers. */
private const val MAX_SAFE_INTEGER = 9_007_199_254_740_991L

private fun tooLarge(): Nothing = throw ExportError(ExportErrorCode.TOO_LARGE, "a segment is longer than a whole segment")

/** A segment's bytes, refusing one longer than a whole segment before buffering it. Closes [response]. */
private fun readSegment(response: Response): ByteArray =
    response.use {
        val declared = it.header("content-length")?.toLongOrNull()
        if (declared != null && declared > SEGMENT_BYTES) tooLarge()
        val source = it.body?.source() ?: return ByteArray(0)
        val out = ByteArrayOutputStream()
        val buffer = ByteArray(READ_BYTES)
        while (true) {
            val n = source.read(buffer)
            if (n < 0) break
            if (out.size() + n > SEGMENT_BYTES) tooLarge()
            out.write(buffer, 0, n)
        }
        out.toByteArray()
    }

private const val READ_BYTES = 64 * 1024

/** [readSegment] off the caller's thread; cancelling the caller closes [response], which ends a stalled read. */
private suspend fun readCancellable(response: Response): ByteArray =
    coroutineScope {
        // The read never fails the scope: a read cut short by the close below must not stand in
        // for the cancellation.
        val read = async(Dispatchers.IO) { runCatching { readSegment(response) } }
        try {
            read.await().getOrThrow()
        } catch (e: CancellationException) {
            response.close()
            throw e
        }
    }

/**
 * Write the plaintext of sealed file [id] (64 hex) of app [app] to [sink], one segment at a time
 * (mero-js `exportFileTo`). Each segment is checked against its hash before it is opened, and the
 * segment list, size and plaintext hash against the file's id and mark at the end. On failure,
 * discard what [sink] received: it is authentic but may be a prefix of the file. A failed check is
 * an [ExportError]; a refusal by the node is its [CalimeroError]. [recipientSecret] pins the export
 * key, for the published vectors only.
 */
internal suspend fun exportFileTo(
    transport: ExportTransport,
    app: String,
    id: String,
    sink: FileSink,
    recipientSecret: ByteArray? = null,
): ExportedFile {
    coroutineContext.ensureActive()
    val recipient = X25519KeyPair.of(recipientSecret)
    val answer = checked(transport.export(app, id, Hex.encode(recipient.publicKey)))
    val segments = answer.segments
    if (segments.isEmpty() || Hex.encode(fileId(segments)) != Hex.encode(answer.named) || Hex.encode(answer.named) != id) {
        throw ExportError(ExportErrorCode.NOT_THE_FILE, "the node answered segments that are not the file $id")
    }
    val dataKey = unwrapExport(recipient, answer.ephemeral, answer.app, id, answer.wrapped)
    val reader = SegmentReader(dataKey, segments)
    for (k in segments.indices) {
        coroutineContext.ensureActive()
        reader.push(readCancellable(transport.segment(app, id, k))) { sink.write(it) }
    }
    val last = reader.finishExpecting(answer.size, answer.sha256)
    if (last.isNotEmpty()) sink.write(last)
    return ExportedFile(answer.size, answer.sha256)
}

/** The plaintext of sealed file [id] of app [app], in memory. Prefer [exportFileTo] for a large file. */
internal suspend fun exportFile(
    transport: ExportTransport,
    app: String,
    id: String,
    recipientSecret: ByteArray? = null,
): ByteArray {
    val out = ByteArrayOutputStream()
    exportFileTo(transport, app, id, { out.write(it) }, recipientSecret)
    return out.toByteArray()
}
