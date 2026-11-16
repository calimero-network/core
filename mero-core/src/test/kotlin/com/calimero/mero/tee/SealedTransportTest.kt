package com.calimero.mero.tee

import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.concat
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.ResponseBody.Companion.asResponseBody
import okio.Buffer
import okio.Source
import okio.Timeout
import okio.buffer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Port of mero-js `src/sealed/sealed.test.ts`, pinned to core's published vectors. */
class SealedTransportTest {
    // Core's published vectors (`crates/server/src/sealed/tests.rs` and
    // `crates/tee-attestation/src/generate.rs`), repeated verbatim.
    private val transportPublicVector = "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20"
    private val handshakeRequestVector =
        "020faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f207b0d47d93427f831116078" +
            "1c7c733fd89f88970aef490d8aa0ee19a4cb8a1b1461fba335b4f3e9be8a34a61900e5b4be"
    private val handshakeResponseVector =
        "02ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b7d6339b7296bdc4778248f" +
            "57126a10ee5b7fb39b0cb104dcbfcaaaf44d0e6017379638a4"
    private val sealedRequestVector =
        "025555555555555555555555555555555500000000000000019fa4301264a48bdf6811cfb1d7c3dad713ede5" +
            "ea1820608358ff2779cee1605efc3d97879f24d6cf302a7877beda552ca6714ef9d5496f978fdcdde522435c" +
            "f663e4be5f60170e8eab74161a913608fe268e5352fcd3090d62e2a12c92b06b51885d43c4d83dcfe544"
    private val sealedResponseVector =
        "0000004f4873ec5a589a1e3585360440100bc00d20522fe35bac2221dcfe80341ce3069481d4794b801776f4" +
            "a76188b81887eb83cfccd2981229af48a3820df924261b3ff12d4b29340ca07c392ed4e0ee4bed0000001cfa" +
            "20a7633655179808da19e2fcbc81bc219262fc4f5406582834f549000000111426897864d7a92eec9261fe29" +
            "93415592"
    private val transportBindingVector = "30274595433e8afc5d4035e30a2b599d93caf0d470867527f13dc6af92fa12a8"

    private val prologue = "calimero/sealed-http/v2".encodeToByteArray()
    private val transportKey = Hex.decode(transportPublicVector, "transport", 32)
    private val vectorHead = SealedTransport.head("POST", "/jsonrpc", listOf("content-type" to "application/json"))
    private val anyRequest = Request.Builder().url("https://tee-node.example/node/jsonrpc").build()

    /** The vector session: the handshake with core's fixed client ephemeral key. */
    private fun vectorSession(): Pair<NoiseInitiator, Session> {
        val noise = NoiseInitiator.initiate(transportKey, prologue, ByteArray(32) { 0x33 })
        return noise to sessionFrom(noise, Hex.decodeUnsized(handshakeResponseVector, "response"))
    }

    private fun streamOf(bytes: ByteArray): Response = FakeNet.respondBytes(anyRequest, 200, bytes, SEALED_CONTENT_TYPE)

    // ---- wire format -------------------------------------------------------------------------

    @Test
    fun `opens the handshake exactly as the node expects`() {
        val (noise, _) = vectorSession()
        assertEquals(handshakeRequestVector, Hex.encode(concat(byteArrayOf(2), transportKey, noise.message1)))
    }

    @Test
    fun `seals a request under the session exactly as the node expects`() {
        val (_, session) = vectorSession()
        assertEquals(sealedRequestVector, Hex.encode(sealRequest(session, vectorHead, "{}".encodeToByteArray()).envelope))
    }

    @Test
    fun `opens the frames of a response the node sealed`() {
        val (_, session) = vectorSession()
        val sealed = sealRequest(session, vectorHead, ByteArray(0))
        val response =
            openResponse(streamOf(Hex.decodeUnsized(sealedResponseVector, "r")), session.responseKey, sealed.header, sealed.requestId, anyRequest)
        assertEquals(200, response.code)
        assertEquals("application/json", response.header("content-type"))
        assertEquals("""{"ok":true}""", response.body!!.string())
    }

    @Test
    fun `refuses a handshake reply from anyone but the attested node`() {
        val noise = NoiseInitiator.initiate(transportKey, prologue, ByteArray(32) { 0x33 })
        val forged = Hex.decodeUnsized(handshakeResponseVector, "r")
        forged[forged.size - 1] = (forged.last().toInt() xor 1).toByte()
        assertRefuses("did not come from the attested node") { sessionFrom(noise, forged) }
    }

    @Test
    fun `refuses a response frame that anyone but the node sealed`() {
        val (_, session) = vectorSession()
        val sealed = sealRequest(session, vectorHead, ByteArray(0))
        val forged = Hex.decodeUnsized(sealedResponseVector, "r")
        forged[forged.size - 1] = (forged.last().toInt() xor 1).toByte()
        val response = openResponse(streamOf(forged), session.responseKey, sealed.header, sealed.requestId, anyRequest)
        assertRefuses("did not open") { response.body!!.string() }
    }

    @Test
    fun `notices a response cut short before its end frame`() {
        val (_, session) = vectorSession()
        val sealed = sealRequest(session, vectorHead, ByteArray(0))
        val whole = Hex.decodeUnsized(sealedResponseVector, "r")
        val endFrame = 4 + 1 + 16
        val response = openResponse(streamOf(whole.copyOfRange(0, whole.size - endFrame)), session.responseKey, sealed.header, sealed.requestId, anyRequest)
        assertRefuses("cut short") { response.body!!.string() }
    }

    @Test
    fun `refuses a frame larger than the node ever sends, before buffering it`() {
        val (_, session) = vectorSession()
        val sealed = sealRequest(session, vectorHead, ByteArray(0))
        var pulled = 0
        val endless =
            object : Source {
                override fun read(
                    sink: Buffer,
                    byteCount: Long,
                ): Long {
                    pulled += 1
                    val chunk = if (pulled == 1) byteArrayOf(-1, -1, -1, -1) else ByteArray(1024)
                    sink.write(chunk)
                    return chunk.size.toLong()
                }

                override fun timeout() = Timeout.NONE

                override fun close() = Unit
            }
        val response =
            Response
                .Builder()
                .request(anyRequest)
                .protocol(Protocol.HTTP_1_1)
                .code(200)
                .message("")
                .header("content-type", SEALED_CONTENT_TYPE)
                .body(endless.buffer().asResponseBody(SEALED_CONTENT_TYPE.toMediaType()))
                .build()
        assertRefuses("larger than the node sends") { openResponse(response, session.responseKey, sealed.header, sealed.requestId, anyRequest) }
        assertTrue(pulled < 3)
    }

    @Test
    fun `computes the transport binding the node puts in its quote`() {
        assertEquals(transportBindingVector, Hex.encode(transportKeyBinding(ByteArray(32) { 0x11 }, ByteArray(32) { 0x22 })))
    }

    // ---- a stand-in node ---------------------------------------------------------------------

    private val base = StandInNode.BASE

    private fun get(
        client: OkHttpClient,
        path: String,
    ): Response = client.newCall(Request.Builder().url("$base$path").build()).execute()

    private fun latin1(net: FakeNet) = net.bodies.joinToString("") { String(it, Charsets.ISO_8859_1) }

    // ---- SealedTransport ---------------------------------------------------------------------

    @Test
    fun `delivers the whole request to the node and returns its answer as a Response`() {
        val node = StandInNode { _, _ -> Answer(201, listOf("x-answer" to "yes"), listOf("{\"result\":", "42}")) }
        val net = FakeNet(node::handle)
        val sealed = SealedTransport(base, node.transportKey, net.client)

        val response =
            sealed.client
                .newCall(
                    Request
                        .Builder()
                        .url("$base/jsonrpc?x=1")
                        .header("Authorization", "Bearer secret-token")
                        .post("""{"method":"secret-call"}""".toRequestBody("application/json".toMediaType()))
                        .build(),
                ).execute()

        val (head, body) = node.requests[0]
        assertEquals("POST", head["method"]!!.jsonPrimitive.content)
        assertEquals("/jsonrpc?x=1", head["path"]!!.jsonPrimitive.content)
        val headers = (head["headers"] as JsonArray).map { (it as JsonArray).let { p -> p[0].jsonPrimitive.content to p[1].jsonPrimitive.content } }
        assertTrue(("authorization" to "Bearer secret-token") in headers)
        assertTrue(headers.any { it.first == "content-type" && it.second.startsWith("application/json") })
        assertEquals("""{"method":"secret-call"}""", body)
        assertEquals(201, response.code)
        assertEquals("yes", response.header("x-answer"))
        assertEquals(
            42,
            Json
                .parseToJsonElement(response.body!!.string())
                .jsonObject["result"]!!
                .jsonPrimitive.int,
        )

        val wire = latin1(net)
        assertFalse(wire.contains("secret-token"))
        assertFalse(wire.contains("secret-call"))
        assertFalse(wire.contains("jsonrpc"))
    }

    @Test
    fun `hands over a streamed body frame by frame, before the stream ends`() {
        val node = StandInNode { _, _ -> Answer(200, listOf("content-type" to "text/event-stream"), listOf("data: one\n\n", "data: two\n\n"), 200) }
        val sealed = SealedTransport(base, node.transportKey, FakeNet(node::handle).client)
        val response =
            sealed.client
                .newCall(
                    Request
                        .Builder()
                        .url("$base/sse")
                        .header("accept", "text/event-stream")
                        .build(),
                ).execute()
        assertEquals("text/event-stream", response.header("content-type"))
        val source = response.body!!.source()
        val started = System.currentTimeMillis()
        assertEquals("data: one", source.readUtf8LineStrict())
        // The first event arrived before the second was even sent.
        assertTrue(System.currentTimeMillis() - started < 350)
        assertEquals("", source.readUtf8LineStrict())
        assertEquals("data: two", source.readUtf8LineStrict())
        assertEquals("", source.readUtf8LineStrict())
        assertTrue(source.exhausted())
    }

    @Test
    fun `opens the session at the unversioned handshake path`() {
        val node = StandInNode { _, _ -> ok() }
        val net = FakeNet(node::handle)
        val sealed = SealedTransport(base, node.transportKey, net.client)
        get(sealed.client, "/admin-api/health").use { assertEquals(200, it.code) }
        assertEquals("/sealed/handshake", HANDSHAKE_PATH)
        assertEquals(listOf("$base/sealed/handshake", "$base$SEALED_PATH"), net.requests.toList())
    }

    @Test
    fun `opens the session at the v2 path when the node answers 404 or 405 at the new one`() {
        // A node released before /sealed/handshake routes only the v2 path.
        for (status in listOf(404, 405)) {
            val node = StandInNode { _, _ -> ok() }
            val net =
                FakeNet { request, body ->
                    if (request.url.toString() == "$base$HANDSHAKE_PATH") {
                        FakeNet.respond(request, status, "Not Found", "text/plain")
                    } else {
                        node.handle(request, body)
                    }
                }
            val sealed = SealedTransport(base, node.transportKey, net.client)
            get(sealed.client, "/admin-api/health").use { assertEquals("$status", 200, it.code) }
            assertEquals(
                "$status",
                listOf("$base/sealed/handshake", "$base/sealed/v2/handshake", "$base$SEALED_PATH"),
                net.requests.toList(),
            )
            assertEquals("$status", 1, node.handshakes)
        }
    }

    @Test
    fun `opens the session at the v2 path when a sealed-only node answers 403 sealed_required`() {
        // An older node that seals everything refuses an unknown path as unsealed.
        val node = StandInNode { _, _ -> ok() }
        val net =
            FakeNet { request, body ->
                if (request.url.toString() == "$base$HANDSHAKE_PATH") {
                    refusal(request, 403, "sealed_required")
                } else {
                    node.handle(request, body)
                }
            }
        val sealed = SealedTransport(base, node.transportKey, net.client)
        get(sealed.client, "/admin-api/health").use { assertEquals(200, it.code) }
        assertEquals(
            listOf("$base/sealed/handshake", "$base/sealed/v2/handshake", "$base$SEALED_PATH"),
            net.requests.toList(),
        )
    }

    @Test
    fun `reports a 404 from the v2 handshake path too, without trying again`() {
        val net = FakeNet { request, _ -> FakeNet.respond(request, 404, "Not Found", "text/plain") }
        val sealed = SealedTransport(base, transportKey, net.client)
        val error = assertRefuses("HTTP 404") { get(sealed.client, "/admin-api/health") }
        assertEquals(404, (error as SealedTransportException).status)
        assertEquals(listOf("$base$HANDSHAKE_PATH", "$base$LEGACY_HANDSHAKE_PATH"), net.requests.toList())
    }

    @Test
    fun `does not try the v2 handshake path on any other refusal`() {
        for ((status, code) in listOf(400 to "malformed", 403 to "forbidden", 409 to "stale_transport_key", 500 to "internal")) {
            val net = FakeNet { request, _ -> refusal(request, status, code) }
            val sealed = SealedTransport(base, transportKey, net.client)
            assertRefuses("") { get(sealed.client, "/admin-api/health") }
            assertEquals("$status", listOf("$base$HANDSHAKE_PATH"), net.requests.toList())
        }
    }

    @Test
    fun `never sends a request outside the node in the clear`() {
        val carrier = FakeNet { _, _ -> error("sent") }
        val sealed = SealedTransport(base, transportKey, carrier.client)
        assertRefuses("Refusing") { sealed.client.newCall(Request.Builder().url("https://elsewhere.example/node/jsonrpc").build()).execute() }
        assertRefuses("Refusing") { sealed.client.newCall(Request.Builder().url("https://tee-node.example/other/jsonrpc").build()).execute() }
        assertTrue(carrier.requests.isEmpty())
    }
}
