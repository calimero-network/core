package com.calimero.mero

import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.fromBase64
import com.calimero.mero.crypto.randomBytes
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
 */

/** The oldest mero-tee release core's Rust client trusts by default (`attest::DEFAULT_MIN_RELEASE`). */
const val DEFAULT_MIN_RELEASE: String = "2.3.109"

/** The mock quote format development clusters use. */
const val MOCK_QUOTE_HEADER: String = "MOCK_TDX_QUOTE_V1"

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
    /** The image profile trusted; defaults to `locked-read-only`. */
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
    /** Override the release signer (defaults to the mero-tee node release workflow). */
    val identity: String? = null,
    val issuer: String? = null,
    /** The current time in milliseconds. For tests. */
    val now: () -> Long = System::currentTimeMillis,
)

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
) {
    companion object {
        /** Mock attestation allowed: `merod local` and other `--dev` clusters only. */
        val development: CloudPolicy = CloudPolicy(allowMock = true)

        /** Real TDX quotes only, from a signed mero-tee release at or above [DEFAULT_MIN_RELEASE]. */
        val production: CloudPolicy = CloudPolicy(release = CloudReleasePolicy(DEFAULT_MIN_RELEASE))
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
)

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
            policy.profile,
            policy.minReleaseVersion,
            policy.allowDebugProfile,
            policy.identity,
            policy.issuer,
        )
    val fetchCollateral = freshCollateral(policy.fetchCollateral ?: defaultCollateral(policy.collateralSource, http), policy.maxCollateralAgeDays, policy.now)
    return trusted.verifier(policy.dcapVerify, fetchCollateral, policy.now).verify(quote)
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
    val suffix = transportKeyBinding(inner, transportKey)
    val quote = fromBase64(doc.quote)
    val header = MOCK_QUOTE_HEADER.encodeToByteArray()
    if (quote.size >= header.size && quote.copyOfRange(0, header.size).contentEquals(header)) {
        if (!policy.allowMock) throw TeeTrustException("mock quote refused: this client only trusts real TEEs")
        val report = quote.copyOfRange(header.size, minOf(quote.size, header.size + 64))
        if (report.size != 64 ||
            !report.copyOfRange(0, 32).contentEquals(Hex.decode(nonce, "nonce", 32)) ||
            !report.copyOfRange(32, 64).contentEquals(suffix)
        ) {
            throw TeeTrustException("the quote does not bind this transport key, app and nonce")
        }
        return transportKey
    }
    val args = TransportQuote(doc.quote, nonce, Hex.encode(suffix))
    val ok =
        when {
            policy.verifyQuote != null -> policy.verifyQuote.verify(args)
            policy.release != null -> verifyRelease(doc, args, policy.release, httpClient)
            else -> throw TeeTrustException("real quote refused: this client trusts no TD image (set policy.release)")
        }
    if (!ok) throw TeeTrustException("The attestation did not verify, so its transport key is not trusted")
    return transportKey
}
