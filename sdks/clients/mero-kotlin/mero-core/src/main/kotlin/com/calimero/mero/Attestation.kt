package com.calimero.mero

import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.concat
import com.calimero.mero.crypto.fromBase64
import com.calimero.mero.crypto.randomBytes
import com.calimero.mero.crypto.sha256
import com.calimero.mero.http.PlainJsonHttp
import com.calimero.mero.http.trimBase
import com.calimero.mero.tee.DEFAULT_COLLATERAL_MIRROR
import com.calimero.mero.tee.DEFAULT_RELEASE_MIRROR
import com.calimero.mero.tee.Dcap
import com.calimero.mero.tee.DcapCollateral
import com.calimero.mero.tee.DcapVerify
import com.calimero.mero.tee.PRODUCTION_PROFILE
import com.calimero.mero.tee.PccsCollateral
import com.calimero.mero.tee.SignedNodeRelease
import com.calimero.mero.tee.TeeTrustException
import com.calimero.mero.tee.TransportQuote
import com.calimero.mero.tee.VerifyTransportQuote
import com.calimero.mero.tee.cloudNodeReleaseUrl
import com.calimero.mero.tee.compareReleaseVersions
import com.calimero.mero.tee.fetchGithubNodeRelease
import com.calimero.mero.tee.fetchNodeRelease
import com.calimero.mero.tee.parseIsoMillis
import com.calimero.mero.tee.releaseVersion
import com.calimero.mero.tee.transportKeyBinding
import com.calimero.mero.tee.trustSignedRelease
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.OkHttpClient

/*
 * The check every cloud client runs before it sends anything (mero-js `cloud-v2/attestation.ts`):
 * the node's quote must be genuine (or, in development, an explicitly allowed mock), from an image
 * this client trusts, and its report data must bind this nonce, the expected app and the transport
 * key the sealed channel will be opened to.
 *
 * Report data, as core's `calimero-cloud-client::attest` builds it:
 * `nonce(32) || SHA-256("calimero.tee-attest.transport-key.v1" | app_hash | transport_key)`.
 *
 * A real quote is judged as the Rust client judges it (`attest::verify` with
 * `Measurements::Release`): the node names the mero-tee release it runs; that release must be at
 * least `minReleaseVersion`; its `published-mrtds.json` is fetched and its Sigstore signature
 * checked against the node release workflow; the quote's signature chain and TCB status are checked
 * against Intel's collateral (fetched here: the node sends none, and cannot forge it); the TD must
 * not be a debug TD; and its registers must be the release's image for the trusted profile,
 * `locked-read-only` unless a debug profile is explicitly allowed. Nothing here reads the
 * environment: mock quotes pass only with `allowMock`.
 *
 * The client asks for the interval form (`form=interval`): the node answers from a quote it makes
 * once a minute over an Ed25519 key of its own and signs this request's nonce with that key, so no
 * quote is made per request. A node that does not know the form answers a quote over this nonce,
 * checked as above. Spec: https://calimero-network.github.io/core/protocol/tee-attestation/#interval-quotes
 */

/** The oldest mero-tee release core's Rust client trusts by default (`attest::DEFAULT_MIN_RELEASE`). */
const val DEFAULT_MIN_RELEASE: String = "2.3.109"

/** The mock quote format development clusters use. */
const val MOCK_QUOTE_HEADER: String = "MOCK_TDX_QUOTE_V1"

/** The `form` that asks `GET /attestation` for the interval form (core's `INTERVAL_FORM`). */
const val INTERVAL_FORM: String = "interval"

/** Domain of an interval quote's binding (core's `ATTEST_INTERVAL_KEY_DOMAIN`). */
const val INTERVAL_KEY_DOMAIN: String = "calimero.tee-attest.interval-key.v1"

/** Domain of what an interval key signs (core's `ATTEST_INTERVAL_SIG_DOMAIN`). */
const val INTERVAL_SIG_DOMAIN: String = "calimero.tee-attest.interval-sig.v1\u0000"

/** The oldest interval quote taken, by its `issuedAt` and this clock (core's `INTERVAL_QUOTE_MAX_AGE_SECS`). */
const val INTERVAL_QUOTE_MAX_AGE_SECS: Long = 300

/** How far ahead of this clock an interval quote may be dated (core's `INTERVAL_QUOTE_MAX_SKEW_SECS`). */
const val INTERVAL_QUOTE_MAX_SKEW_SECS: Long = 60

/** Where [CloudReleasePolicy] gets the release's signed measurements from. Untrusted either way. */
sealed interface ReleaseSource {
    /** The GitHub release assets, what core's Rust client reads. */
    data object Github : ReleaseSource

    /** A mirror serving [cloudNodeReleaseUrl]'s path (default [DEFAULT_RELEASE_MIRROR]). */
    data class Mirror(
        val baseUrl: String = DEFAULT_RELEASE_MIRROR,
    ) : ReleaseSource

    /** Anything else: a file bundled with the app, say. */
    fun interface Custom : ReleaseSource {
        suspend fun release(version: String): SignedNodeRelease
    }
}

/** [CloudReleasePolicy.collateralSource] for Intel's PCS itself ([PccsCollateral.INTEL_PCS_URL]). */
const val INTEL_COLLATERAL_SOURCE: String = "intel"

/** How the signed release a node names is trusted (core's `Measurements::Release`). */
data class CloudReleasePolicy(
    /**
     * The oldest mero-tee release trusted, e.g. `2.3.109`. Required: every release ever signed
     * stays validly signed, so without a floor a node could name an old release with a flaw since
     * fixed.
     */
    val minReleaseVersion: String,
    /** The image profile trusted; defaults to `locked-read-only`. Not with [profiles]. */
    val profile: String = PRODUCTION_PROFILE,
    /** Trust a debug profile. Its image is readable by the operator: testing only. */
    val allowDebugProfile: Boolean = false,
    /** Where the release's signed measurements come from (mero-js `releaseSource`). */
    val releaseSource: ReleaseSource = ReleaseSource.Mirror(),
    /** DCAP verification; defaults to the SDK's own, [Dcap]. */
    val dcapVerify: DcapVerify = Dcap,
    /** Intel's collateral for a quote; defaults to [PccsCollateral] from [collateralSource]. */
    val fetchCollateral: (suspend (ByteArray) -> DcapCollateral)? = null,
    /**
     * Where the default [fetchCollateral] gets Intel's collateral: a PCCS base URL (default
     * [DEFAULT_COLLATERAL_MIRROR], the cloud's mirror), or `"intel"` ([INTEL_COLLATERAL_SOURCE])
     * for Intel's PCS itself. Untrusted either way: every item is signed by Intel and verified up
     * to Intel's root CA.
     */
    val collateralSource: String = DEFAULT_COLLATERAL_MIRROR,
    /**
     * Refuse collateral whose TCB info Intel issued more than this many days ago. Null: only
     * Intel's own `nextUpdate` (about 30 days out) bounds how old it may be. A mirror cannot forge
     * collateral, but it can replay an older, still valid copy; this shortens that window. Too
     * small a value refuses every node if Intel is slow to re-sign.
     */
    val maxCollateralAgeDays: Double? = null,
    /**
     * Refuse collateral whose TCB info names Intel TCB evaluation data older than this. Raises
     * [com.calimero.mero.tee.DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER] (core's floor), never lowers it.
     */
    val minTcbEvaluationDataNumber: Int? = null,
    /** Override the release signer (defaults to the mero-tee node release workflow). */
    val identity: String? = null,
    val issuer: String? = null,
    /** The current time in milliseconds. For tests. */
    val now: () -> Long = System::currentTimeMillis,
    /**
     * Trust any of these image profiles, as core's `Measurements::Release` `allowed_profiles` and
     * mero-js `release.profiles` do: the one the node names is used when it is listed, and the
     * quote must still be that profile's signed image, so naming another profile only gets a node
     * refused. For a cluster whose profile the client does not know: staging's
     * ([CloudEnvironment.STAGING]). A listed debug profile still needs [allowDebugProfile]. When
     * set, [profile] must be left at its default.
     */
    val profiles: List<String>? = null,
)

/**
 * The profile a real quote is judged against: [CloudReleasePolicy.profile], or, with
 * [CloudReleasePolicy.profiles], the one the node names when it is listed (mero-js
 * `trustedProfile`). Every listed debug profile needs [CloudReleasePolicy.allowDebugProfile], so the
 * list never trusts one silently.
 */
@Suppress("ThrowsCount")
internal fun trustedProfile(
    doc: AttestationDoc,
    policy: CloudReleasePolicy,
): String {
    val profiles = policy.profiles ?: return policy.profile
    if (policy.profile != PRODUCTION_PROFILE) throw TeeTrustException("give CloudReleasePolicy.profile or profiles, not both")
    if (profiles.isEmpty()) throw TeeTrustException("CloudReleasePolicy.profiles is empty, so no image would be trusted")
    val debug = profiles.filter { it != PRODUCTION_PROFILE }
    if (debug.isNotEmpty() && !policy.allowDebugProfile) {
        throw TeeTrustException(
            "CloudReleasePolicy.profiles lists ${debug.joinToString(", ") { "\"$it\"" }}, which an operator can read: " +
                "pass allowDebugProfile to trust them",
        )
    }
    val named = doc.release.profile
    if (named !in profiles) {
        throw TeeTrustException("The node runs profile \"$named\", which is not one of ${profiles.joinToString(", ")}")
    }
    return named
}

/** What a [CalimeroClient] trusts a node by (mero-js `CloudPolicy`). */
data class CloudPolicy(
    /** Accept mock quotes. Development clusters only (`merod local`); never set from the environment. */
    val allowMock: Boolean = false,
    /** Trust real quotes from the signed release the node runs. */
    val release: CloudReleasePolicy? = null,
    /** Verify a real quote yourself instead (e.g. `createQuoteVerifier` with pinned measurements). Takes precedence over [release]. */
    val verifyQuote: VerifyTransportQuote? = null,
    /** The app bundle hash (hex) the quote must bind, when talking to one app. */
    val appHash: String? = null,
    /** The current time in milliseconds, which an interval quote's age is judged by. For tests. */
    val now: () -> Long = System::currentTimeMillis,
) {
    companion object {
        /** Mock attestation allowed: `merod local` and other `--dev` clusters only. */
        val development: CloudPolicy = CloudPolicy(allowMock = true)

        /** Real TDX quotes only, from a signed mero-tee release at or above [DEFAULT_MIN_RELEASE]. */
        val production: CloudPolicy = CloudEnvironment.PRODUCTION.policy()

        /**
         * A staging cluster's: real quotes only, from a signed mero-tee release at or above
         * [DEFAULT_MIN_RELEASE], of any of staging's image profiles, debug ones included
         * ([CloudEnvironment.STAGING]). Testing only: never in a build people install from a store.
         */
        val staging: CloudPolicy = CloudEnvironment.STAGING.policy()
    }
}

/** A node's `GET /attestation` answer. */
@Serializable
data class AttestationDoc(
    val quote: String,
    val transportKey: String,
    val appHash: String = "",
    val nonce: String = "",
    val release: AttestedRelease = AttestedRelease(),
    val cluster: String = "",
    val node: Long = 0,
    val mock: Boolean = false,
    /** With `form=interval`, from a node that has a current interval quote. */
    val interval: IntervalProof? = null,
)

/** What a `form=interval` answer adds (core's `IntervalProof`). */
@Serializable
data class IntervalProof(
    /** The interval key (Ed25519) the quote binds, hex. */
    val key: String,
    /** The first half of the quote's report data, which the node chose, hex. */
    val quoteNonce: String,
    /** When the node made the quote, Unix seconds by its clock. */
    val issuedAt: Long,
    /** Ed25519 over [intervalMessage], hex. */
    val signature: String,
)

private fun u64be(n: Long): ByteArray = ByteArray(8) { i -> (n ushr (56 - 8 * i)).toByte() }

/** An interval quote's report data after its nonce (core's `interval_key_binding`). */
fun intervalKeyBinding(
    transportKey: ByteArray,
    intervalKey: ByteArray,
    issuedAt: Long,
): ByteArray = sha256(concat(INTERVAL_KEY_DOMAIN.encodeToByteArray(), transportKey, intervalKey, u64be(issuedAt)))

/** What an interval key signs for one request (core's `interval_message`). */
fun intervalMessage(
    nonce: ByteArray,
    appHash: ByteArray,
    transportKey: ByteArray,
): ByteArray = concat(INTERVAL_SIG_DOMAIN.encodeToByteArray(), nonce, appHash, transportKey)

/**
 * The interval form (core's `verify_interval`): the quote is fresh by [CloudPolicy.now], the interval
 * key signed this [nonce], [appHash] and [transportKey], and the quote binds that key, the transport
 * key and the time. Throws [TeeTrustException] when it does not check out.
 */
@Suppress("ThrowsCount", "LongParameterList")
internal suspend fun verifyInterval(
    doc: AttestationDoc,
    interval: IntervalProof,
    nonce: ByteArray,
    appHash: ByteArray,
    transportKey: ByteArray,
    policy: CloudPolicy,
    httpClient: OkHttpClient,
) {
    val now = policy.now() / 1000
    if (interval.issuedAt > now + INTERVAL_QUOTE_MAX_SKEW_SECS) {
        throw TeeTrustException("The interval quote is dated ${interval.issuedAt - now}s ahead of this clock")
    }
    if (now - interval.issuedAt > INTERVAL_QUOTE_MAX_AGE_SECS) {
        throw TeeTrustException(
            "The interval quote is ${now - interval.issuedAt}s old, more than the ${INTERVAL_QUOTE_MAX_AGE_SECS}s this client takes",
        )
    }
    val key = Hex.decode(interval.key, "interval.key", 32)
    val signature = Hex.decode(interval.signature, "interval.signature", 64)
    if (!verifyStrict(key, intervalMessage(nonce, appHash, transportKey), signature)) {
        throw TeeTrustException("The interval key did not sign this nonce, app and transport key")
    }
    val quoteNonce = Hex.decode(interval.quoteNonce, "interval.quoteNonce", 32)
    verifyQuoteBinding(doc, quoteNonce, intervalKeyBinding(transportKey, key, interval.issuedAt), policy, "interval key", httpClient)
}

/** The release a node says it runs. Its claim only: the quote must match it. */
@Serializable
data class AttestedRelease(
    val version: String = "",
    val profile: String = "",
)

private val attestJson =
    Json {
        ignoreUnknownKeys = true
        coerceInputValues = true
    }

/**
 * The release a node names, checked against the floor before anything is fetched. Accepts
 * `2.3.109` and the tag form `mero-tee-v2.3.109`.
 */
fun claimedRelease(
    doc: AttestationDoc,
    minReleaseVersion: String,
): String {
    val floor = releaseVersion(minReleaseVersion, "minReleaseVersion")
    val raw =
        doc.release.version
            .trim()
            .removePrefix("mero-tee-v")
            .removePrefix("v")
    val version = releaseVersion(raw, "The release the node names")
    if (compareReleaseVersions(version, floor) < 0) throw TeeTrustException("mero-tee release $version is older than the minimum $floor")
    return version
}

private suspend fun releaseFor(
    version: String,
    policy: CloudReleasePolicy,
    http: OkHttpClient,
): SignedNodeRelease =
    when (val source = policy.releaseSource) {
        is ReleaseSource.Custom -> source.release(version)
        ReleaseSource.Github -> fetchGithubNodeRelease(version, httpClient = http)
        is ReleaseSource.Mirror -> fetchNodeRelease(cloudNodeReleaseUrl(source.baseUrl, version), http)
    }

/** The collateral fetch a policy gets unless it brings its own: [PccsCollateral] against [source]. */
private fun defaultCollateral(
    source: String,
    http: OkHttpClient,
): suspend (ByteArray) -> DcapCollateral {
    val url = if (source == INTEL_COLLATERAL_SOURCE) PccsCollateral.INTEL_PCS_URL else source
    return { quote -> PccsCollateral.fetch(quote, url, http) }
}

private const val MS_PER_DAY = 86_400_000.0

/**
 * [fetch], refusing collateral older than [maxCollateralAgeDays] (by its TCB info's `issueDate`).
 * Null: [fetch] itself, untouched (Intel's `nextUpdate` is checked when the quote is verified).
 */
fun freshCollateral(
    fetch: suspend (ByteArray) -> DcapCollateral,
    maxCollateralAgeDays: Double?,
    now: () -> Long = System::currentTimeMillis,
): suspend (ByteArray) -> DcapCollateral {
    if (maxCollateralAgeDays == null) return fetch
    return { quote ->
        val collateral = fetch(quote)
        val issued =
            runCatching {
                parseIsoMillis((attestJson.parseToJsonElement(collateral.tcbInfo) as JsonObject).string("issueDate")!!)
            }.getOrNull() ?: throw TeeTrustException("The quote's collateral names no TCB info issue date")
        val ageDays = (now() - issued) / MS_PER_DAY
        if (ageDays > maxCollateralAgeDays) {
            throw TeeTrustException(
                "The quote's collateral was issued ${ageDays.toLong()} days ago, more than the ${maxCollateralAgeDays.toBigDecimal().stripTrailingZeros().toPlainString()} this client accepts",
            )
        }
        collateral
    }
}

/** Judge a real quote as core's `Measurements::Release` does. */
private suspend fun verifyRelease(
    doc: AttestationDoc,
    quote: TransportQuote,
    policy: CloudReleasePolicy,
    http: OkHttpClient,
): Boolean {
    // Refuse what no release could admit before reaching for the network.
    val version = claimedRelease(doc, policy.minReleaseVersion)
    val trusted =
        trustSignedRelease(
            releaseFor(version, policy, http),
            trustedProfile(doc, policy),
            policy.minReleaseVersion,
            policy.allowDebugProfile,
            policy.identity,
            policy.issuer,
        )
    val fetchCollateral = freshCollateral(policy.fetchCollateral ?: defaultCollateral(policy.collateralSource, http), policy.maxCollateralAgeDays, policy.now)
    return trusted.verifier(policy.dcapVerify, fetchCollateral, policy.minTcbEvaluationDataNumber, policy.now).verify(quote)
}

/**
 * Check that [doc]'s quote binds [nonce] and then [suffix] (mero-js `verifyQuoteBinding`): a mock
 * quote only under [CloudPolicy.allowMock], a real one under [CloudPolicy.verifyQuote] or
 * [CloudPolicy.release]. [what] names the key it vouches for, [bound] what the suffix binds, for the errors. Throws
 * [TeeTrustException] when it does not check out.
 */
@Suppress("ThrowsCount")
internal suspend fun verifyQuoteBinding(
    doc: AttestationDoc,
    nonce: ByteArray,
    suffix: ByteArray,
    policy: CloudPolicy,
    what: String,
    httpClient: OkHttpClient,
    bound: String = what,
) {
    val quote =
        try {
            fromBase64(doc.quote)
        } catch (e: Exception) {
            throw TeeTrustException("The quote is not base64", e)
        }
    val header = MOCK_QUOTE_HEADER.encodeToByteArray()
    if (quote.size >= header.size && quote.copyOfRange(0, header.size).contentEquals(header)) {
        if (!policy.allowMock) throw TeeTrustException("mock quote refused: this client only trusts real TEEs")
        val report = quote.copyOfRange(header.size, minOf(quote.size, header.size + 64))
        if (report.size != 64 || !report.copyOfRange(0, 32).contentEquals(nonce) || !report.copyOfRange(32, 64).contentEquals(suffix)) {
            throw TeeTrustException("the quote does not bind this $bound and nonce")
        }
        return
    }
    val args = TransportQuote(doc.quote, Hex.encode(nonce), Hex.encode(suffix))
    val ok =
        when {
            policy.verifyQuote != null -> policy.verifyQuote.verify(args)
            policy.release != null -> verifyRelease(doc, args, policy.release, httpClient)
            else -> throw TeeTrustException("real quote refused: this client trusts no TD image (set policy.release)")
        }
    if (!ok) throw TeeTrustException("The attestation did not verify, so its $what is not trusted")
}

/**
 * Fetch the node's attestation and return its transport key once it checks out (mero-js
 * `fetchAttestedKey`). Throws [TeeTrustException] when it does not.
 */
@Suppress("ThrowsCount")
suspend fun fetchAttestedKey(
    baseUrl: String,
    policy: CloudPolicy,
    httpClient: OkHttpClient = PlainJsonHttp.defaultClient(),
): ByteArray {
    val nonce = Hex.encode(randomBytes(32))
    val url =
        "${baseUrl.trimBase()}/attestation"
            .toHttpUrl()
            .newBuilder()
            .addQueryParameter("nonce", nonce)
            .addQueryParameter("form", INTERVAL_FORM)
            .apply { policy.appHash?.let { addQueryParameter("appHash", it) } }
            .build()
            .toString()
    val response = PlainJsonHttp(httpClient, role = null).execute("GET", url)
    if (!response.isSuccessful) throw TeeTrustException("The node refused to attest: HTTP ${response.status}")
    val doc =
        try {
            attestJson.decodeFromString(AttestationDoc.serializer(), response.body)
        } catch (e: Exception) {
            throw TeeTrustException("The attestation response is malformed", e)
        }
    val transportKey = Hex.decode(doc.transportKey, "transportKey", 32)
    val inner = policy.appHash?.let { Hex.decode(it, "appHash", 32) } ?: ByteArray(32)
    val interval = doc.interval
    if (interval != null) {
        verifyInterval(doc, interval, Hex.decode(nonce, "nonce", 32), inner, transportKey, policy, httpClient)
        return transportKey
    }
    verifyQuoteBinding(doc, Hex.decode(nonce, "nonce", 32), transportKeyBinding(inner, transportKey), policy, "transport key", httpClient, bound = "transport key, app")
    return transportKey
}
