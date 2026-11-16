package com.calimero.mero.tee

import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.concat
import com.calimero.mero.crypto.randomBytes
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.put
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.Response
import okhttp3.ResponseBody.Companion.asResponseBody
import okio.Pipe
import okio.buffer
import java.util.concurrent.ConcurrentHashMap
import kotlin.concurrent.thread

internal class Answer(
    val status: Int,
    val headers: List<Pair<String, String>>,
    /** Body pieces, each sent as its own frame, [delayMs] apart. */
    val chunks: List<String>,
    val delayMs: Long = 0,
)

internal fun ok(body: String = "{}") = Answer(200, listOf("content-type" to "application/json"), listOf(body))

/** Answers the Noise handshake with its transport secret and seals its answers frame by frame, as core does. */
internal class StandInNode(
    val base: String = BASE,
    val answer: (JsonObject, String) -> Answer,
) {
    @Volatile var transport = X25519KeyPair.of()
    var handshakes = 0
    val requests: MutableList<Pair<JsonObject, String>> = java.util.Collections.synchronizedList(mutableListOf())
    private val sessions = ConcurrentHashMap<String, Pair<ByteArray, ByteArray>>()

    val transportKey: ByteArray get() = transport.publicKey

    fun restart() {
        transport = X25519KeyPair.of()
        sessions.clear()
    }

    fun forgetSessions() = sessions.clear()

    fun handle(
        request: Request,
        body: ByteArray,
    ): Response? =
        when (request.url.toString()) {
            // A current node serves the handshake at both paths (v2 is the deprecated alias).
            "$base$HANDSHAKE_PATH", "$base$LEGACY_HANDSHAKE_PATH" -> handshake(request, body)
            "$base$SEALED_PATH" -> exchange(request, body)
            else -> null
        }

    @Synchronized
    private fun handshake(
        request: Request,
        body: ByteArray,
    ): Response {
        if (!body.copyOfRange(1, 33).contentEquals(transport.publicKey)) return refusal(request, 409, "stale_transport_key")
        handshakes += 1
        val message1 = body.copyOfRange(33, body.size)
        val state = SymmetricState()
        state.mixHash("calimero/sealed-http/v2".encodeToByteArray())
        state.mixHash(transport.publicKey)
        val remote = message1.copyOfRange(0, 32)
        state.mixHash(remote)
        state.mixKey(transport.dh(remote))
        state.decryptAndHash(message1.copyOfRange(32, message1.size))
        val ephemeral = X25519KeyPair.of()
        state.mixHash(ephemeral.publicKey)
        state.mixKey(ephemeral.dh(remote))
        val sessionId = randomBytes(16)
        val message2 = concat(ephemeral.publicKey, state.encryptAndHash(concat(sessionId, byteArrayOf(0, 0, 0x0e, 0x10))))
        sessions[Hex.encode(sessionId)] = state.split()
        return FakeNet.respondBytes(request, 200, concat(byteArrayOf(2), message2), SEALED_CONTENT_TYPE)
    }

    private fun exchange(
        request: Request,
        envelope: ByteArray,
    ): Response {
        val header = envelope.copyOfRange(0, 25)
        val keys = sessions[Hex.encode(header.copyOfRange(1, 17))] ?: return refusal(request, 409, "unknown_session")
        val requestId = header.copyOfRange(17, 25)
        val iv = { index: Int -> concat(requestId, byteArrayOf(0, 0, 0, index.toByte())) }
        val inner = AesGcm.open(keys.first, iv(0), header, envelope.copyOfRange(25, envelope.size))
        val len = ((inner[0].toInt() and 0xff) shl 24) or ((inner[1].toInt() and 0xff) shl 16) or ((inner[2].toInt() and 0xff) shl 8) or (inner[3].toInt() and 0xff)
        val head = Json.parseToJsonElement(String(inner, 4, len)).jsonObject
        val text = String(inner, 4 + len, inner.size - 4 - len)
        requests += head to text
        val reply = answer(head, text)
        var index = 0
        val frame = { kind: Int, data: ByteArray ->
            val sealed = AesGcm.seal(keys.second, iv(index++), header, concat(byteArrayOf(kind.toByte()), data))
            concat(byteArrayOf((sealed.size ushr 24).toByte(), (sealed.size ushr 16).toByte(), (sealed.size ushr 8).toByte(), sealed.size.toByte()), sealed)
        }
        val headJson =
            buildJsonObject {
                put("status", reply.status)
                put("headers", JsonArray(reply.headers.map { (k, v) -> JsonArray(listOf(JsonPrimitive(k), JsonPrimitive(v))) }))
            }.toString()
        val pipe = Pipe(1 shl 20)
        val sink = pipe.sink.buffer()
        sink.write(frame(0, headJson.encodeToByteArray())).flush()
        thread {
            for (chunk in reply.chunks) {
                if (reply.delayMs > 0) Thread.sleep(reply.delayMs)
                sink.write(frame(1, chunk.encodeToByteArray())).flush()
            }
            sink.write(frame(2, ByteArray(0))).flush()
            sink.close()
        }
        return Response
            .Builder()
            .request(request)
            .protocol(Protocol.HTTP_1_1)
            .code(200)
            .message("")
            .header("content-type", SEALED_CONTENT_TYPE)
            .body(pipe.source.buffer().asResponseBody(SEALED_CONTENT_TYPE.toMediaType()))
            .build()
    }

    companion object {
        const val BASE = "https://tee-node.example/node"
    }
}

internal fun refusal(
    request: Request,
    status: Int,
    code: String,
): Response = FakeNet.respond(request, status, """{"error":{"code":"$code","message":"$code"}}""")
