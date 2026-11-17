package com.calimero.mero

import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.sha256
import com.calimero.mero.tee.SEGMENT_BYTES
import com.calimero.mero.tee.SealedFileTest
import com.calimero.mero.tee.fileId
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.Response
import okhttp3.ResponseBody.Companion.asResponseBody
import okio.Buffer
import okio.Source
import okio.Timeout
import okio.buffer
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayOutputStream

/** mero-js `client/export-file.test.ts`: [exportFileTo] against a node holding one Rust vector. */
class ExportFileTest {
    private val three get() = SealedFileTest.three
    private val full get() = SealedFileTest.full
    private val empty get() = SealedFileTest.empty

    private fun response(
        body: Source,
        headers: Map<String, String> = emptyMap(),
    ): Response =
        Response
            .Builder()
            .request(Request.Builder().url("http://node.test/").build())
            .protocol(Protocol.HTTP_1_1)
            .code(200)
            .message("")
            .apply { headers.forEach { (k, v) -> header(k, v) } }
            .body(body.buffer().asResponseBody(null, headers["content-length"]?.toLong() ?: -1))
            .build()

    private fun bytes(data: ByteArray): Source = Buffer().write(data)

    /** A node holding [v]: its export answer (changeable with [change]) and its segment. */
    private inner class Node(
        val v: SealedFileTest.Vector,
        change: (MutableMap<String, JsonElement>) -> Unit = {},
        val segment: ByteArray = v.segment0,
        var respond: () -> Response = { response(bytes(segment)) },
    ) : ExportTransport {
        val calls = mutableListOf<String>()
        var recipient: String? = null
        var answer: JsonElement =
            mutableMapOf<String, JsonElement>(
                "app" to JsonPrimitive(v.app),
                "id" to JsonPrimitive(v.id),
                "size" to JsonPrimitive(v.size),
                "sha256" to JsonPrimitive(v.plaintextSha256),
                "segments" to JsonArray(v.segments.map(::JsonPrimitive)),
                "ephemeral" to JsonPrimitive(Hex.encode(v.ephemeral)),
                "wrapped" to JsonPrimitive(Hex.encode(v.wrapped)),
            ).also(change).let(::JsonObject)
        var exportOverride: (suspend () -> JsonElement)? = null
        var segmentOverride: (suspend () -> Response)? = null

        override suspend fun export(
            app: String,
            id: String,
            recipient: String,
        ): JsonElement {
            calls += "POST apps/$app/files/$id/export"
            this.recipient = recipient
            return exportOverride?.invoke() ?: answer
        }

        override suspend fun segment(
            app: String,
            id: String,
            k: Int,
        ): Response {
            calls += "GET apps/$app/files/$id/segments/$k"
            return segmentOverride?.invoke() ?: respond()
        }
    }

    private class Sink : FileSink {
        val chunks = mutableListOf<ByteArray>()

        override suspend fun write(chunk: ByteArray) {
            chunks += chunk
        }

        fun joined(): ByteArray = ByteArrayOutputStream().apply { chunks.forEach(::write) }.toByteArray()
    }

    private fun run(
        n: Node,
        v: SealedFileTest.Vector = n.v,
        out: Sink = Sink(),
        id: String = v.id,
    ) = runBlocking { exportFileTo(n, v.app, id, out, v.recipientSecret) }

    private fun codeOf(block: () -> Unit): String =
        try {
            block()
            "resolved"
        } catch (e: ExportError) {
            e.code.name
        } catch (e: Exception) {
            "not an ExportError: $e"
        }

    private fun messageOf(block: () -> Unit): String =
        try {
            block()
            "resolved"
        } catch (e: Exception) {
            e.message.orEmpty()
        }

    private fun assertRefused(
        message: String,
        block: () -> Unit,
    ) {
        val got = messageOf(block)
        assertTrue("'$got' does not contain '$message'", got.contains(message))
    }

    @Test
    fun `decrypts the Rust vectors to exactly their plaintext`() {
        for (v in SealedFileTest.vectors) {
            val n = Node(v)
            val out = Sink()
            assertEquals(ExportedFile(v.size, v.plaintextSha256), run(n, v, out))
            assertArrayEquals(v.name, SealedFileTest.plain(v.plaintextLen), out.joined())
            assertEquals(v.plaintextSha256, Hex.encode(sha256(out.joined())))
            assertEquals(listOf("POST apps/${v.app}/files/${v.id}/export", "GET apps/${v.app}/files/${v.id}/segments/0"), n.calls)
            assertEquals(v.recipientPublic, n.recipient)
        }
    }

    @Test
    fun `hands the sink chunks of at most 64 KiB, in order`() {
        val out = Sink()
        run(Node(three), out = out)
        assertEquals(listOf(65536, 65536, 5), out.chunks.map { it.size })
    }

    @Test
    fun `writes nothing for an empty file`() {
        val out = Sink()
        run(Node(empty), out = out)
        assertTrue(out.chunks.isEmpty())
    }

    @Test
    fun `works with a fresh export key, which cannot open a wrap made for another`() {
        assertRefused("does not open") { runBlocking { exportFileTo(Node(three), three.app, three.id, Sink()) } }
        assertRefused("does not open") { runBlocking { exportFile(Node(three), three.app, three.id) } }
    }

    @Test
    fun `exportFile returns the bytes`() {
        val bytes = runBlocking { exportFile(Node(three), three.app, three.id, three.recipientSecret) }
        assertArrayEquals(SealedFileTest.plain(three.plaintextLen), bytes)
    }

    @Test
    fun `fails a tampered segment before any of its bytes reach the sink`() {
        val tampered = three.segment0.copyOf().also { it[3] = (it[3].toInt() xor 1).toByte() }
        val out = Sink()
        assertRefused("is not the one the file lists") { run(Node(three, segment = tampered), out = out) }
        assertTrue(out.chunks.isEmpty())
    }

    @Test
    fun `fails a segment list that does not hash to the id`() {
        val n = Node(three, { it["segments"] = JsonArray(listOf(JsonPrimitive("07".repeat(32)))) })
        assertRefused("are not the file") { run(n) }
        assertEquals(1, n.calls.size)
    }

    @Test
    fun `fails an answer that names another file than the one asked for`() {
        assertRefused("are not the file") { run(Node(three), id = full.id) }
    }

    @Test
    fun `fails an empty segment list even when it hashes to the requested id`() {
        val id = Hex.encode(fileId(emptyList()))
        val n =
            Node(three, {
                it["segments"] = JsonArray(emptyList())
                it["id"] = JsonPrimitive(id)
            })
        assertRefused("are not the file") { run(n, id = id) }
    }

    @Test
    fun `fails a consistent answer for a file other than the one asked for`() {
        val other = ByteArray(32) { 9 }
        val n =
            Node(three, {
                it["segments"] = JsonArray(listOf(JsonPrimitive(Hex.encode(other))))
                it["id"] = JsonPrimitive(Hex.encode(fileId(listOf(other))))
            })
        assertRefused("are not the file") { run(n) }
    }

    @Test
    fun `fails a wrong final size, withholding the last chunk`() {
        val out = Sink()
        assertRefused("not the one that was sealed") { run(Node(three, { it["size"] = JsonPrimitive(three.size + 1) }), out = out) }
        assertEquals(2 * 65536, out.joined().size)
    }

    @Test
    fun `fails a wrong final SHA-256`() {
        assertRefused("not the one that was sealed") { run(Node(three, { it["sha256"] = JsonPrimitive("00".repeat(32)) })) }
    }

    @Test
    fun `fails a wrap made for another app`() {
        val n = Node(three, { it["app"] = JsonPrimitive("another-app") })
        assertRefused("does not open") { run(n) }
        assertEquals(1, n.calls.size)
    }

    @Test
    fun `fails a malformed answer`() {
        assertRefused("ephemeral key") { run(Node(three, { it["ephemeral"] = JsonPrimitive("zz") })) }
        assertRefused("sha256") { run(Node(three, { it["sha256"] = JsonPrimitive("ab") })) }
        assertRefused("bad size") { run(Node(three, { it["size"] = JsonPrimitive(-1) })) }
        assertRefused("bad size") { run(Node(three, { it["size"] = JsonPrimitive(1L shl 60) })) }
    }

    // ---------------------------------------------------------------- bounded segment reads

    /** A body of [chunk]-sized pieces, forever, counting reads and noting a close. */
    private class Endless(
        val chunk: Int,
        val delayMs: Long = 0,
    ) : Source {
        @Volatile var reads = 0

        @Volatile var closed = false

        override fun read(
            sink: Buffer,
            byteCount: Long,
        ): Long {
            if (closed) throw java.io.IOException("closed")
            reads += 1
            if (delayMs > 0) Thread.sleep(delayMs)
            val n = minOf(byteCount, chunk.toLong())
            sink.write(ByteArray(n.toInt()))
            return n
        }

        override fun timeout(): Timeout = Timeout.NONE

        override fun close() {
            closed = true
        }
    }

    @Test
    fun `refuses a declared length past a whole segment without reading the body`() {
        val s = Endless(1024)
        val out = Sink()
        val n = Node(three, respond = { response(s, mapOf("content-length" to (SEGMENT_BYTES + 1).toString())) })
        assertEquals("TOO_LARGE", codeOf { run(n, out = out) })
        assertTrue(s.reads <= 1)
        assertTrue(s.closed)
        assertTrue(out.chunks.isEmpty())
    }

    @Test
    fun `stops reading a stream that runs past a whole segment, the last segment included`() {
        val s = Endless(1 shl 20)
        val n = Node(three, respond = { response(s) })
        assertEquals("TOO_LARGE", codeOf { run(n) })
        assertTrue("${s.reads} reads", s.reads < 3_000)
        assertTrue(s.closed)
    }

    // ---------------------------------------------------------------- cancellation

    @Test
    fun `stops a download that never ends when the coroutine is cancelled`() =
        runBlocking {
            val s = Endless(16, delayMs = 5)
            val n = Node(three, respond = { response(s) })
            val started = System.nanoTime()
            val outcome =
                try {
                    withTimeout(200) { exportFileTo(n, three.app, three.id, Sink(), three.recipientSecret) }
                    "resolved"
                } catch (e: CancellationException) {
                    "cancelled"
                }
            assertEquals("cancelled", outcome)
            assertTrue((System.nanoTime() - started) / 1_000_000 < 5_000)
            delay(50)
            assertTrue(s.closed)
        }

    @Test
    fun `does not start when already cancelled`() =
        runBlocking {
            val n = Node(three)
            val job =
                async(Dispatchers.Default) {
                    kotlinx.coroutines.currentCoroutineContext().let { (it[kotlinx.coroutines.Job])!!.cancel() }
                    exportFileTo(n, three.app, three.id, Sink(), three.recipientSecret)
                }
            val outcome =
                try {
                    job.await()
                    "resolved"
                } catch (_: CancellationException) {
                    "cancelled"
                }
            assertEquals("cancelled", outcome)
            assertTrue(n.calls.isEmpty())
        }

    // ---------------------------------------------------------------- error codes

    @Test
    fun `names the check that failed`() {
        val tampered = three.segment0.copyOf().also { it[3] = (it[3].toInt() xor 1).toByte() }
        assertEquals("TAMPERED", codeOf { run(Node(three, segment = tampered)) })
        assertEquals("TAMPERED", codeOf { run(Node(three, { it["size"] = JsonPrimitive(1) })) })
        assertEquals("TAMPERED", codeOf { run(Node(three, { it["app"] = JsonPrimitive("x") })) })
        assertEquals("NOT_THE_FILE", codeOf { run(Node(three, { it["segments"] = JsonArray(listOf(JsonPrimitive("00".repeat(32)))) })) })
    }

    @Test
    fun `turns a malformed answer into BAD_ANSWER`() {
        val bad: List<(MutableMap<String, JsonElement>) -> Unit> =
            listOf(
                { it["segments"] = JsonPrimitive("nope") },
                { it["segments"] = JsonArray(listOf(JsonPrimitive(7))) },
                { it["segments"] = JsonArray(listOf(JsonPrimitive("zz"))) },
                { it["app"] = JsonPrimitive(5) },
                { it.remove("id") },
                { it["size"] = JsonPrimitive("3") },
                { it["size"] = JsonPrimitive(1.5) },
                { it["sha256"] = JsonNull },
                { it["ephemeral"] = JsonPrimitive("ab") },
                { it["wrapped"] = JsonPrimitive("abc") },
                { it.remove("wrapped") },
            )
        for (change in bad) assertEquals("BAD_ANSWER", codeOf { run(Node(three, change)) })
        val n = Node(three).also { it.exportOverride = { JsonNull } }
        assertEquals("BAD_ANSWER", codeOf { run(n) })
    }

    @Test
    fun `leaves the node's own refusal as its error`() {
        val n = Node(three).also { it.exportOverride = { throw NotFound("no such file") } }
        assertTrue(codeOf { run(n) }.startsWith("not an ExportError"))
    }
}
