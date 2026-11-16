package com.calimero.mero

import com.calimero.mero.crypto.sha256
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import okhttp3.OkHttpClient
import okhttp3.Request

/**
 * Where a package's frontend is served: `https://{label}.apps.calimero.network/`.
 * (== core `app_host::label`, mdma `mdma_common.app_host`; Swift `AppHost`.)
 *
 * `label` is the unpadded lowercase base32 of `SHA-256("calimero.app-host.v1\0" + package)`. A
 * package on one cluster in particular is at [clusterLabel], the same encoding of
 * `SHA-256("calimero.app-host.cluster.v1\0" + cluster + "\0" + package)`.
 */
object AppHost {
    /** The apps domain every Calimero Cloud cluster shares. */
    const val DEFAULT_DOMAIN: String = "apps.calimero.network"

    /** The DNS label of [packageName]'s frontend origin (its primary cluster). */
    fun label(packageName: String): String = encode(sha256("calimero.app-host.v1\u0000$packageName".encodeToByteArray()))

    /** The DNS label of [packageName]'s frontend origin on the cluster named [cluster]. */
    fun clusterLabel(
        cluster: String,
        packageName: String,
    ): String = encode(sha256("calimero.app-host.cluster.v1\u0000$cluster\u0000$packageName".encodeToByteArray()))

    /** `https://{label}.{domain}`: on [cluster] when one is named, else on the package's primary cluster. */
    fun origin(
        packageName: String,
        cluster: String? = null,
        domain: String = DEFAULT_DOMAIN,
    ): String = "https://${cluster?.let { clusterLabel(it, packageName) } ?: label(packageName)}.$domain"

    /**
     * The newest release whose frontend [origin] serves, from its `/.well-known/calimero.json`
     * (releases oldest first). Throws [ServedPageSignInError] when the host serves none.
     */
    @Suppress("ThrowsCount")
    suspend fun latestVersion(
        origin: String,
        httpClient: OkHttpClient = CalimeroClientOptions.defaultHttpClient(),
    ): String {
        val url = "${origin.trimEnd('/')}/.well-known/calimero.json"
        val (status, body) =
            withContext(Dispatchers.IO) {
                httpClient
                    .newCall(
                        Request
                            .Builder()
                            .url(url)
                            .get()
                            .build(),
                    ).execute()
                    .use { it.code to it.body?.string().orEmpty() }
            }
        val host = url.substringAfter("://").substringBefore('/')
        if (status != 200) throw ServedPageSignInError("$host answered $status: no frontend served there")
        val info =
            try {
                json.decodeFromString(Info.serializer(), body)
            } catch (e: kotlinx.serialization.SerializationException) {
                throw ServedPageSignInError("$host answered no release list", e)
            }
        return info.versions.lastOrNull() ?: throw ServedPageSignInError("the app host serves no release")
    }

    @Serializable
    private data class Info(
        val versions: List<String> = emptyList(),
    )

    private val json = Json { ignoreUnknownKeys = true }

    private const val ALPHABET = "abcdefghijklmnopqrstuvwxyz234567"

    private fun encode(digest: ByteArray): String {
        val out = StringBuilder()
        var acc = 0
        var bits = 0
        for (byte in digest) {
            acc = (acc shl 8) or (byte.toInt() and 0xff)
            bits += 8
            while (bits >= 5) {
                bits -= 5
                out.append(ALPHABET[(acc ushr bits) and 31])
            }
            acc = acc and ((1 shl bits) - 1)
        }
        if (bits > 0) out.append(ALPHABET[(acc shl (5 - bits)) and 31])
        return out.toString()
    }
}
