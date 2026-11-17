package com.calimero.mero.tee

import com.calimero.mero.ExportError
import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.concat
import com.calimero.mero.crypto.sha256
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.ByteArrayOutputStream

/** mero-js `sealed/file.test.ts` and `file-multi-segment.test.ts`, over the same Rust vectors. */
class SealedFileTest {
    internal class Vector(
        private val o: JsonObject,
    ) {
        private fun s(key: String) = o.getValue(key).jsonPrimitive.content

        val name = s("name")
        val app = s("app")
        val id = s("id")
        val dataKey = Hex.decodeUnsized(s("dataKey"), "dataKey")
        val ephemeral = Hex.decodeUnsized(s("ephemeral"), "ephemeral")
        val recipientSecret = Hex.decodeUnsized(s("recipientSecret"), "secret")
        val recipientPublic = s("recipientPublic")
        val segment0 = Hex.decodeUnsized(s("segment0"), "segment")
        val segments = o.getValue("segments").jsonArray.map { it.jsonPrimitive.content }
        val size = o.getValue("size").jsonPrimitive.long
        val plaintextLen = o.getValue("plaintextLen").jsonPrimitive.int
        val plaintextSha256 = s("plaintextSha256")
        val wrapped = Hex.decodeUnsized(s("wrapped"), "wrapped")
        val dataKeyHex = s("dataKey")
    }

    internal companion object {
        val V: JsonObject by lazy { Json.parseToJsonElement(TeeFixtures.text("sealed-file-vectors.json")).jsonObject }
        val vectors: List<Vector> by lazy { V.getValue("vectors").jsonArray.map { Vector(it.jsonObject) } }
        val three get() = vectors[0]
        val full get() = vectors[1]
        val empty get() = vectors[2]

        fun plain(len: Int) = ByteArray(len) { (it % 251).toByte() }

        fun hashes(list: List<String>) = list.map { Hex.decode(it, "hash", 32) }
    }

    private inline fun <reified T : Throwable> refuses(
        message: String,
        block: () -> Unit,
    ) {
        try {
            block()
        } catch (e: Throwable) {
            if (e !is T) throw AssertionError("expected ${T::class.simpleName}, got $e", e)
            assertTrue("'${e.message}' does not contain '$message'", e.message.orEmpty().contains(message))
            return
        }
        fail("expected a refusal containing '$message'")
    }

    private fun read(
        v: Vector,
        segment: ByteArray,
        list: List<ByteArray>? = null,
    ): Pair<SegmentReader, List<ByteArray>> =
        runBlocking {
            val reader = SegmentReader(v.dataKey, list ?: listOf(sha256(segment)))
            val out = mutableListOf<ByteArray>()
            reader.push(segment) { out += it }
            reader to out
        }

    // ---------------------------------------------------------------- constants and ids

    @Test
    fun `has the reference segment size`() = assertEquals(V.getValue("segmentBytes").jsonPrimitive.int, SEGMENT_BYTES)

    @Test
    fun `derives file ids over one to three segments, order included`() {
        for (f in V.getValue("fileIds").jsonArray) {
            val segments =
                hashes(
                    f.jsonObject
                        .getValue("segments")
                        .jsonArray
                        .map { it.jsonPrimitive.content },
                )
            assertEquals(
                f.jsonObject
                    .getValue("id")
                    .jsonPrimitive.content,
                Hex.encode(fileId(segments)),
            )
        }
    }

    @Test
    fun `derives the id of every vector from its segment list`() {
        for (v in vectors) assertEquals(v.id, Hex.encode(fileId(hashes(v.segments))))
    }

    // ---------------------------------------------------------------- unwrapExport

    private fun open(
        v: Vector,
        appId: String = v.app,
        fileId: String = v.id,
        recipient: X25519KeyPair = X25519KeyPair.of(v.recipientSecret),
        ephemeral: ByteArray = v.ephemeral,
        wrapped: ByteArray = v.wrapped,
    ) = unwrapExport(recipient, ephemeral, appId, fileId, wrapped)

    @Test
    fun `opens the Rust wrap to the data key`() {
        for (v in vectors) {
            assertEquals(v.dataKeyHex, Hex.encode(open(v)))
            assertEquals(v.recipientPublic, Hex.encode(X25519KeyPair.of(v.recipientSecret).publicKey))
        }
    }

    @Test
    fun `does not open for another app, another file id or another key`() {
        refuses<ExportError>("does not open") { open(three, appId = "other") }
        refuses<ExportError>("does not open") { open(three, fileId = full.id) }
        refuses<ExportError>("does not open") { open(three, recipient = X25519KeyPair.of()) }
    }

    @Test
    fun `does not open when ids differ only in where they split`() {
        refuses<ExportError>("does not open") { open(three, appId = "${three.app}\u0000${three.id}", fileId = "") }
    }

    @Test
    fun `refuses a truncated, tampered or low-order wrap`() {
        refuses<ExportError>("truncated") { open(three, wrapped = three.wrapped.copyOfRange(0, 5)) }
        val flipped = three.wrapped.copyOf().also { it[it.size - 1] = (it[it.size - 1].toInt() xor 1).toByte() }
        refuses<ExportError>("does not open") { open(three, wrapped = flipped) }
        refuses<ExportError>("low order") { open(three, ephemeral = ByteArray(32)) }
    }

    // ---------------------------------------------------------------- SegmentReader

    @Test
    fun `opens each Rust vector to its plaintext, size and hash`() {
        for (v in vectors) {
            val (reader, out) = read(v, v.segment0)
            val last = reader.finishExpecting(v.size, v.plaintextSha256)
            val joined = ByteArrayOutputStream().apply { out.forEach(::write) }.toByteArray() + last
            assertArrayEquals(v.name, plain(v.plaintextLen), joined)
        }
    }

    @Test
    fun `holds the last chunk back even when it is full`() {
        assertEquals(emptyList<ByteArray>(), read(full, full.segment0).second)
    }

    @Test
    fun `refuses a segment that is not the one listed, before opening any of it`() =
        runBlocking {
            val reader = SegmentReader(three.dataKey, listOf(sha256(three.segment0)))
            val tampered = three.segment0.copyOf().also { it[it.size - 1] = (it[it.size - 1].toInt() xor 1).toByte() }
            val out = mutableListOf<ByteArray>()
            refuses<ExportError>("is not the one the file lists") { runBlocking { reader.push(tampered) { out += it } } }
            assertTrue(out.isEmpty())
        }

    @Test
    fun `refuses a short non-last segment and a segment past the list`() =
        runBlocking {
            val hash = sha256(three.segment0)
            val reader = SegmentReader(three.dataKey, listOf(hash, hash))
            refuses<ExportError>("not a whole segment") { runBlocking { reader.push(three.segment0) {} } }
            val single = SegmentReader(three.dataKey, listOf(hash))
            single.push(three.segment0) {}
            refuses<ExportError>("no segment 1") { runBlocking { single.push(three.segment0) {} } }
        }

    @Test
    fun `refuses a missing segment`() {
        val reader = SegmentReader(three.dataKey, listOf(ByteArray(32), ByteArray(32)))
        refuses<ExportError>("missing segments") { reader.finishExpecting(0, "") }
    }

    @Test
    fun `refuses a wrong size or plaintext hash`() {
        read(three, three.segment0).first.finishExpecting(three.size, three.plaintextSha256)
        refuses<ExportError>("not the one that was sealed") { read(three, three.segment0).first.finishExpecting(three.size + 1, three.plaintextSha256) }
        refuses<ExportError>("not the one that was sealed") { read(three, three.segment0).first.finishExpecting(three.size, "00".repeat(32)) }
    }

    @Test
    fun `refuses a file cut after a chunk (the last chunk was not sealed as last)`() {
        val sealed = 65536 + 16
        for (keep in listOf(2 * sealed, sealed)) {
            val (reader) = read(three, three.segment0.copyOfRange(0, keep))
            refuses<ExportError>("does not open") { reader.finishExpecting(0, "") }
        }
    }

    @Test
    fun `refuses a stream shorter than one tag`() {
        val (reader) = read(three, ByteArray(0))
        refuses<ExportError>("truncated") { reader.finishExpecting(0, "") }
    }

    @Test
    fun `refuses a dropped middle chunk`() {
        val sealed = 65536 + 16
        val s = three.segment0
        val (reader) = read(three, concat(s.copyOfRange(0, sealed), s.copyOfRange(2 * sealed, s.size)))
        refuses<ExportError>("does not open") { reader.finishExpecting(0, "") }
    }

    // ---------------------------------------------------------------- multi-segment files

    /** The sealed stream the Rust writer makes: whole chunks, and [more] seals a full final chunk as not last. */
    private fun seal(
        key: ByteArray,
        len: Int,
        more: Boolean,
    ): ByteArray {
        val chunk = 65536
        val plain = plain(len)
        val chunks = maxOf(1, (len + chunk - 1) / chunk + if (more && len % chunk == 0) 1 else 0)
        val out = ByteArrayOutputStream()
        for (i in 0 until chunks) {
            val iv = ByteArray(12)
            for (b in 0 until 8) iv[4 + b] = (i.toLong() ushr (8 * (7 - b))).toByte()
            val piece = plain.copyOfRange(minOf(len, i * chunk), minOf(len, (i + 1) * chunk))
            out.write(AesGcm.seal(key, iv, "calimero.file.v1".encodeToByteArray() + byteArrayOf(if (i == chunks - 1) 1 else 0), piece))
        }
        return out.toByteArray()
    }

    @Test
    fun `multi-segment files match the Rust reference`() =
        runBlocking {
            val multi = Json.parseToJsonElement(TeeFixtures.text("sealed-file-multi-segment-vectors.json")) as JsonArray
            for (e in multi) {
                val v = e.jsonObject
                val name = v.getValue("name").jsonPrimitive.content
                val len = v.getValue("len").jsonPrimitive.int
                val key = Hex.decodeUnsized(v.getValue("key").jsonPrimitive.content, "key")
                val listed = v.getValue("segments").jsonArray.map { it.jsonPrimitive.content }
                val all = seal(key, len, v.getValue("more").jsonPrimitive.boolean)
                val segments = (all.indices step SEGMENT_BYTES).map { all.copyOfRange(it, minOf(all.size, it + SEGMENT_BYTES)) }
                assertEquals(name, listed, segments.map { Hex.encode(sha256(it)) })
                assertEquals(name, v.getValue("lastLen").jsonPrimitive.int, segments.last().size)
                assertEquals(name, v.getValue("id").jsonPrimitive.content, Hex.encode(fileId(hashes(listed))))

                val reader = SegmentReader(key, hashes(listed))
                var count = 0
                var ok = true
                val check = { c: ByteArray ->
                    for (b in c) {
                        if (b != (count % 251).toByte()) ok = false
                        count += 1
                    }
                }
                for (segment in segments) reader.push(segment) { check(it) }
                check(reader.finishExpecting(v.getValue("size").jsonPrimitive.long, v.getValue("sha256").jsonPrimitive.content))
                assertEquals(name, len, count)
                assertTrue(name, ok)
            }
        }
}
