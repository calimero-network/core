package com.calimero.mero.tee

import okhttp3.Interceptor
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.Response
import okhttp3.ResponseBody.Companion.toResponseBody
import okio.Buffer

/**
 * A network of made-up hosts (`https://relay.example`, `https://cloud.example`, …) for the TEE
 * tests, as mero-js's tests stub `fetch`: every request the [client] makes is answered by
 * [handle], in process, and recorded in [requests]. A null answer is a `500 unexpected`.
 */
internal class FakeNet(
    private val handle: (Request, ByteArray) -> Response?,
) {
    val requests: MutableList<String> = java.util.Collections.synchronizedList(mutableListOf())

    /** Every request body, as sent, in order. */
    val bodies: MutableList<ByteArray> = java.util.Collections.synchronizedList(mutableListOf())

    val client: OkHttpClient =
        OkHttpClient
            .Builder()
            .addInterceptor(
                Interceptor { chain ->
                    val request = chain.request()
                    val body = request.body?.let { b -> Buffer().also { b.writeTo(it) }.readByteArray() } ?: ByteArray(0)
                    requests += request.url.toString()
                    bodies += body
                    handle(request, body) ?: respond(request, 500, "unexpected", "text/plain")
                },
            ).build()

    companion object {
        fun respond(
            request: Request,
            status: Int,
            body: String,
            contentType: String = "application/json",
            headers: Map<String, String> = emptyMap(),
        ): Response = respondBytes(request, status, body.encodeToByteArray(), contentType, headers)

        fun respondBytes(
            request: Request,
            status: Int,
            body: ByteArray,
            contentType: String,
            headers: Map<String, String> = emptyMap(),
        ): Response =
            Response
                .Builder()
                .request(request)
                .protocol(Protocol.HTTP_1_1)
                .code(status)
                .message("")
                .header("content-type", contentType)
                .apply { headers.forEach { (k, v) -> header(k, v) } }
                .body(body.toResponseBody(contentType.toMediaType()))
                .build()
    }
}
