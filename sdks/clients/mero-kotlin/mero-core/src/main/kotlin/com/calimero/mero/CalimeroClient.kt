package com.calimero.mero

import com.calimero.mero.api.facade.AppsFacade
import com.calimero.mero.api.facade.AuthFacade
import com.calimero.mero.api.facade.ConsentFacade
import com.calimero.mero.api.facade.DeploymentsFacade
import com.calimero.mero.api.facade.OpsFacade
import com.calimero.mero.api.facade.OrgsFacade
import com.calimero.mero.api.facade.PackagesFacade
import com.calimero.mero.api.facade.PublicFacade
import com.calimero.mero.api.facade.Routes
import com.calimero.mero.api.infrastructure.RequestConfig
import com.calimero.mero.api.infrastructure.Serializer
import com.calimero.mero.api.models.AppCreateRequest
import com.calimero.mero.api.models.IntentsRequest
import com.calimero.mero.api.models.RecoveryStartRequest
import com.calimero.mero.api.models.ViewsRequest
import com.calimero.mero.crypto.DeviceSigner
import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.fromBase64
import com.calimero.mero.crypto.randomBytes
import com.calimero.mero.http.MeroCallFactory
import com.calimero.mero.http.NetworkException
import com.calimero.mero.http.PlainJsonHttp
import com.calimero.mero.http.trimBase
import com.calimero.mero.tee.SealedTransport
import com.calimero.mero.tee.SealedTransportException
import com.calimero.mero.tee.StaleTransportKeyException
import com.calimero.mero.tee.TeeTrustException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.channelFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.encodeToJsonElement
import kotlinx.serialization.json.put
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okio.Buffer
import okio.BufferedSource
import java.io.IOException
import java.net.URLEncoder
import java.util.concurrent.TimeUnit

/** How a [CalimeroClient] reaches and trusts a cluster (mero-js `CalimeroClientOptions`). */
data class CalimeroClientOptions(
    /** One node of the cluster: the one the client talks to first. */
    val baseUrl: String,
    /**
     * Other nodes of the same cluster, in the order to move to them when a node answers
     * `421 WRONG_NODE` (its release may not serve this account), or stays unavailable to a call or
     * a view through all its retries. Each is attested under the same [policy] before anything is
     * sent to it. Without them a `421` reaches the caller as [WrongNode].
     */
    val nodes: List<String> = emptyList(),
    val policy: CloudPolicy,
    /** Carries the attestation and the sealed envelopes. */
    val httpClient: OkHttpClient = defaultHttpClient(),
    /** How calls and views retry a 503 or a failed connection. */
    val retry: CloudRetryOptions = CloudRetryOptions(),
    /**
     * Told every time the session changes: a sign-in, a refresh, a sign-out (null). Keep the
     * session here (encrypted) to restore it with [CalimeroClient.useSession]: a refresh token is
     * used once, so the one kept must always be the newest.
     */
    val onSession: ((CalimeroSession?) -> Unit)? = null,
    /**
     * Approves a `412 CONFIRM_REQUIRED` for the facade (`client.orgs.inviteCreate(...)`) and the
     * generated API, when a call is given no passkey of its own. Null: the [ConfirmRequired] reaches
     * the caller. Also [CalimeroClient.passkey].
     */
    val passkey: PasskeyProvider? = null,
    /**
     * Signs a `428 OPERATOR_SIGNATURE_REQUIRED` (operator decisions) for the facade and the generated
     * API, when a call is given no signer of its own. Also [CalimeroClient.operator].
     */
    val operator: DeviceSigner? = null,
) {
    companion object {
        /** 30 s to connect and to wait for an answer. */
        fun defaultHttpClient(): OkHttpClient = PlainJsonHttp.defaultClient(30_000)
    }
}

/** How [CalimeroClient.connect] reaches a Calimero Cloud cluster by name (mero-js `CalimeroConnectOptions`). */
data class CalimeroConnectOptions(
    /** The cluster's name, e.g. `eu-shared-1`. */
    val cluster: String,
    /** What every node must attest to before anything is sent to it. */
    val policy: CloudPolicy,
    /** The node directory to ask. Default [DEFAULT_DIRECTORY]. */
    val directory: String = DEFAULT_DIRECTORY,
    /** Try the nodes in this zone (e.g. `europe-west4-a`) first. */
    val zone: String? = null,
    /** Carries the directory, the attestation and the sealed envelopes. */
    val httpClient: OkHttpClient = CalimeroClientOptions.defaultHttpClient(),
    /** How calls and views retry a 503 or a failed connection. */
    val retry: CloudRetryOptions = CloudRetryOptions(),
    /** As [CalimeroClientOptions.onSession]. */
    val onSession: ((CalimeroSession?) -> Unit)? = null,
    /** Randomness for the weighted node order, in [0, 1). For tests. */
    val random: () -> Double = Math::random,
    /** The current time in milliseconds. For tests. */
    val now: () -> Long = System::currentTimeMillis,
)

/**
 * The client for Calimero 1.0, the confidential cloud (mero-js `CalimeroClient`): one app = one
 * replicated log on attested nodes. It verifies the node, seals every request to it
 * (`calimero/sealed-http/v2`), signs people in with passkeys and machines with API keys.
 *
 * ```kotlin
 * val cloud = CalimeroClient(CalimeroClientOptions(baseUrl, policy = CloudPolicy(release = CloudReleasePolicy("2.3.109"))))
 * cloud.signIn(PlatformPasskey(activity))
 * val (index, result) = cloud.call(appId, "set", buildJsonObject { put("key", "a"); put("value", "1") })
 * ```
 *
 * Every method is a `suspend` function that does its I/O on [Dispatchers.IO]. A refusal throws
 * the [CalimeroError] it means ([errorFrom]); a node that does not attest throws
 * [TeeTrustException].
 */
@Suppress("TooManyFunctions", "LargeClass")
class CalimeroClient(
    val options: CalimeroClientOptions,
) {
    /** A client for one node, trusting it under [policy]. */
    constructor(baseUrl: String, policy: CloudPolicy) : this(CalimeroClientOptions(baseUrl, policy = policy))

    /** Every node this client may use, `baseUrl` first (or as the directory last ordered them). */
    @Volatile private var nodeUrls: List<String> = (listOf(options.baseUrl) + options.nodes).map { it.trimBase() }.distinct()

    /** The one in use: an index into [nodeUrls]. */
    @Volatile private var at = 0

    /** Where a client made by [connect] re-reads its node list. */
    private class DirectorySource(
        val options: CalimeroConnectOptions,
        /** When the listing is due to be read again (ms since the epoch). */
        @Volatile var due: Long,
        @Volatile var reading: Boolean = false,
    )

    @Volatile private var source: DirectorySource? = null

    private val channels = HashMap<String, SealedTransport>()
    private val lock = Any()

    @Volatile private var credential: String? = null

    @Volatile private var current: CalimeroSession? = null
    private var refreshing: Deferred<CalimeroSession>? = null
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val streaming: OkHttpClient by lazy {
        options.httpClient
            .newBuilder()
            .readTimeout(0, TimeUnit.MILLISECONDS)
            .build()
    }

    /** The node requests go to now: `baseUrl`, until a `421 WRONG_NODE` or a failover moved the client on. */
    val nodeUrl: String get() = synchronized(lock) { nodeUrls[at] }

    /** Every node this client may use, in the order it moves through them. */
    val nodes: List<String> get() = nodeUrls

    /** The session in use, or null (signed out, or an API key). */
    val session: CalimeroSession? get() = current

    /** Approves a `412 CONFIRM_REQUIRED` for a facade or generated-API call given no passkey of its own (see [CalimeroClientOptions.passkey]). */
    @Volatile var passkey: PasskeyProvider? = options.passkey

    /** Signs a `428 OPERATOR_SIGNATURE_REQUIRED` for a facade or generated-API call given no signer of its own (see [CalimeroClientOptions.operator]). */
    @Volatile var operator: DeviceSigner? = options.operator

    // ---------------------------------------------------------------- the generated client

    private val defaultApi: MeroApi by lazy { MeroApi(callFactory(StepUp())) }

    /**
     * The OpenAPI Generator client over this client's transport (sealed, attested, `401` refresh,
     * `421` failover, `503` retries of calls and views). A `412` is approved with [passkey] (else the
     * client's [CalimeroClient.passkey]) and a `428` signed with [operator] (else
     * [CalimeroClient.operator]); with neither, the typed error reaches the caller. Prefer the facade
     * ([orgs], [apps], ...), which takes named parameters instead of request models.
     */
    fun api(
        passkey: PasskeyProvider? = null,
        operator: DeviceSigner? = null,
    ): MeroApi = if (passkey == null && operator == null) defaultApi else MeroApi(callFactory(StepUp(passkey, operator)))

    /** The generated client for one facade call: its step-up overrides and the fields it sends as `null`. */
    private fun facadeApi(
        passkey: PasskeyProvider?,
        operator: DeviceSigner?,
        nulls: Set<String>,
    ): MeroApi = if (nulls.isEmpty()) api(passkey, operator) else MeroApi(callFactory(StepUp(passkey, operator, nulls)))

    private fun callFactory(override: StepUp): MeroCallFactory = MeroCallFactory(scope) { send(it, stepUp(override)) }

    private fun stepUp(override: StepUp): StepUp = StepUp(override.passkey ?: passkey, override.operator ?: operator, override.nulls)

    /** Core's account operations (`auth`): passkeys, sessions, API keys, aliases, recovery, usage. */
    val auth: AuthFacade by lazy { AuthFacade(::facadeApi) }

    /** Core's app operations: instances, calls and views, blobs, files, presence. */
    val apps: AppsFacade by lazy { AppsFacade(::facadeApi) }

    /** The node's consent page. */
    val consent: ConsentFacade by lazy { ConsentFacade(::facadeApi) }

    /** Operator routes (the ops token, [useApiKey]). */
    val ops: OpsFacade by lazy { OpsFacade(::facadeApi) }

    /** Organizations, members, teams, invites. */
    val orgs: OrgsFacade by lazy { OrgsFacade(::facadeApi) }

    /** Packages and their web origins. */
    val packages: PackagesFacade by lazy { PackagesFacade(::facadeApi) }

    /** What any node answers anyone: health, attestation, releases, publisher bindings. */
    val public: PublicFacade by lazy { PublicFacade(::facadeApi) }

    /** Sign-up policy and invites, deployers and deployments. */
    val deployments: DeploymentsFacade by lazy { DeploymentsFacade(::facadeApi) }

    /** The sealed channel to [url], attested under the client's policy before its first request. */
    private fun channel(url: String): SealedTransport =
        synchronized(lock) {
            channels.getOrPut(url) {
                SealedTransport(url, options.httpClient) { fetchAttestedKey(url, options.policy, options.httpClient) }
            }
        }

    /**
     * Move on from node [from] (the one that answered `421 WRONG_NODE`) to the next one. False when
     * there is no other. A request that saw the same node refuse after another request moved on
     * already does not move again.
     */
    private fun moveOn(from: String): Boolean =
        synchronized(lock) {
            if (nodeUrls.size < 2) return false
            if (nodeUrls[at] == from) at = (at + 1) % nodeUrls.size
            true
        }

    /**
     * Fail over from node [from], where a request's retries ran out, to the next node it has not
     * [tried]. False when it has tried them all. A request whose node another request moved away
     * from already goes to that node, if untried.
     */
    private fun failOver(
        from: String,
        tried: Set<String>,
    ): Boolean {
        synchronized(lock) {
            val urls = nodeUrls
            if (urls[at] != from && urls[at] !in tried) return true
            for (step in 1 until urls.size) {
                val next = (at + step) % urls.size
                if (urls[next] !in tried) {
                    at = next
                    break
                }
            }
            if (urls[at] in tried) return false
        }
        reread()
        return true
    }

    /**
     * Read the node list again from the directory, in the background, for a client made by
     * [connect]: the node in use stays first, then the directory's order; a node no longer listed is
     * dropped. A failed read is ignored. One read at a time.
     */
    private fun reread() {
        val src = source ?: return
        synchronized(lock) {
            if (src.reading) return
            src.reading = true
        }
        scope.launch {
            try {
                val listing = readDirectory(src.options.directory, src.options.cluster, options.httpClient)
                val listed = orderNodes(listing.nodes, src.options.zone, src.options.random)
                synchronized(lock) {
                    val current = nodeUrls[at]
                    nodeUrls = listOf(current) + listed.filter { it != current }
                    at = 0
                }
                src.due = src.options.now() + (listing.refreshAfterSeconds * 1000).toLong()
            } catch (_: DirectoryError) {
                // Keep the list the client has.
            } finally {
                src.reading = false
            }
        }
    }

    /** Authenticate every request with an API key (`cal_…`, or an operator token). */
    fun useApiKey(key: String) {
        credential = key
        current = null
    }

    /** Use a session kept from before (an `onSession` callback's). */
    fun useSession(session: CalimeroSession) {
        credential = session.sessionToken
        current = session
    }

    @Volatile private var observer: ((CalimeroSession?) -> Unit)? = options.onSession

    /** Be told every time the session changes (replaces [CalimeroClientOptions.onSession]); null stops. */
    fun onSessionChange(observer: ((CalimeroSession?) -> Unit)?) {
        this.observer = observer
    }

    private fun takeSession(session: CalimeroSession) {
        useSession(session)
        observer?.invoke(session)
    }

    private fun dropSession() {
        credential = null
        current = null
        observer?.invoke(null)
    }

    // ---------------------------------------------------------------- transport

    private class Body(
        val bytes: ByteArray,
        val contentType: String,
    )

    private fun jsonBody(value: JsonElement): Body = Body(value.toString().encodeToByteArray(), "application/json")

    /** Send one request to the node in use, with the credential held now, and answer the response whatever its status. */
    @Suppress("ThrowsCount")
    private suspend fun request(
        method: String,
        path: String,
        body: Body?,
        stream: Boolean,
    ): Response =
        withContext(Dispatchers.IO) {
            source?.let { if (it.options.now() >= it.due) reread() }
            val url = nodeUrl
            val builder = Request.Builder().url("$url/$path")
            credential?.let { builder.header("authorization", "Bearer $it") }
            val payload = body?.let { it.bytes.toRequestBody(it.contentType.toMediaType()) }
            when (method) {
                "GET" -> builder.get()
                "DELETE" -> if (payload != null) builder.delete(payload) else builder.delete()
                else -> builder.method(method, payload ?: ByteArray(0).toRequestBody("application/json".toMediaType()))
            }
            val sealed = channel(url)
            val client = if (stream) streaming.newBuilder().apply { interceptors().add(0, sealed) }.build() else sealed.client
            try {
                client.newCall(builder.build()).execute()
            } catch (e: SealedTransportException) {
                val cause = e.cause
                if (cause is TeeTrustException) throw cause
                // The node refuses an envelope past its 64 MiB sealed cap with `413` in the clear,
                // before opening it: to the caller it is the API's `TOO_LARGE`.
                if (e.status == 413) {
                    throw errorFrom(
                        413,
                        buildJsonObject {
                            put("error", "TOO_LARGE")
                            put("message", (e.message ?: "").removePrefix("Sealed request refused: "))
                        },
                    )
                }
                throw e
            }
        }

    /**
     * [request], answering what a lost issuing node or a node on the wrong release does to a
     * session: a successful response, or the refusal as a typed error.
     *
     * - `401 UNAUTHENTICATED` to a request that carried a session: the client refreshes once,
     *   shared by every request that needs it at the same time ([refresh]), and repeats the request
     *   once. A refused refresh surfaces as [LoginRequired].
     * - `421 WRONG_NODE`: the client moves to the next node it knows and repeats the request once;
     *   with none, the [WrongNode] reaches the caller.
     *
     * Repeating is safe even for a request that is not idempotent: the node answers `401` and `421`
     * when it judges the credential, before it reads what the request asks. The exceptions are the
     * ceremonies that mint a session, never repeated here.
     */
    @Suppress("ComplexCondition", "LoopWithTooManyJumpStatements")
    private suspend fun exchange(
        method: String,
        path: String,
        body: Body? = null,
        stream: Boolean = false,
    ): Response {
        var refreshed = false
        var moved = false
        while (true) {
            val token = credential
            val from = nodeUrl
            val response = request(method, path, body, stream)
            if (response.isSuccessful) return response
            val error = refusal(response)
            if (error is WrongNode && !moved) {
                moved = true
                if (moveOn(from) && !ceremony(path)) continue
            } else if (error is LoginRequired && !refreshed && token != null && current != null && !ceremony(path)) {
                refreshed = true
                if (credential == token) refresh()
                continue
            }
            throw error
        }
    }

    private suspend fun json(
        method: String,
        path: String,
        body: Body? = null,
    ): JsonElement = parse(exchange(method, path, body))

    private suspend fun <T> typed(
        strategy: DeserializationStrategy<T>,
        method: String,
        path: String,
        body: Body? = null,
    ): T = wire.decodeFromJsonElement(strategy, json(method, path, body))

    /**
     * [json] for a call or a view: while the node answers 503 or cannot be reached, the same
     * request, with the same idempotency key, is sent again after [retryDelayMs] (or the pause the
     * node asked for, if longer), at most `retries` times. Every other answer is final.
     *
     * When a node's retries run out, the request fails over to the next node the client knows and
     * its retries start again there, with the same key: the key is kept in the app's replicated
     * log, so the cluster answers it once whichever node it reaches. Each node is tried once per
     * request. The last failure surfaces as [UnavailableError]: the node's 503, or reason `network`
     * for a connection that failed, whose `committed` is [Committed.Unknown].
     *
     * [once] sends the request exactly once, to the node in use (the confirmed attempt of
     * [callConfirmed], whose challenge is single-use).
     */
    @Suppress("ThrowsCount", "NestedBlockDepth")
    private suspend fun retriedExchange(
        method: String,
        path: String,
        body: Body?,
        once: Boolean = false,
    ): Response {
        val retries = if (once) 0 else options.retry.retries
        val tried = HashSet<String>()
        while (true) {
            var node: String
            var attempt = 0
            var unavailable: UnavailableError
            while (true) {
                node = nodeUrl
                tried += node
                unavailable =
                    try {
                        return exchange(method, path, body)
                    } catch (e: UnavailableError) {
                        e
                    } catch (e: SealedTransportException) {
                        // A node that cannot be reached to attest it is as unavailable as one
                        // that drops the connection; any other sealed-channel failure is final.
                        if (e.status != null || e.cause !is NetworkException) throw e
                        UnavailableError("NETWORK", "the node could not be reached: ${e.cause?.message}", cause = e)
                    } catch (e: StaleTransportKeyException) {
                        throw e
                    } catch (e: IOException) {
                        UnavailableError("NETWORK", "the node could not be reached: ${e.message}", cause = e)
                    }
                if (attempt >= retries) break
                val backoff = retryDelayMs(attempt, options.retry)
                delay(maxOf(backoff, ((unavailable.retryAfter ?: 0.0) * 1000).toLong()))
                attempt += 1
            }
            if (once || !failOver(node, tried)) throw unavailable
        }
    }

    /** A generated request (on [MeroApi.API_BASE]), sent through [dispatch] to the node in use. */
    private suspend fun send(
        request: Request,
        stepUp: StepUp,
    ): Response {
        val bytes = request.body?.let { b -> Buffer().also { b.writeTo(it) }.readByteArray() }
        val path = relativePath(request.url)
        val route = path.substringBefore('?')
        val binary =
            Routes.binaryBody.entries
                .firstOrNull { it.key.matches(request.method, route) }
                ?.value
        val type = binary ?: request.body?.contentType()?.let { "${it.type}/${it.subtype}" }
        var body = if (bytes == null || (bytes.isEmpty() && type == null)) null else Body(bytes, type ?: OCTET_STREAM)
        if (stepUp.nulls.isNotEmpty()) {
            // The generated request leaves every null out; these the caller set to `null` ([Explicit]).
            val json = body?.let { parseOrNull(it.bytes.decodeToString()) as? JsonObject } ?: JsonObject(emptyMap())
            body = jsonBody(JsonObject(json + stepUp.nulls.filterNot(json::containsKey).associateWith { JsonNull }))
        }
        return dispatch(request.method, path, body, stepUp)
    }

    /**
     * Every request, generated or hand-written: [exchange] (`401` refresh, `421` failover), with the
     * `503` retries and failover of [retriedExchange] for calls and views (a call given no
     * idempotency key gets one, shared by its retries), then the step-ups: a
     * `412 CONFIRM_REQUIRED` answered with [StepUp.passkey]'s assertion as `confirm`, a
     * `428 OPERATOR_SIGNATURE_REQUIRED` with [StepUp.operator]'s signature as `operatorProof`, each
     * once, in the request's JSON body. Without the one it needs, the typed error reaches the caller.
     */
    @Suppress("ThrowsCount", "ReturnCount", "LoopWithTooManyJumpStatements", "CyclomaticComplexMethod")
    private suspend fun dispatch(
        method: String,
        path: String,
        body: Body?,
        stepUp: StepUp,
    ): Response {
        val route = path.substringBefore('?')
        if (Routes.plain.any { it.matches(method, route) }) return plain(method, path)
        val retriable = Routes.retrySafe.any { it.matches(method, route) }
        var json = body?.takeIf { it.contentType.startsWith(JSON) }?.let { parseOrNull(it.bytes.decodeToString()) as? JsonObject }
        if (json != null && Routes.idempotencyKey.any { it.matches(method, route) } && !json.containsKey("idempotencyKey")) {
            json = JsonObject(json + ("idempotencyKey" to JsonPrimitive(newIdempotencyKey())))
        }
        var current = json?.let(::jsonBody) ?: body
        while (true) {
            val sent = json
            val binary = current != null && sent == null
            val field: Pair<String, JsonElement> =
                try {
                    return if (retriable) retriedExchange(method, path, current, once = sent?.containsKey("confirm") == true) else exchange(method, path, current)
                } catch (e: ConfirmRequired) {
                    val passkey = stepUp.passkey ?: throw e
                    if (binary || sent?.containsKey("confirm") == true) throw e
                    "confirm" to assertionFor(passkey, e.details)
                } catch (e: OperatorSignatureRequired) {
                    val signer = stepUp.operator ?: throw e
                    if (binary || sent?.containsKey("operatorProof") == true) throw e
                    "operatorProof" to
                        buildJsonObject {
                            put("nonce", e.nonce)
                            put("signature", Hex.encode(signer.sign(fromBase64(e.statement))))
                        }
                }
            val next = JsonObject(sent.orEmpty() + field)
            json = next
            current = jsonBody(next)
        }
    }

    /** A route the spec says travels in the clear (`x-calimero-sealed: false`: `/health`, `/attestation`), to the node in use. */
    private suspend fun plain(
        method: String,
        path: String,
    ): Response =
        withContext(Dispatchers.IO) {
            val response =
                options.httpClient
                    .newCall(
                        Request
                            .Builder()
                            .url("$nodeUrl/$path")
                            .method(method, null)
                            .build(),
                    ).execute()
            if (!response.isSuccessful) throw refusal(response)
            response
        }

    /** A passkey's `confirm` for a `412`'s [challenge]: `{ challengeId, assertion }`. A declined prompt is [ConfirmCancelled]. */
    private suspend fun assertionFor(
        passkey: PasskeyProvider,
        challenge: JsonObject,
    ): JsonObject {
        val assertion =
            try {
                passkey.get(
                    fromB64url(challenge.string("challenge").orEmpty()),
                    challenge.string("rpId").orEmpty(),
                    (challenge["allowCredentials"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content },
                )
            } catch (e: PasskeyCancelled) {
                throw ConfirmCancelled(e)
            }
        return buildJsonObject {
            put("challengeId", challenge.string("challengeId").orEmpty())
            put("assertion", wire.encodeToJsonElement(assertion))
        }
    }

    /** A successful response's JSON (`{}` for an empty body). */
    private suspend fun parse(response: Response): JsonElement {
        val text = withContext(Dispatchers.IO) { response.use { it.body?.string().orEmpty() } }
        return if (text.isBlank()) JsonObject(emptyMap()) else wire.parseToJsonElement(text)
    }

    /**
     * A generated operation's request ([config], from `api().<tag>.<op>RequestConfig(...)`) sent
     * through [dispatch], its answer as JSON: how the deprecated methods call the generated client
     * and still read the answer as they always did.
     */
    private suspend inline fun <reified I> raw(
        config: RequestConfig<I>,
        passkey: PasskeyProvider? = null,
        extra: JsonObject? = null,
    ): JsonElement {
        val encoded = config.body?.let { Serializer.kotlinxSerializationJson.encodeToJsonElement(it) as JsonObject }
        val merged = if (extra == null) encoded else JsonObject(encoded.orEmpty() + extra)
        return parse(dispatch(config.method.name, configPath(config), merged?.let(::jsonBody), stepUp(StepUp(passkey))))
    }

    /**
     * [config]'s route with [body] in place of the generated one: the binary uploads that keep the
     * caller's content type, or the raw bytes, as they always sent them.
     */
    private suspend fun binary(
        method: String,
        config: RequestConfig<*>,
        body: Body,
    ): JsonElement = parse(dispatch(method, configPath(config), body, stepUp(StepUp())))

    /** POST [body] to [path] (relative to the node, no leading slash) and answer the JSON. */
    private suspend fun post(
        path: String,
        body: JsonElement = JsonObject(emptyMap()),
    ): JsonElement = json("POST", path, jsonBody(body))

    private suspend fun get(path: String): JsonElement = json("GET", path)

    private suspend fun put(
        path: String,
        body: JsonElement = JsonObject(emptyMap()),
    ): JsonElement = json("PUT", path, jsonBody(body))

    private suspend fun delete(
        path: String,
        body: JsonElement? = null,
    ): JsonElement = json("DELETE", path, body?.let(::jsonBody))

    // ---------------------------------------------------------------- identity

    /**
     * Create an account with a new passkey, and sign in. The node then asks the new passkey to sign
     * the account it creates (`412 CONFIRM_REQUIRED`, answered here), so a sign-up prompts twice:
     * create, then sign.
     *
     * [SignUpOptions.invite] goes on both halves and on the retry; a sign-up the cluster's policy
     * refuses ([signUpPolicy]), or an unknown, expired or used invite, is [SignUpRefused], from
     * `begin`, before any passkey is made. [SignUpOptions.origin] names a registered web origin:
     * the passkey is made for its relying party and the session is bound to its package; an
     * unregistered one is [OriginNotRegistered].
     */
    suspend fun signUp(
        passkey: PasskeyProvider,
        name: String,
        options: SignUpOptions = SignUpOptions(),
    ): CalimeroSession {
        val begin =
            begin(
                "auth/passkey/register/begin",
                buildJsonObject {
                    put("name", name)
                    options.origin?.let { put("origin", it) }
                    options.invite?.let { put("invite", it) }
                },
                options.origin,
            )
        val registration = passkey.create(begin["options"] as? JsonObject ?: JsonObject(emptyMap()))
        val finished =
            postConfirmed(
                "auth/passkey/register/finish",
                buildJsonObject {
                    put("challengeId", begin.string("challengeId").orEmpty())
                    put("registration", wire.encodeToJsonElement(registration))
                    options.invite?.let { put("invite", it) }
                },
                passkey,
            )
        val session = wire.decodeFromJsonElement(CalimeroSession.serializer(), finished)
        takeSession(session)
        return session
    }

    /**
     * Sign in with an existing passkey. With [SignInOptions.origin] (a registered web origin) the
     * assertion is made for that origin's relying-party id, as the node answers it, and the session
     * is bound to the origin's package; an unregistered one is [OriginNotRegistered].
     */
    suspend fun signIn(
        passkey: PasskeyProvider,
        options: SignInOptions = SignInOptions(),
    ): CalimeroSession {
        val begin = begin("auth/passkey/login/begin", options.origin?.let { o -> buildJsonObject { put("origin", o) } }, options.origin)
        val opts = begin["options"] as? JsonObject ?: JsonObject(emptyMap())
        val rpId = opts.string("rpId") ?: (opts["rp"] as? JsonObject).string("id")
        val assertion = passkey.get(fromB64url(opts.string("challenge").orEmpty()), rpId.orEmpty())
        val session =
            typed(
                CalimeroSession.serializer(),
                "POST",
                "auth/passkey/login/finish",
                jsonBody(
                    buildJsonObject {
                        put("challengeId", begin.string("challengeId").orEmpty())
                        put("assertion", wire.encodeToJsonElement(assertion))
                    },
                ),
            )
        takeSession(session)
        return session
    }

    /** POST a ceremony's `begin`, naming [origin] in an [OriginNotRegistered] the node left it out of. */
    private suspend fun begin(
        path: String,
        body: JsonObject?,
        origin: String?,
    ): JsonObject =
        try {
            post(path, body ?: JsonObject(emptyMap())) as JsonObject
        } catch (e: OriginNotRegistered) {
            if (e.origin.isNotEmpty() || origin == null) throw e
            throw OriginNotRegistered(e.nodeMessage, JsonObject(e.details + ("origin" to JsonPrimitive(origin))))
        }

    /**
     * Swap the refresh token for a new session, on whichever node the client uses: any node of the
     * cluster takes it. A refresh token is used once, so calls made while one refresh is under way
     * share it rather than sending the same token again (which would revoke the session).
     */
    suspend fun refresh(): CalimeroSession {
        val pending =
            synchronized(lock) {
                refreshing ?: scope.async { exchangeRefresh() }.also { started ->
                    refreshing = started
                    started.invokeOnCompletion { synchronized(lock) { if (refreshing === started) refreshing = null } }
                }
            }
        return pending.await()
    }

    private suspend fun exchangeRefresh(): CalimeroSession {
        val session = checkNotNull(current) { "no session to refresh" }
        var tried = 1
        while (true) {
            try {
                val fresh =
                    typed(
                        CalimeroSession.serializer(),
                        "POST",
                        "auth/refresh",
                        jsonBody(buildJsonObject { put("refreshToken", session.refreshToken) }),
                    )
                // A sign-in or another session taken while this one was refreshing wins.
                if (current === session) takeSession(fresh)
                return fresh
            } catch (e: WrongNode) {
                // A node that may not serve the account answers 421 before it spends the refresh
                // token, so the same token goes to the next node (`send` moved there): once per node.
                if (tried >= nodeUrls.size) throw e
                tried += 1
            }
        }
    }

    /** End this session. */
    suspend fun signOut() {
        raw(defaultApi.accounts.logoutRequestConfig())
        dropSession()
    }

    /** End every session of the signed-in account, on every device, this one too. */
    suspend fun signOutEverywhere() {
        raw(defaultApi.accounts.logoutAllRequestConfig())
        dropSession()
    }

    /** Forget the session here without telling the node (it lapses by itself). */
    fun forget() = dropSession()

    /** The signed-in account: its passkeys, its organizations. */
    internal suspend fun me(): AccountInfo {
        val o = raw(defaultApi.accounts.meRequestConfig()) as JsonObject
        return AccountInfo(
            raw = o,
            accountId = o.string("accountId").orEmpty(),
            name = o.string("name").orEmpty(),
            passkeys = (o["passkeys"] as? JsonArray)?.let { wire.decodeFromJsonElement(ListSerializer(PasskeyInfo.serializer()), it) }.orEmpty(),
            orgs = (o["orgs"] as? JsonArray)?.let { wire.decodeFromJsonElement(ListSerializer(OrgMembership.serializer()), it) }.orEmpty(),
            via = o.string("via").orEmpty(),
            passkeyId = o.string("passkeyId"),
            app = o.string("app"),
            origin = o.string("origin"),
        )
    }

    /**
     * Add another passkey (another device, [passkey]) to the signed-in account; answers its
     * credential id. The node asks for the approval of a passkey the account already has
     * (`412 CONFIRM_REQUIRED`, never the new one), which [approver] gives.
     */
    suspend fun addPasskey(
        passkey: PasskeyProvider,
        approver: PasskeyProvider,
    ): String {
        val begin = post("auth/passkeys/add/begin") as JsonObject
        val registration = passkey.create(begin["options"] as? JsonObject ?: JsonObject(emptyMap()))
        val done =
            postConfirmed(
                "auth/passkeys/add/finish",
                buildJsonObject {
                    put("challengeId", begin.string("challengeId").orEmpty())
                    put("registration", wire.encodeToJsonElement(registration))
                },
                approver,
            ) as JsonObject
        return done.string("credentialId").orEmpty()
    }

    /**
     * The node's attestation document, unverified: for display only. Every request this client
     * sends is sealed to a key it verified itself first.
     */
    suspend fun attestation(): AttestationDoc =
        withContext(Dispatchers.IO) {
            val nonce = Hex.encode(randomBytes(32))
            val response = PlainJsonHttp(options.httpClient, role = null).execute("GET", "$nodeUrl/attestation?nonce=$nonce")
            if (!response.isSuccessful) throw errorFrom(response.status, parseOrNull(response.body), response.headers["retry-after"])
            wire.decodeFromString(AttestationDoc.serializer(), response.body)
        }

    // ---------------------------------------------------------------- apps

    /**
     * Upload a blob for [app]: `blobId` is base58 (what apps print), `id` hex. [contentType]
     * (printable ASCII; the node refuses another with `400`) is sent as the upload's
     * `Content-Type`: a component app records it and serves downloads with it. A component app
     * holding as many blobs as it may answers `429 QUOTA_EXCEEDED`.
     */
    suspend fun uploadBlob(
        app: String,
        data: ByteArray,
        contentType: String = OCTET_STREAM,
    ): BlobInfo = wire.decodeFromJsonElement(BlobInfo.serializer(), binary("POST", defaultApi.apps.blobUploadRequestConfig(app, NO_BYTES, null), Body(data, contentType)))

    /**
     * A blob of [app] (with [grant], one a run handed out: no session needed) with the content type the node
     * serves it as: the uploader's, for a component app; `application/octet-stream` otherwise.
     */
    suspend fun blobContent(
        app: String,
        id: String,
        grant: String? = null,
    ): BlobContent {
        val response = dispatch("GET", configPath(defaultApi.apps.blobDownloadRequestConfig(app, id, grant)), null, StepUp())
        return withContext(Dispatchers.IO) {
            response.use { BlobContent(it.body?.bytes() ?: ByteArray(0), it.header("content-type") ?: OCTET_STREAM) }
        }
    }

    // ---------------------------------------------------------------- packages

    // ---------------------------------------------------------------- app instances

    /** POST [body] to [path], answering a `412 CONFIRM_REQUIRED` with [passkey]'s assertion over the node's challenge. */
    private suspend fun postConfirmed(
        path: String,
        body: JsonObject,
        passkey: PasskeyProvider,
    ): JsonElement = confirmed(body, passkey) { json("POST", path, jsonBody(it)) }

    private suspend fun confirmed(
        body: JsonObject,
        passkey: PasskeyProvider,
        send: suspend (JsonObject) -> JsonElement,
    ): JsonElement {
        val challenge =
            try {
                return send(body)
            } catch (e: ConfirmRequired) {
                e.details
            }
        return send(JsonObject(body + ("confirm" to assertionFor(passkey, challenge))))
    }

    /** One app instance: its record (`name`, `owner`, `org`, `access`, `metadata`, ...). */
    internal suspend fun getApp(app: String): JsonObject = raw(defaultApi.apps.appGetRequestConfig(app)) as JsonObject

    /**
     * Create an app instance from a bundle hash, or from a package release (installed from the
     * registry first if the cluster lacks it), or from what a deployment offers
     * ([AppSource.Package.deployment]). Answers its app id. A failed init throws code
     * `INIT_FAILED`.
     */
    @Suppress("LongParameterList")
    internal suspend fun createApp(
        name: String,
        source: AppSource,
        org: String? = null,
        access: AppAccess? = null,
        confirmMethods: List<String>? = null,
        initArgs: JsonElement? = null,
        metadata: JsonElement? = null,
    ): String {
        val request =
            when (source) {
                is AppSource.Bundle -> AppCreateRequest(name = name, bundle = source.hash, org = org, access = access, initArgs = initArgs, metadata = metadata)
                is AppSource.Package ->
                    AppCreateRequest(
                        name = name,
                        `package` = source.`package`,
                        version = source.version,
                        service = source.service,
                        deployment = source.deployment,
                        org = org,
                        access = access,
                        initArgs = initArgs,
                        metadata = metadata,
                    )
            }
        // `confirmMethods` is not in core's OpenAPI document, so the generated request has no field for it.
        val extra = confirmMethods?.let { list -> buildJsonObject { put("confirmMethods", JsonArray(list.map(::JsonPrimitive))) } }
        return (raw(defaultApi.apps.appCreateRequestConfig(request), extra = extra) as JsonObject).string("appId").orEmpty()
    }

    /**
     * Call a method. A retry with the same [idempotencyKey] is answered once. Without one, the
     * client makes one, so its own retries of a 503 share it.
     */
    internal suspend fun call(
        app: String,
        method: String,
        args: JsonElement = JsonObject(emptyMap()),
        idempotencyKey: String? = null,
    ): CallResult {
        val request = IntentsRequest(method = method, args = args, idempotencyKey = idempotencyKey ?: newIdempotencyKey())
        return wire.decodeFromJsonElement(CallResult.serializer(), raw(defaultApi.apps.intentsRequestConfig(app, request)))
    }

    /** Run a read-only method: strong by default, or [stale] from any replica. */
    internal suspend fun view(
        app: String,
        method: String,
        args: JsonElement = JsonObject(emptyMap()),
        stale: Boolean = false,
        minIndex: Long = 0,
    ): ViewResult {
        val consistency = if (stale) ViewsRequest.Consistency.STALE else ViewsRequest.Consistency.STRONG
        val request = ViewsRequest(method = method, args = args, consistency = consistency, minIndex = minIndex)
        return wire.decodeFromJsonElement(ViewResult.serializer(), raw(defaultApi.apps.viewsRequestConfig(app, request)))
    }

    /** Committed events from index [from], then live, until the collector stops. */
    fun events(
        app: String,
        from: Long = 0,
    ): Flow<CloudEvent> = stream("apps/${seg(app)}/events?from=$from")

    /** A server-sent event stream on [path], sealed like every other answer. Cancelling the collector closes it. */
    private fun stream(path: String): Flow<CloudEvent> =
        channelFlow {
            val response = exchange("GET", path, stream = true)
            val source = response.body?.source() ?: throw CalimeroError(response.code, "EVENTS", "the event stream has no body")
            launch(Dispatchers.IO) {
                try {
                    sseFrames(source) { send(it) }
                    channel.close()
                } catch (e: IOException) {
                    if (isActive) channel.close(e)
                }
            }
            awaitClose { response.close() }
        }

    // ---------------------------------------------------------------- presence

    /** Presence as a stream: a `snapshot` event (the entries present), then a `presence` event per change. */
    fun presenceStream(app: String): Flow<CloudEvent> = stream("apps/${seg(app)}/presence/stream")

    // ---------------------------------------------------------------- aliases

    // ---------------------------------------------------------------- organizations

    /**
     * Start handing [org] to the signed-in account with the recovery [seed], which signs here and
     * never leaves this process. Finish with [finishRecovery] once [RecoveryTimes.readyAt] has
     * passed; until then the owner may [cancelRecovery]. A wrong seed (or no organization, or none
     * set) is `403 WRONG_RECOVERY_KEY`.
     */
    suspend fun startRecovery(
        org: String,
        seed: ByteArray,
    ): RecoveryTimes {
        val account = me().accountId
        val nonce = b64url(randomBytes(16))
        val signature = signRecovery(seed, org, account, nonce)
        val request = RecoveryStartRequest(nonce = nonce, signature = signature)
        return wire.decodeFromJsonElement(RecoveryTimes.serializer(), raw(defaultApi.organizations.recoveryStartRequestConfig(org, request)))
    }

    /** The organization, with the teams this caller reaches or manages. */
    internal suspend fun getOrg(org: String): OrgInfo = orgInfo(raw(defaultApi.organizations.orgGetRequestConfig(org)) as JsonObject)

    // ---------------------------------------------------------------- approvals

    /** One page of the report of a scope's apps: an organization's, or ([org] null) the account's own. */
    internal suspend fun appsReport(
        org: String?,
        page: PageOptions = PageOptions(),
    ): AppsReport {
        pageQuery(page) // refuses a limit outside 1..MAX_REPORT_PAGE
        val limit = page.limit?.toLong()
        val config =
            if (org == null) defaultApi.accounts.accountAppsRequestConfig(page.after, limit) else defaultApi.organizations.orgAppsRequestConfig(org, page.after, limit)
        return wire.decodeFromJsonElement(AppsReport.serializer(), raw(config))
    }

    /** [appsReport], every page, with the apps of all of them in `apps`. */
    suspend fun appsReportAll(org: String?): AppsReport {
        val first = appsReport(org)
        val apps = first.apps.toMutableList()
        var next = first.next
        while (next != null) {
            val page = appsReport(org, PageOptions(after = next))
            apps += page.apps
            next = page.next
        }
        return first.copy(apps = apps, next = null)
    }

    /** `GET /releases`: the cluster's governance and every release deadline. Public. */
    internal suspend fun releaseRules(): ReleaseRules = wire.decodeFromJsonElement(ReleaseRules.serializer(), raw(defaultApi.publicApi.releasesPublicRequestConfig()))

    /** What waits for a decision in a scope ([org], or the account's own apps with null). */
    suspend fun pendingApprovals(org: String? = null): PendingApprovals = pendingApprovalsFrom(appsReportAll(org), releaseRules())

    /**
     * Ask the node to ask the user (`POST /consent/requests`): something a package-bound session may
     * not do itself (approve a release, upgrade, outbound hosts; create an organization, mint or
     * accept an invitation), decided on the node's own consent page, never in the app. Show the
     * person the answer's `code`, then send them to its `url` (a Custom Tab: mero-compose's
     * `ConsentLauncher`, or [openConsentPage] for both steps). The page sends them back to
     * `returnTo?request_id=…&result=approved|declined` ([resumeConsent]), and [consentResult] (or
     * [waitForConsent]) confirms the decision and its [ConsentOutcome]. It does not check the
     * answer's `code`: [openConsentPage] does ([checkConsentCode]).
     */
    suspend fun requestConsent(request: ConsentRequest): ConsentStarted =
        wire.decodeFromJsonElement(ConsentStarted.serializer(), raw(defaultApi.consent.consentPageCreateRequestConfig(request)))

    /** [requestConsent], then hand the consent page's URL to [navigate] (open it in a Custom Tab). */
    suspend fun openConsentPage(
        request: ConsentRequest,
        navigate: (String) -> Unit,
    ): ConsentStarted = openConsentPage(request, OpenConsentOptions(navigate))

    /**
     * [requestConsent], then send the person to the consent page ([OpenConsentOptions.navigate]).
     * [OpenConsentOptions.confirm] runs first with the node's answer, so the app can show the
     * person its `code`; when it answers `false` the page is not opened. When the node sends a
     * `code` that is not [confirmationCode] of `requestId`, it throws [CalimeroError]
     * (`CONSENT_CODE_MISMATCH`) and does not navigate ([checkConsentCode]).
     */
    suspend fun openConsentPage(
        request: ConsentRequest,
        options: OpenConsentOptions,
    ): ConsentStarted {
        val started = requestConsent(request)
        checkConsentCode(started)
        if (options.confirm?.invoke(started) == false) return started
        options.navigate(started.url)
        return started
    }

    /**
     * The decision on a consent request (`GET /consent/{id}/result`; any node answers, for the
     * account that asked), with its typed [ConsentResult.outcome] once approved. Unlike the facade's
     * `consent.pageResult`, it reads kinds this SDK does not know as [ConsentOutcome.Other].
     */
    suspend fun consentResult(requestId: String): ConsentResult =
        wire.decodeFromJsonElement(ConsentResult.serializer(), raw(defaultApi.consent.consentPageResultRequestConfig(requestId)))

    /**
     * The link the consent page sent the person back with (the App Link the system handed the app):
     * the decision on [requestId], the request this app started ([ConsentStarted.requestId]), read
     * from the node ([consentResult]), never from the link. Null when [href] is not a consent return.
     * The link's `result` is never trusted; an approval's [ConsentResult.outcome] is the node's.
     *
     * Throws [CalimeroError] with code:
     * - [CONSENT_NOT_STARTED]: the link names a request, and [requestId] is null (the app started none);
     * - [CONSENT_REQUEST_MISMATCH]: the link, or the node's answer, names another request than [requestId];
     * - [CONSENT_NOT_DECIDED]: [requestId] is still pending although the page sent the person back,
     *   a sign that another request was approved on a page that was not the node's. Do not act on it;
     * - [CONSENT_REPLACED]: the node no longer knows [requestId] (`404`): an account has one undecided
     *   request per node, and a newer one replaced it, possibly on a page that was not the node's.
     *   Do not act on it either.
     */
    suspend fun resumeConsent(
        href: String,
        requestId: String?,
    ): ConsentResult? {
        val back = consentReturn(href) ?: return null
        if (requestId.isNullOrEmpty()) consentRefused(CONSENT_NOT_STARTED, "this app started no consent request", returned = back.requestId)
        if (back.requestId != requestId) {
            consentRefused(CONSENT_REQUEST_MISMATCH, "the consent page returned another request than this app started", requestId, back.requestId)
        }
        val result =
            try {
                consentResult(requestId)
            } catch (e: NotFound) {
                // One undecided request per account per node: a newer one replaced this one,
                // which then answers 404, perhaps swapped on a forged page.
                consentRefused(CONSENT_REPLACED, "this app's consent request was replaced by a newer one", requestId, status = 404, cause = e)
            }
        if (result.requestId != requestId) consentRefused(CONSENT_REQUEST_MISMATCH, "the node answered another request", requestId, result.requestId)
        if (result.isPending) consentRefused(CONSENT_NOT_DECIDED, "the consent page returned, but this request is still pending", requestId)
        return result
    }

    private fun consentRefused(
        code: String,
        message: String,
        requestId: String? = null,
        returned: String? = null,
        status: Int = 200,
        cause: Throwable? = null,
    ): Nothing =
        throw CalimeroError(
            status,
            code,
            message,
            buildJsonObject {
                requestId?.let { put("requestId", it) }
                returned?.let { put("returned", it) }
            },
            cause,
        )

    /**
     * Poll [consentResult] until the request is decided, every [intervalMs], until [expiresAt]
     * (Unix seconds) passes, when it answers the last pending result. Cancel the coroutine to stop.
     */
    suspend fun waitForConsent(
        requestId: String,
        intervalMs: Long = 2_000,
        expiresAt: Long? = null,
    ): ConsentResult {
        while (true) {
            val result = consentResult(requestId)
            if (!result.isPending) return result
            if (expiresAt != null && System.currentTimeMillis() / 1000 >= expiresAt) return result
            delay(intervalMs)
        }
    }

    /**
     * Poll [consentResult] every [intervalMs] until the request is decided or [timeout] passes, when
     * it answers the last (pending) result. A request lives 10 minutes on the node.
     */
    suspend fun waitForConsent(
        requestId: String,
        timeout: kotlin.time.Duration,
        intervalMs: Long = 2_000,
    ): ConsentResult {
        require(intervalMs > 0) { "intervalMs is positive" }
        val deadline = System.nanoTime() + timeout.inWholeNanoseconds
        while (true) {
            val result = consentResult(requestId)
            if (!result.isPending) return result
            val left = deadline - System.nanoTime()
            if (left <= 0) return result
            delay(minOf(intervalMs, (left + 999_999) / 1_000_000))
        }
    }

    // ---------------------------------------------------------------- operator

    // ---------------------------------------------------------------- members and teams

    // ---------------------------------------------------------------- package origins

    // ---------------------------------------------------------------- deployers

    // ---------------------------------------------------------------- deployments

    // ---------------------------------------------------------------- helpers

    private fun <T> listOf(
        key: String,
        item: kotlinx.serialization.KSerializer<T>,
        answer: JsonElement,
    ): List<T> = ((answer as? JsonObject)?.get(key) as? JsonArray)?.let { wire.decodeFromJsonElement(ListSerializer(item), it) }.orEmpty()

    private fun orgInfo(o: JsonObject): OrgInfo {
        fun map(key: String) = (o[key] as? JsonObject).orEmpty()
        return OrgInfo(
            raw = o,
            orgId = o.string("orgId").orEmpty(),
            name = o.string("name").orEmpty(),
            members = map("members").mapValues { (it.value as? JsonPrimitive)?.content.orEmpty() },
            teams = map("teams").mapValues { wire.decodeFromJsonElement(TeamInfo.serializer(), it.value) },
            metadata = o["metadata"] ?: JsonNull,
            memberMetadata = map("memberMetadata"),
            releaseApprovers = (o["releaseApprovers"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }.orEmpty(),
            publishes = (o["publishes"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }.orEmpty(),
            newWorkStopped = (o["newWorkStopped"] as? JsonObject)?.let { wire.decodeFromJsonElement(NewWorkStopped.serializer(), it) },
            recovery = (o["recovery"] as? JsonObject)?.let { wire.decodeFromJsonElement(OrgRecovery.serializer(), it) },
        )
    }

    companion object {
        /**
         * A client for the Calimero Cloud cluster named [CalimeroConnectOptions.cluster]: its node
         * list read from the public node directory, those in `zone` first, each group in
         * weighted-random order. The directory only picks which nodes to try: each is attested under
         * `policy` before anything is sent to it. Throws [DirectoryError] when the directory names no
         * usable node.
         *
         * The list is read again in the background after a failover, and at the first request once
         * the directory's `refreshAfterSeconds` has passed.
         */
        suspend fun connect(options: CalimeroConnectOptions): CalimeroClient {
            val listing = readDirectory(options.directory, options.cluster, options.httpClient)
            val order = orderNodes(listing.nodes, options.zone, options.random)
            val client =
                CalimeroClient(
                    CalimeroClientOptions(
                        baseUrl = order.first(),
                        nodes = order.drop(1),
                        policy = options.policy,
                        httpClient = options.httpClient,
                        retry = options.retry,
                        onSession = options.onSession,
                    ),
                )
            client.source = DirectorySource(options, options.now() + (listing.refreshAfterSeconds * 1000).toLong())
            return client
        }

        /**
         * Throw [CalimeroError] `CONSENT_CODE_MISMATCH` when the node answered a consent request with
         * a `code` that is not the one its page shows for that request ([confirmationCode]). A node
         * that predates the code (`""`) passes.
         */
        fun checkConsentCode(started: ConsentStarted) {
            if (started.code.isEmpty()) return
            val expected = confirmationCode(started.requestId)
            if (started.code != expected) {
                throw CalimeroError(
                    200,
                    "CONSENT_CODE_MISMATCH",
                    "the consent page code does not match the request",
                    buildJsonObject {
                        put("requestId", started.requestId)
                        put("code", started.code)
                        put("expected", expected)
                    },
                )
            }
        }

        /** [resumeConsent]: a consent return reached an app that started no request. */
        const val CONSENT_NOT_STARTED: String = "CONSENT_NOT_STARTED"

        /** [resumeConsent]: the return, or the node's answer, is for another request than the app's. */
        const val CONSENT_REQUEST_MISMATCH: String = "CONSENT_REQUEST_MISMATCH"

        /** [resumeConsent]: the page returned, but the app's request is still pending. */
        const val CONSENT_NOT_DECIDED: String = "CONSENT_NOT_DECIDED"

        /** [resumeConsent]: the node no longer knows the app's request (`404`): a newer one replaced it. */
        const val CONSENT_REPLACED: String = "CONSENT_REPLACED"

        internal const val OCTET_STREAM = "application/octet-stream"

        private const val JSON = "application/json"

        /** The bytes a generated binary upload's config is made with when [binary] sends its own. */
        private val NO_BYTES = ByteArray(0)

        /**
         * [url]'s path and query relative to the node, each path segment and query part encoded as
         * [seg] encodes it, whichever encoding the URL arrived in. An empty segment (an id that was
         * empty, `.` or `..`) is refused before anything is sent, so an id reaches no other route.
         */
        internal fun relativePath(url: HttpUrl): String {
            val segments = url.pathSegments
            require(segments.none { it.isEmpty() || it == "." || it == ".." }) { "an empty, '.' or '..' id in ${url.encodedPath}" }
            val path = segments.joinToString("/") { seg(it) }
            val query =
                (0 until url.querySize).joinToString("&") { i ->
                    val value = url.queryParameterValue(i)
                    if (value == null) enc(url.queryParameterName(i)) else "${enc(url.queryParameterName(i))}=${enc(value)}"
                }
            return if (query.isEmpty()) path else "$path?$query"
        }

        /** A generated [RequestConfig]'s route, relative to the node ([relativePath]). */
        internal fun configPath(config: RequestConfig<*>): String {
            val url =
                (MeroApi.API_BASE + config.path)
                    .toHttpUrl()
                    .newBuilder()
                    .apply { config.query.forEach { (name, values) -> values.forEach { addQueryParameter(name, it) } } }
                    .build()
            return relativePath(url)
        }

        internal val wire =
            Json {
                ignoreUnknownKeys = true
                coerceInputValues = true
                explicitNulls = false
                encodeDefaults = false
            }

        /** One URL path segment, so an id cannot reach another route. */
        internal fun seg(value: String): String = URLEncoder.encode(value, "UTF-8").replace("+", "%20")

        internal fun enc(value: String): String = seg(value)

        internal fun parseOrNull(text: String): JsonElement? = runCatching { wire.parseToJsonElement(text) }.getOrNull()

        private val JSON_LITERAL = Regex("^(true|false|null|-?\\d+(\\.\\d+)?([eE][+-]?\\d+)?)$")

        /** [text] as JSON only when it is JSON (the parser takes a bare word as a literal). */
        internal fun strictJson(text: String): JsonElement? =
            parseOrNull(text)?.takeUnless { it is JsonPrimitive && !it.isString && !JSON_LITERAL.matches(it.content) }

        /** The ceremonies that mint a session, which [exchange] never repeats by itself. */
        internal fun ceremony(path: String): Boolean = path == "auth/refresh" || path.startsWith("auth/passkey/")

        /**
         * A key for a call the caller gave none, made once so every retry shares it: `node:c-` and
         * 16 random bytes in base64url, as the Rust client mints it (kept 10 minutes by the node).
         */
        internal fun newIdempotencyKey(): String = "node:c-${b64url(randomBytes(16))}"

        internal fun callBody(
            method: String,
            args: JsonElement,
            key: String,
        ): JsonObject =
            buildJsonObject {
                put("method", method)
                put("args", args)
                put("idempotencyKey", key)
            }

        internal fun accessJson(access: AppAccess): JsonObject =
            buildJsonObject {
                when (access) {
                    AppAccess.Owner -> put("kind", "owner")
                    AppAccess.Org -> put("kind", "org")
                    AppAccess.Public -> put("kind", "public")
                    is AppAccess.Accounts -> {
                        put("kind", "accounts")
                        put("accounts", JsonArray(access.accounts.map(::JsonPrimitive)))
                    }
                    is AppAccess.Team -> {
                        put("kind", "team")
                        put("team", access.team)
                    }
                }
            }

        /** Read a refused response's body and map it ([errorFrom]). */
        internal fun refusal(response: Response): CalimeroError =
            response.use {
                val text = runCatching { it.body?.string().orEmpty() }.getOrDefault("")
                val body = if (text.isBlank()) JsonObject(emptyMap()) else parseOrNull(text) ?: buildJsonObject { put("message", text) }
                errorFrom(it.code, body, it.header("retry-after"))
            }

        /** Server-sent event frames: `{ id, kind, data }` each, `data` parsed as JSON when it is. */
        internal suspend fun sseFrames(
            source: BufferedSource,
            emit: suspend (CloudEvent) -> Unit,
        ) {
            var id = ""
            var kind = ""
            val data = StringBuilder()
            while (true) {
                val line = source.readUtf8Line() ?: return
                if (line.isEmpty()) {
                    if (kind.isNotEmpty() || data.isNotEmpty()) {
                        val text = data.toString()
                        emit(CloudEvent(id, kind, strictJson(text) ?: JsonPrimitive(text)))
                    }
                    id = ""
                    kind = ""
                    data.setLength(0)
                    continue
                }
                when {
                    line.startsWith("id:") -> id = line.substring(3).trim()
                    line.startsWith("event:") -> kind = line.substring(6).trim()
                    line.startsWith("data:") -> data.append(line.substring(5).trim())
                }
            }
        }
    }
}
