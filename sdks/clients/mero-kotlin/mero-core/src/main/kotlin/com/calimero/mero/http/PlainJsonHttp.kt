package com.calimero.mero.http

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.IOException
import java.util.concurrent.TimeUnit

/**
 * A bare JSON-over-HTTP helper for what is fetched in the clear: a node's `/attestation`,
 * a signed release from its mirror, Intel's collateral, an agent gate.
 *
 * Non-2xx answers come back as an [HttpResponse]; only a transport failure throws
 * ([NetworkException]).
 *
 * [role] names the host for [TransportSecurity.checkServiceUrl]: plain http is refused to
 * anything but loopback ([InsecureTransportException]). Null skips the check, for requests
 * that are attested or sealed end to end (a TEE node's `/tee/attest` and `/tee/info`, the
 * agent gate) or verified after the fact (a signed release).
 */
internal class PlainJsonHttp(
    private val client: OkHttpClient = defaultClient(),
    private val role: String? = "relay",
) {
    suspend fun execute(
        method: String,
        url: String,
        body: String? = null,
        headers: Map<String, String> = emptyMap(),
    ): HttpResponse =
        withContext(Dispatchers.IO) {
            role?.let { TransportSecurity.checkServiceUrl(url, it) }
            val builder = Request.Builder().url(url).header("Accept", "application/json")
            val requestBody = body?.toRequestBody(JSON)
            when (method.uppercase()) {
                "GET" -> builder.get()
                "DELETE" -> if (requestBody != null) builder.delete(requestBody) else builder.delete()
                "POST" -> builder.post(requestBody ?: "".toRequestBody(JSON))
                "PUT" -> builder.put(requestBody ?: "".toRequestBody(JSON))
                else -> error("Unsupported HTTP method: $method")
            }
            headers.forEach { (k, v) -> builder.header(k, v) }
            val response =
                try {
                    client.newCall(builder.build()).execute()
                } catch (e: IOException) {
                    throw NetworkException(e.message ?: "Network error", e)
                }
            response.use { resp ->
                HttpResponse(
                    status = resp.code,
                    headers = resp.headers.names().associateWith { resp.header(it).orEmpty() },
                    body = resp.body?.let { readBounded(it.source()) }.orEmpty(),
                    url = url,
                )
            }
        }

    companion object {
        private val JSON = "application/json".toMediaType()

        fun defaultClient(timeoutMs: Long = 15_000): OkHttpClient =
            OkHttpClient
                .Builder()
                .connectTimeout(timeoutMs, TimeUnit.MILLISECONDS)
                .readTimeout(timeoutMs, TimeUnit.MILLISECONDS)
                .writeTimeout(timeoutMs, TimeUnit.MILLISECONDS)
                .build()
    }
}

/** A response as [PlainJsonHttp] answers it. */
internal class HttpResponse(
    val status: Int,
    val headers: Map<String, String>,
    val body: String,
    val url: String,
) {
    val isSuccessful: Boolean get() = status in 200..299
}

/** The most bytes of a plain response read (a release, collateral): 16 MiB. */
internal const val MAX_PLAIN_RESPONSE_BYTES: Long = 16L * 1024 * 1024

private fun readBounded(source: okio.BufferedSource): String {
    if (source.request(MAX_PLAIN_RESPONSE_BYTES + 1)) throw NetworkException("The response is larger than $MAX_PLAIN_RESPONSE_BYTES bytes")
    return source.readUtf8()
}

/** Strip trailing slashes from a base URL. */
internal fun String.trimBase(): String = trimEnd('/')
