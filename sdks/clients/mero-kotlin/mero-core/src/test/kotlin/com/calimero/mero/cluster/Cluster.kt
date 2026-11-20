package com.calimero.mero.cluster

import com.calimero.mero.AppGrant
import com.calimero.mero.CalimeroClient
import com.calimero.mero.CalimeroClientOptions
import com.calimero.mero.CloudPolicy
import com.calimero.mero.SoftPasskey
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.junit.Assume.assumeTrue
import java.io.File

/**
 * The local merod 1.0 cluster (`merod local`, mock attestation) the cluster tests run against,
 * the Kotlin twin of mero-js's `tests/cluster/`. Skipped unless the environment names one:
 *
 *     MERO_CLUSTER=http://127.0.0.1:3501,http://127.0.0.1:3502,http://127.0.0.1:3503
 *     MERO_KV_WASM=path/to/app.wasm                 (core's kv-store test fixture, the raw component)
 *     MERO_KV_MPK=path/to/kv-store-test-fixture.mpk (the same, as a signed package; optional)
 *
 * `scripts/cluster-test.sh` starts one from a `merod` binary and runs them.
 */
object Cluster {
    const val ORIGIN = "http://localhost:3000"

    /** This run's own suffix for package names: a package stays its first signer's on the cluster. */
    val run: String = java.lang.Long.toString(System.currentTimeMillis(), 36)
    lateinit var urls: List<String>

    /** kv-store with its own bytes (a custom section appended): the same WASM inside MERO_KV_MPK
     * is that private release's, a hash only its installers may name. */
    lateinit var kvWasm: ByteArray

    /** kv-store as it is, for packages signed here ([signedMpk]). */
    lateinit var kvRaw: ByteArray
    var kvMpk: ByteArray? = null

    /** Skips the calling class unless a cluster is named. */
    fun require() {
        val cluster = System.getenv("MERO_CLUSTER").orEmpty()
        val wasm = System.getenv("MERO_KV_WASM").orEmpty()
        assumeTrue("MERO_CLUSTER and MERO_KV_WASM name no cluster: skipped", cluster.isNotBlank() && wasm.isNotBlank())
        if (::urls.isInitialized) return
        urls = cluster.split(',').map { it.trim().trimEnd('/') }.filter { it.isNotEmpty() }
        kvRaw = File(wasm).readBytes()
        kvWasm = variant(kvRaw, "mero-kotlin-raw")
        kvMpk = System.getenv("MERO_KV_MPK")?.takeIf { it.isNotBlank() }?.let { File(it).readBytes() }
    }

    fun client(
        i: Int,
        allowMock: Boolean = true,
    ) = CalimeroClient(CalimeroClientOptions(baseUrl = urls[i % urls.size], policy = CloudPolicy(allowMock = allowMock)))

    suspend fun person(
        i: Int,
        name: String,
    ): Pair<CalimeroClient, SoftPasskey> {
        val c = client(i)
        val pk = SoftPasskey("localhost", ORIGIN)
        c.signUp(pk, name)
        return c to pk
    }

    suspend fun deployKv(
        c: CalimeroClient,
        name: String,
        org: String? = null,
        grants: List<AppGrant>? = null,
    ): String {
        val bundle = c.packages.bundlePut(body = kvWasm).hash
        return c.apps.create(name = name, bundle = bundle, org = org, grants = grants).appId
    }

    fun kv(
        key: String,
        value: String? = null,
    ) = buildJsonObject {
        put("key", key)
        value?.let { put("value", it) }
    }
}

/** [wasm] with a custom section named [tag]: another module that runs the same. */
fun variant(
    wasm: ByteArray,
    tag: String,
): ByteArray {
    val name = tag.toByteArray()
    return wasm + byteArrayOf(0, (name.size + 1).toByte(), name.size.toByte()) + name
}

/** What [block] threw, when it is a [T]; fails otherwise. */
suspend inline fun <reified T : Throwable> refused(block: () -> Unit): T {
    try {
        block()
    } catch (e: Throwable) {
        if (e is T) return e
        throw AssertionError("expected ${T::class.simpleName}, got $e", e)
    }
    throw AssertionError("expected ${T::class.simpleName}, got success")
}
