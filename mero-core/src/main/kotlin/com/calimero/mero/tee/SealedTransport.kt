package com.calimero.mero.tee

import com.calimero.mero.crypto.concat
import com.calimero.mero.crypto.sha256
import com.calimero.mero.http.PlainJsonHttp
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.add
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import okhttp3.Headers
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.Interceptor
import okhttp3.MediaType
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.MediaType.Companion.toMediaTypeOrNull
import okhttp3.OkHttpClient
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.ResponseBody
import okhttp3.ResponseBody.Companion.toResponseBody
import okio.Buffer
import okio.BufferedSource
import okio.Source
import okio.Timeout
import okio.buffer
import java.io.IOException
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong

/*
 * Sealed transport: requests encrypted end to end to a TEE node's attested key — port of
 * mero-js `src/sealed/sealed.ts`, wire-identical with core's `crates/server/src/sealed.rs`:
 *
 *   handshake   Noise_NK_25519_AESGCM_SHA256, prologue "calimero/sealed-http/v2"
 *     request   POST /sealed/handshake      0x02 ‖ transport_pk ‖ Noise message 1
 *               (nodes before it served only /sealed/v2/handshake; see LEGACY_HANDSHAKE_PATH)
 *     response  0x02 ‖ Noise message 2 (payload = session_id(16) ‖ lifetime secs, u32 BE)
 *     keys      (req key, resp key) = Split()
 *   exchange    POST /sealed/v2
 *     header    0x02 ‖ session_id(16) ‖ request_id (u64 BE; from 1, each used once)
 *     request   header ‖ AES-256-GCM(req key, nonce = request_id ‖ 0u32, aad = header, inner)
 *     response  frames, each u32 BE length ‖ AES-256-GCM(resp key,
 *               nonce = request_id ‖ frame index (u32 BE, from 0), aad = header, kind ‖ data)
 *               kind 0 = head (JSON), 1 = body bytes, 2 = end; no end frame = cut short
 *     inner     u32 BE head length ‖ head (JSON) ‖ body
 *
 * A node speaks plain HTTP and any TLS in front of it ends outside the enclave, at whatever
 * proxy or relay the operator runs, so everything crossing that hop (the bearer token,
 * a call's arguments, results) is readable there. Sealing encrypts it so that only the
 * attested TD can read it, and only the TD can produce a response the client accepts. The
 * session keys come from both sides' ephemeral keys, so once a session is gone (an hour at
 * most) what was sent in it cannot be opened, not even with the transport key. Responses
 * stream back frame by frame, so server-sent events are sealed too; WebSockets cannot be.
 */

/** Where a session is opened, relative to the node's base URL. */
const val HANDSHAKE_PATH: String = "/sealed/handshake"

/**
 * Where nodes released before [HANDSHAKE_PATH] existed open a session (newer nodes still serve
 * it as a deprecated alias). The handshake is tried here only when the node answers 404, 405 or
 * 403 `sealed_required` at [HANDSHAKE_PATH].
 */
const val LEGACY_HANDSHAKE_PATH: String = "/sealed/v2/handshake"

/** Where sealed requests are posted, relative to the node's base URL. */
const val SEALED_PATH: String = "/sealed/v2"

/** Content type of every sealed body. */
const val SEALED_CONTENT_TYPE: String = "application/vnd.calimero.sealed"

private const val VERSION = 2
private val PROLOGUE = "calimero/sealed-http/v2".encodeToByteArray()
private const val SESSION_ID_LEN = 16
private const val HEADER_LEN = 1 + SESSION_ID_LEN + 8
private const val FRAME_HEAD = 0
private const val FRAME_DATA = 1
private const val FRAME_END = 2

/**
 * The largest frame the node sends: a kind byte, at most 64 KiB of data, a tag. Anything
 * larger is refused before it is buffered, so a proxy cannot make the client hold an
 * unbounded frame it has yet to authenticate.
 */
private const val MAX_FRAME_LEN = 1 + 64 * 1024 + 16

/** Far more than a handshake reply (69 bytes) ever is. */
private const val MAX_HANDSHAKE_REPLY = 1024L

/** Open a new session this long before the node would drop the current one. */
private const val RENEW_BEFORE_MS = 60_000L
private const val DEFAULT_BUSY_WAIT_MS = 1_000L

/**
 * What a node from before [HANDSHAKE_PATH] answers there: no such route, no POST on it, or, sealing
 * everything, a refusal of the path as an unsealed request (`sealed_required`).
 */
private const val HTTP_NOT_FOUND = 404
private const val HTTP_BAD_METHOD = 405
private const val HTTP_FORBIDDEN = 403
private const val MAX_REFUSAL_PEEK = 4096L
private const val MAX_RETRY_AFTER_SECS = 5.0
private const val TRANSPORT_BINDING_DOMAIN = "calimero.tee-attest.transport-key.v1"
private val NULL_BODY_STATUSES = setOf(101, 204, 205, 304)
private val SEALED_MEDIA = SEALED_CONTENT_TYPE.toMediaType()
private val sealedJson = Json { ignoreUnknownKeys = true }

/**
 * The node no longer holds the transport key this client attested (it restarted): attest
 * again for the current one. A [SealedTransport] given an attestation step does that itself;
 * one given fixed key bytes throws this.
 */
class StaleTransportKeyException : IOException("The node no longer holds the attested transport key; attest again for the current one")

/**
 * The sealed transport failed: the node refused the envelope itself, before anything inside it
 * ran ([status] and the node's error [code], such as `unknown_session`, `busy`, `malformed`),
 * or the attestation or a frame did not hold ([status] null). An [IOException], so OkHttp
 * reports it as the call's failure.
 */
class SealedTransportException(
    message: String,
    /** The node's HTTP status for an envelope refusal; null for a failure on this side. */
    val status: Int? = null,
    val code: String? = null,
    /** How long the node asked to wait before retrying (`Retry-After`, at most 5 s), if it did. */
    val retryAfterMs: Long? = null,
    cause: Throwable? = null,
) : IOException(message, cause)

/**
 * What the attest endpoint puts in report data bytes `32..64` when asked to bind its transport
 * key: `SHA-256("calimero.tee-attest.transport-key.v1" ‖ inner ‖ transportKey)`. [inner] is
 * what would have been there otherwise: the expected application hash, or 32 zero bytes.
 * Mirrors core's `attest_transport_binding`.
 */
fun transportKeyBinding(
    inner: ByteArray,
    transportKey: ByteArray,
): ByteArray = sha256(concat(TRANSPORT_BINDING_DOMAIN.encodeToByteArray(), inner, transportKey))

/**
 * Seals every request to a TEE node's attested transport key — mero-js `createSealedFetch`, as
 * an OkHttp [Interceptor]. Use [client] (the given client with this interceptor in front) for
 * every request to the node. [com.calimero.mero.CalimeroClient] opens one per node it uses.
 *
 * A request for any URL outside [baseUrl] is refused rather than sent in the clear: falling
 * back to plaintext would silently undo the point of sealing.
 *
 * The transport key is either fixed bytes (a restarted node then surfaces as
 * [StaleTransportKeyException]) or a suspend step that attests and returns it — typically
 * `{ fetchAttestedKey(baseUrl, policy) }`: a restarted node is then attested again,
 * through your verifier, and the request retried once. A failed attestation is not
 * remembered: the next request attests again (mero-js keeps the failure).
 *
 * One session serves every request, concurrent ones included, and is renewed a minute before
 * the node would drop it; a session the node no longer knows (`unknown_session`) is replaced
 * and the request sent again, which is safe because nothing inside it ran.
 *
 * The response handed back is the node's own, opened frame by frame as it streams. It is built
 * here, so OkHttp's [okhttp3.Authenticator] and redirect handling do not see it: a `401` or a
 * `3xx` inside the envelope reaches the caller as is.
 */
class SealedTransport private constructor(
    /** The node's base URL. */
    val baseUrl: String,
    private val fixedKey: ByteArray?,
    private val attest: (suspend () -> ByteArray)?,
    private val base: OkHttpClient,
) : Interceptor {
    /** Seal to fixed transport key bytes: a restarted node then surfaces as [StaleTransportKeyException]. */
    constructor(
        baseUrl: String,
        transportPublicKey: ByteArray,
        httpClient: OkHttpClient = PlainJsonHttp.defaultClient(),
    ) : this(baseUrl, transportPublicKey.copyOf(), null, httpClient)

    /** Seal to the key [transportPublicKey] attests, asked again whenever the node restarted. */
    constructor(
        baseUrl: String,
        httpClient: OkHttpClient = PlainJsonHttp.defaultClient(),
        transportPublicKey: suspend () -> ByteArray,
    ) : this(baseUrl, null, transportPublicKey, httpClient)

    private val baseHttpUrl = baseUrl.toHttpUrl()
    private val basePath = baseHttpUrl.encodedPath.trimEnd('/')
    private val nodeUrl =
        baseHttpUrl
            .newBuilder()
            .encodedPath(basePath.ifEmpty { "/" })
            .build()
            .toString()
            .trimEnd('/')
    private val lock = Any()
    private var transportKey: ByteArray? = fixedKey
    private var session: Session? = null

    /** [httpClient] with this interceptor in front of its own. */
    val client: OkHttpClient = base.newBuilder().apply { interceptors().add(0, this@SealedTransport) }.build()

    override fun intercept(chain: Interceptor.Chain): Response {
        val request = chain.request()
        val url = request.url
        val encodedPath = url.encodedPath
        if (url.scheme != baseHttpUrl.scheme ||
            url.host != baseHttpUrl.host ||
            url.port != baseHttpUrl.port ||
            !encodedPath.startsWith("$basePath/")
        ) {
            throw SealedTransportException("Refusing to send $url outside the sealed node at $baseUrl")
        }
        val carrier =
            base
                .newBuilder()
                .connectTimeout(chain.connectTimeoutMillis().toLong(), TimeUnit.MILLISECONDS)
                .readTimeout(chain.readTimeoutMillis().toLong(), TimeUnit.MILLISECONDS)
                .writeTimeout(chain.writeTimeoutMillis().toLong(), TimeUnit.MILLISECONDS)
                .build()

        val body = request.body?.let { b -> Buffer().also { b.writeTo(it) }.readByteArray() } ?: ByteArray(0)
        val head = requestHead(request, encodedPath.removePrefix(basePath) + (url.encodedQuery?.let { "?$it" } ?: ""))

        val open = currentSession(carrier)
        val response =
            try {
                send(carrier, open, head, body, request)
            } catch (e: SealedTransportException) {
                // The node refused the envelope before running anything in it, so the request
                // can be sent again safely under a new session.
                if (e.code != "unknown_session") throw e
                send(carrier, renew(carrier, open), head, body, request)
            }
        return response
    }

    /** Forget the session (and an attested key), so the next request opens a fresh one. For tests. */
    internal fun reset() {
        synchronized(lock) {
            session = null
            if (attest != null) transportKey = null
        }
    }

    private fun send(
        carrier: OkHttpClient,
        open: Session,
        head: JsonObject,
        body: ByteArray,
        original: Request,
    ): Response {
        val sealed = sealRequest(open, head, body)
        val response =
            carrier
                .newCall(
                    Request
                        .Builder()
                        .url("$nodeUrl$SEALED_PATH")
                        .post(sealed.envelope.toRequestBody(SEALED_MEDIA))
                        .build(),
                ).execute()
        if (response.header("content-type") != SEALED_CONTENT_TYPE) throw envelopeRefusal(response)
        return openResponse(response, open.responseKey, sealed.header, sealed.requestId, original)
    }

    private fun currentSession(carrier: OkHttpClient): Session =
        synchronized(lock) {
            val existing = session
            if (existing != null && System.currentTimeMillis() < existing.renewAt) return existing
            openSession(carrier).also { session = it }
        }

    /** Replace the session only if it is still the one found stale, so concurrent requests open one between them. */
    private fun renew(
        carrier: OkHttpClient,
        stale: Session,
    ): Session =
        synchronized(lock) {
            val existing = session
            if (existing != null && existing !== stale) return existing
            openSession(carrier).also { session = it }
        }

    private fun openSession(carrier: OkHttpClient): Session =
        try {
            handshake(carrier, key())
        } catch (e: StaleTransportKeyException) {
            if (attest == null) throw e
            transportKey = null
            handshake(carrier, key())
        }

    @Suppress("ThrowsCount")
    private fun key(): ByteArray {
        transportKey?.let { return it }
        val step = checkNotNull(attest)
        val attested =
            try {
                runBlocking { step() }
            } catch (e: IOException) {
                throw e
            } catch (e: Exception) {
                throw SealedTransportException(e.message ?: e.toString(), cause = e)
            }
        if (attested.size != 32) throw SealedTransportException("transportPublicKey must be 32 bytes")
        transportKey = attested
        return attested
    }

    private fun handshake(
        carrier: OkHttpClient,
        transportKey: ByteArray,
    ): Session =
        try {
            handshakeOnce(carrier, transportKey)
        } catch (e: SealedTransportException) {
            // The node limits how fast sessions open. A refused handshake opened nothing, so
            // trying once more after the pause it asks for is safe.
            if (e.code != "busy") throw e
            Thread.sleep(e.retryAfterMs ?: DEFAULT_BUSY_WAIT_MS)
            handshakeOnce(carrier, transportKey)
        }

    @Suppress("ThrowsCount")
    private fun handshakeOnce(
        carrier: OkHttpClient,
        transportKey: ByteArray,
    ): Session {
        if (transportKey.size != 32) throw SealedTransportException("transportPublicKey must be 32 bytes")
        val noise = NoiseInitiator.initiate(transportKey, PROLOGUE)
        val body = concat(byteArrayOf(VERSION.toByte()), transportKey, noise.message1)
        postHandshake(carrier, body).use { response ->
            if (response.header("content-type") != SEALED_CONTENT_TYPE) throw envelopeRefusal(response)
            val source = response.body?.source() ?: return sessionFrom(noise, ByteArray(0))
            if (source.request(MAX_HANDSHAKE_REPLY + 1)) throw SealedTransportException("The handshake reply is malformed: it is too long")
            return sessionFrom(noise, source.readByteArray())
        }
    }

    /**
     * Post handshake message 1 to [HANDSHAKE_PATH]; a node from before the path gets the same
     * message once at [LEGACY_HANDSHAKE_PATH]. Such a node routes nothing there (404, 405) or,
     * sealing everything as Cloud nodes do, refuses it as an unsealed request (403
     * `sealed_required`). That node never read the first message, so sending it again opens
     * nothing twice. Any other answer is returned as it is.
     */
    private fun postHandshake(
        carrier: OkHttpClient,
        body: ByteArray,
    ): Response {
        fun post(path: String): Response =
            carrier
                .newCall(
                    Request
                        .Builder()
                        .url("$nodeUrl$path")
                        .post(body.toRequestBody(SEALED_MEDIA))
                        .build(),
                ).execute()
        val response = post(HANDSHAKE_PATH)
        if (!predatesHandshakePath(response)) return response
        response.close()
        return post(LEGACY_HANDSHAKE_PATH)
    }

    internal companion object {
        /** The request's head as the node reads it: `{method, path, headers: [[name, value], …]}`, names in lower case. */
        fun requestHead(
            request: Request,
            path: String,
        ): JsonObject {
            val headers = mutableListOf<Pair<String, String>>()
            request.headers.forEach { (name, value) -> headers += name.lowercase() to value }
            val contentType = request.body?.contentType()
            if (contentType != null && headers.none { it.first == "content-type" }) headers += "content-type" to contentType.toString()
            return head(request.method, path, headers)
        }

        fun head(
            method: String,
            path: String,
            headers: List<Pair<String, String>>,
        ): JsonObject =
            buildJsonObject {
                put("method", method)
                put("path", path)
                putJsonArray("headers") {
                    headers.forEach { (name, value) ->
                        add(
                            buildJsonArray {
                                add(name)
                                add(value)
                            },
                        )
                    }
                }
            }
    }
}

/** An open session: its keys, its id, and the next request id. */
internal class Session(
    val requestKey: ByteArray,
    val responseKey: ByteArray,
    private val sessionId: ByteArray,
    val renewAt: Long,
) {
    private val nextRequestId = AtomicLong(1)

    fun nextId(): Long = nextRequestId.getAndIncrement()

    fun header(requestId: Long): ByteArray {
        val header = ByteArray(HEADER_LEN)
        header[0] = VERSION.toByte()
        sessionId.copyInto(header, 1)
        for (i in 0 until 8) header[1 + SESSION_ID_LEN + i] = (requestId ushr (8 * (7 - i))).toByte()
        return header
    }
}

/** Finish the handshake with the node's [reply]. */
@Suppress("ThrowsCount")
internal fun sessionFrom(
    noise: NoiseInitiator,
    reply: ByteArray,
    now: Long = System.currentTimeMillis(),
): Session {
    if (reply.isEmpty() || reply[0].toInt() != VERSION) throw SealedTransportException("The handshake reply is malformed")
    val finished =
        try {
            noise.finish(reply.copyOfRange(1, reply.size))
        } catch (_: AeadException) {
            throw SealedTransportException("The handshake reply did not open: it did not come from the attested node")
        }
    val payload = finished.payload
    if (payload.size != SESSION_ID_LEN + 4) throw SealedTransportException("The handshake reply is malformed")
    val lifetimeSecs = (0 until 4).fold(0L) { acc, i -> (acc shl 8) or (payload[SESSION_ID_LEN + i].toLong() and 0xff) }
    return Session(finished.split.first, finished.split.second, payload.copyOfRange(0, SESSION_ID_LEN), now + lifetimeSecs * 1000 - RENEW_BEFORE_MS)
}

internal class SealedRequest(
    val requestId: Long,
    val header: ByteArray,
    val envelope: ByteArray,
)

/** Seal one request under the session's next request id. */
internal fun sealRequest(
    session: Session,
    head: JsonObject,
    body: ByteArray,
): SealedRequest {
    val requestId = session.nextId()
    val header = session.header(requestId)
    val encoded = head.toString().encodeToByteArray()
    val inner = concat(u32be(encoded.size.toLong()), encoded, body)
    return SealedRequest(requestId, header, concat(header, AesGcm.seal(session.requestKey, frameNonce(requestId, 0), header, inner)))
}

/** `request_id ‖ index`, as the node numbers them. */
private fun frameNonce(
    requestId: Long,
    index: Int,
): ByteArray {
    val nonce = ByteArray(12)
    for (i in 0 until 8) nonce[i] = (requestId ushr (8 * (7 - i))).toByte()
    u32be(index.toLong()).copyInto(nonce, 8)
    return nonce
}

private fun u32be(value: Long): ByteArray = ByteArray(4) { i -> (value ushr (8 * (3 - i))).toByte() }

/**
 * Open a sealed response as it streams: read its head, then hand back a [Response] whose body
 * opens each frame as it arrives. A frame that does not open, arrives out of order, or a stream
 * that stops before its end frame fails the body rather than passing anything unauthenticated on.
 */
@Suppress("ThrowsCount")
internal fun openResponse(
    response: Response,
    responseKey: ByteArray,
    header: ByteArray,
    requestId: Long,
    original: Request,
): Response {
    val source = response.body?.source() ?: throw SealedTransportException("The sealed response has no body")
    val frames = FrameReader(source, responseKey, header, requestId)
    val first =
        try {
            frames.next()
        } catch (e: IOException) {
            response.close()
            throw e
        }
    if (first == null || first.first != FRAME_HEAD) {
        response.close()
        throw SealedTransportException("The sealed response is malformed")
    }
    val head =
        runCatching { sealedJson.parseToJsonElement(String(first.second, Charsets.UTF_8)) as JsonObject }.getOrNull()
            ?: run {
                response.close()
                throw SealedTransportException("The sealed response is malformed")
            }
    val status = (head["status"] as? JsonPrimitive)?.intOrNull ?: throw SealedTransportException("The sealed response is malformed")
    val headers = Headers.Builder()
    (head["headers"] as? JsonArray)?.forEach { pair ->
        val p = pair as? JsonArray ?: return@forEach
        val name = (p.getOrNull(0) as? JsonPrimitive)?.content ?: return@forEach
        val value = (p.getOrNull(1) as? JsonPrimitive)?.content ?: return@forEach
        headers.addUnsafeNonAscii(name, value)
    }
    val built = headers.build()
    val mediaType = built["content-type"]?.toMediaTypeOrNull()
    val body: ResponseBody =
        if (status in NULL_BODY_STATUSES) {
            response.close()
            ByteArray(0).toResponseBody(mediaType)
        } else {
            StreamingBody(FrameSource(frames, response), mediaType)
        }
    return Response
        .Builder()
        .request(original)
        .protocol(Protocol.HTTP_1_1)
        .code(status)
        .message("")
        .headers(built)
        .body(body)
        .build()
}

private class StreamingBody(
    private val source: Source,
    private val mediaType: MediaType?,
    private val length: Long = -1,
) : ResponseBody() {
    private val buffered: BufferedSource by lazy { source.buffer() }

    override fun contentType(): MediaType? = mediaType

    override fun contentLength(): Long = length

    override fun source(): BufferedSource = buffered
}

/** Length-prefixed frames off the wire, each opened under the response key in order. */
private class FrameReader(
    private val source: BufferedSource,
    private val responseKey: ByteArray,
    private val header: ByteArray,
    private val requestId: Long,
) {
    private var index = 0

    /** The next frame as `(kind, data)`, or null when the stream ended. */
    @Suppress("ThrowsCount")
    fun next(): Pair<Int, ByteArray>? {
        if (!source.request(4)) return null
        val length = source.readInt().toLong() and 0xffffffffL
        if (length > MAX_FRAME_LEN) throw IOException("The sealed response is malformed: a frame is larger than the node sends")
        if (!source.request(length)) return null
        val sealed = source.readByteArray(length)
        val plain =
            try {
                AesGcm.open(responseKey, frameNonce(requestId, index), header, sealed)
            } catch (_: AeadException) {
                throw IOException("A sealed response frame did not open: it was not sealed by the attested node")
            }
        index += 1
        if (plain.isEmpty()) throw IOException("The sealed response is malformed")
        return (plain[0].toInt() and 0xff) to plain.copyOfRange(1, plain.size)
    }
}

/** The plaintext body: data frames in order, ending at the end frame. */
private class FrameSource(
    private val frames: FrameReader,
    private val response: Response,
) : Source {
    private val pending = Buffer()
    private var done = false

    override fun read(
        sink: Buffer,
        byteCount: Long,
    ): Long {
        while (pending.size == 0L) {
            if (done) return -1
            val frame = frames.next() ?: throw IOException("The sealed response was cut short")
            when (frame.first) {
                FRAME_DATA -> pending.write(frame.second)
                FRAME_END -> {
                    done = true
                    response.close()
                }
                else -> throw IOException("The sealed response is malformed")
            }
        }
        return pending.read(sink, minOf(byteCount, pending.size))
    }

    override fun timeout(): Timeout = Timeout.NONE

    override fun close() {
        response.close()
    }
}

/** Whether a handshake answer says the node predates [HANDSHAKE_PATH] (see `postHandshake`); reads the body without consuming it. */
private fun predatesHandshakePath(response: Response): Boolean =
    when (response.code) {
        HTTP_NOT_FOUND, HTTP_BAD_METHOD -> true
        HTTP_FORBIDDEN ->
            runCatching {
                val body = response.peekBody(MAX_REFUSAL_PEEK).string()
                ((sealedJson.parseToJsonElement(body) as JsonObject)["error"] as? JsonObject).string("code") == "sealed_required"
            }.getOrDefault(false)
        else -> false
    }

/** The node's refusal of an envelope, as the exception it means. */
private fun envelopeRefusal(response: Response): IOException =
    response.use {
        var code: String? = null
        var message = "Sealed request refused: HTTP ${response.code}"
        runCatching {
            val error = (sealedJson.parseToJsonElement(response.body?.string().orEmpty()) as JsonObject)["error"] as? JsonObject
            code = error.string("code")
            error.string("message")?.let { message = "Sealed request refused: $it" }
        }
        // Not the node's JSON refusal — a proxy's page, most likely: the status is all that can
        // honestly be reported.
        if (code == "stale_transport_key") return StaleTransportKeyException()
        val retryAfter = response.header("retry-after")?.toDoubleOrNull()
        val retryAfterMs = retryAfter?.takeIf { it.isFinite() && it > 0 }?.let { (minOf(it, MAX_RETRY_AFTER_SECS) * 1000).toLong() }
        SealedTransportException(message, response.code, code, retryAfterMs)
    }
