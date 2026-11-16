package com.calimero.mero

import com.calimero.mero.http.NetworkException
import com.calimero.mero.http.PlainJsonHttp
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.doubleOrNull
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.OkHttpClient
import java.net.URLEncoder

/*
 * The Calimero Cloud node directory: which nodes of a cluster take traffic now, and in what order
 * this client tries them (mero-js `cloud-v2/directory.ts`).
 *
 * `GET {directory}/{cluster}` (no auth) answers
 * `{ cluster, nodes: [{ id, url, zone, weight }], refreshAfterSeconds }`. The directory is not
 * trusted for security: it only picks which nodes to try. Every node is attested under the
 * caller's own policy before anything is sent to it, so a bad list can only make connecting fail.
 */

/** The Calimero Cloud control plane's public node directory. */
const val DEFAULT_DIRECTORY: String = "https://manager.cloud.calimero.network/api/directory"

/** How long a listing stands when the directory does not say. */
private const val DEFAULT_REFRESH_SECONDS = 60.0

/** Why a [DirectoryError] was thrown. */
enum class DirectoryFailure(
    val wire: String,
) {
    /** No answer, or an error status. */
    Unreachable("unreachable"),

    /** The directory does not know the cluster (`404`). */
    NotFound("not-found"),

    /** Not a listing. */
    BadResponse("bad-response"),

    /** No usable node. */
    Empty("empty"),
}

/**
 * The directory could not name a node to try ([reason]). Not a [CalimeroError]: no node answered
 * yet. [CalimeroClient.connect] throws it; a later read of the directory, after a failover, never
 * does (it keeps the list the client has).
 */
class DirectoryError(
    val cluster: String,
    val reason: DirectoryFailure,
    message: String,
    val status: Int? = null,
    cause: Throwable? = null,
) : Exception("directory for cluster $cluster: $message", cause)

/** One usable node of a listing. */
data class DirectoryNode(
    val url: String,
    val zone: String? = null,
    val weight: Int,
)

/** A cluster's listing: its usable nodes, and when to read it again. */
data class DirectoryListing(
    val nodes: List<DirectoryNode>,
    val refreshAfterSeconds: Double,
)

/** The URL of [cluster]'s listing in [directory]. */
fun directoryUrl(
    directory: String,
    cluster: String,
): String = "${directory.trimEnd('/')}/${URLEncoder.encode(cluster, "UTF-8").replace("+", "%20")}"

private val directoryJson = Json { ignoreUnknownKeys = true }

/**
 * Read [cluster]'s listing, keeping only the entries with an http(s) URL and a positive integer
 * weight (the first of a repeated URL). Throws [DirectoryError].
 */
@Suppress("ThrowsCount")
suspend fun readDirectory(
    directory: String,
    cluster: String,
    httpClient: OkHttpClient = PlainJsonHttp.defaultClient(),
): DirectoryListing {
    val response =
        try {
            PlainJsonHttp(httpClient, role = null).execute("GET", directoryUrl(directory, cluster))
        } catch (e: NetworkException) {
            throw DirectoryError(cluster, DirectoryFailure.Unreachable, "could not be reached: ${e.message}", cause = e)
        }
    if (response.status == 404) throw DirectoryError(cluster, DirectoryFailure.NotFound, "unknown cluster", 404)
    if (!response.isSuccessful) throw DirectoryError(cluster, DirectoryFailure.Unreachable, "answered ${response.status}", response.status)
    val body =
        try {
            directoryJson.parseToJsonElement(response.body)
        } catch (e: kotlinx.serialization.SerializationException) {
            throw DirectoryError(cluster, DirectoryFailure.BadResponse, "answered something other than JSON", response.status, e)
        }
    val entries = ((body as? JsonObject)?.get("nodes") as? JsonArray) ?: throw DirectoryError(cluster, DirectoryFailure.BadResponse, "answered no node list", response.status)
    val nodes = mutableListOf<DirectoryNode>()
    val seen = HashSet<String>()
    for (entry in entries) {
        val node = usable(entry as? JsonObject) ?: continue
        if (seen.add(node.url)) nodes += node
    }
    if (nodes.isEmpty()) throw DirectoryError(cluster, DirectoryFailure.Empty, "lists no usable node", response.status)
    val after = ((body["refreshAfterSeconds"] as? JsonPrimitive)?.takeIf { !it.isString })?.doubleOrNull
    return DirectoryListing(nodes, if (after != null && after.isFinite() && after >= 1) after else DEFAULT_REFRESH_SECONDS)
}

private fun usable(entry: JsonObject?): DirectoryNode? {
    val url = (entry?.get("url") as? JsonPrimitive)?.takeIf { it.isString }?.content
    val weight = (entry?.get("weight") as? JsonPrimitive)?.takeIf { !it.isString }?.doubleOrNull
    val scheme = url?.toHttpUrlOrNull()?.scheme
    val ok = weight != null && weight >= 1 && weight == Math.floor(weight) && weight <= Int.MAX_VALUE && (scheme == "https" || scheme == "http")
    if (!ok) return null
    val zone = (entry["zone"] as? JsonPrimitive)?.takeIf { it.isString }?.content
    return DirectoryNode(url.trimEnd('/'), zone, weight.toInt())
}

/**
 * The order to try [nodes] in: those in [zone] first (when given), each group in weighted-random
 * order, a node's weight its relative chance to come next. [random] answers in [0, 1).
 */
fun orderNodes(
    nodes: List<DirectoryNode>,
    zone: String?,
    random: () -> Double = Math::random,
): List<String> {
    val near = if (zone == null) emptyList() else nodes.filter { it.zone == zone }
    val far = if (zone == null) nodes else nodes.filter { it.zone != zone }
    return weightedOrder(near, random) + weightedOrder(far, random)
}

private fun weightedOrder(
    nodes: List<DirectoryNode>,
    random: () -> Double,
): List<String> {
    val left = nodes.toMutableList()
    val order = mutableListOf<String>()
    while (left.isNotEmpty()) {
        var roll = random() * left.sumOf { it.weight }
        var pick = 0
        while (pick < left.size - 1 && roll >= left[pick].weight) roll -= left[pick++].weight
        order += left.removeAt(pick).url
    }
    return order
}
