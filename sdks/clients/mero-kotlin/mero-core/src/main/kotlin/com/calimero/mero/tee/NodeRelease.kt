package com.calimero.mero.tee

import com.calimero.mero.http.PlainJsonHttp
import com.calimero.mero.http.TransportSecurity
import com.calimero.mero.http.trimBase
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import okhttp3.OkHttpClient

/*
 * Trusting a TEE node's image by its signed release, fetched at run time from anywhere,
 * instead of shipping the release's `published-mrtds.json` with the app — port of mero-js
 * `src/sealed/release.ts`.
 *
 * Each mero-tee node release publishes `published-mrtds.json` (the measurements of its image)
 * with a cosign keyless bundle, signed by the mero-tee release workflow on GitHub Actions.
 * [verifySignedNodeRelease] checks that signature against the Sigstore trust root embedded in
 * this SDK, so the file can come from any source: the cloud's mirror, a mirror of your own, or
 * a file bundled with the app. None of them is trusted.
 *
 * What is trusted: the signer ([NODE_RELEASE_SIGNER], the identity core pins as
 * `NODE_RELEASE_IDENTITY`), the Sigstore public-good trust root, and the minimum release you
 * pass. What is not: the transport, the server that serves the release, and the node's choice
 * of release — a node can present any genuinely signed release at or above the minimum and no
 * other, and its quote must then match that release's measurements exactly.
 */

/** The workflow that signs every mero-tee node release, and the issuer of its identity. */
val NODE_RELEASE_SIGNER: SignerIdentity =
    SignerIdentity(
        identity = "https://github.com/calimero-network/mero-tee/.github/workflows/release-node-image-gcp.yaml@refs/heads/master",
        issuer = "https://token.actions.githubusercontent.com",
    )

/**
 * The public mirror of signed node releases, used when no other source is given. Only a
 * transport: what it serves is verified, so it can fail to answer but cannot make a release
 * trusted.
 */
const val DEFAULT_RELEASE_MIRROR: String = "https://cloud.calimero.network"

/**
 * Where Intel's collateral for a quote comes from by default: the cloud's mirror of the PCCS paths
 * [PccsCollateral] reads (mero-js `DEFAULT_COLLATERAL_MIRROR`). It only carries Intel-signed items,
 * each verified up to Intel's root CA, so it cannot forge anything.
 */
const val DEFAULT_COLLATERAL_MIRROR: String = "$DEFAULT_RELEASE_MIRROR/api/tee/pcs"

/**
 * A node release as the servers hand it out (`{"data": ...}`): the release's
 * `published-mrtds.json`, byte for byte, and its cosign bundle. Unverified until
 * [verifySignedNodeRelease] says otherwise.
 */
data class SignedNodeRelease(
    /** The release, e.g. `2.3.87`. It must be the signed file's `tag`. */
    val version: String,
    /** `published-mrtds.json`, exactly as signed. */
    val publishedMrtds: String,
    /** `published-mrtds.json.bundle.json`, as JSON text. */
    val bundle: String,
)

private val MEASUREMENTS = listOf("mrtd", "rtmr0", "rtmr1", "rtmr2", "rtmr3")
private val MEASUREMENT = Regex("^[0-9a-fA-F]{96}$")
private val releaseJson = Json { ignoreUnknownKeys = true }

/**
 * Verify a node release's `published-mrtds.json` against its cosign [bundle] (JSON text or a
 * parsed [JsonElement]), as `cosign verify-blob` would, and return the file parsed. Throws
 * [TeeTrustException], with the reason, unless:
 *
 * - the signature over the exact bytes verifies with the bundle's certificate;
 * - that certificate chains to a Fulcio CA in the Sigstore trust root, and it and the CA were
 *   valid when Rekor logged the signature;
 * - it names the node release workflow ([identity], [issuer]) and GitHub Actions as its issuer;
 * - Rekor's signed entry timestamp verifies, for an entry that is this signature over this
 *   file by this certificate;
 * - the file is a node release (`role: "node"`) listing measured profiles, and
 *   [expectedVersion], when given, is its `tag`.
 *
 * Nothing is read from the clock, so a release that verifies once always does.
 *
 * @param signature the release's detached `published-mrtds.json.sig`, when you have it; it
 *   must be the bundle's.
 */
@Suppress("LongParameterList")
fun verifySignedNodeRelease(
    publishedMrtds: ByteArray,
    bundle: Any,
    expectedVersion: String? = null,
    identity: String? = null,
    issuer: String? = null,
    signature: String? = null,
): PublishedMrtds {
    Sigstore.verifyCosignBundle(
        publishedMrtds,
        bundle,
        SignerIdentity(identity ?: NODE_RELEASE_SIGNER.identity, issuer ?: NODE_RELEASE_SIGNER.issuer),
        signature,
    )
    val release =
        try {
            releaseJson.parseToJsonElement(utf8(publishedMrtds))
        } catch (_: Exception) {
            refuse("The signed release is not JSON")
        } as? JsonObject
    val role = release?.get("role")
    if (release == null || (role as? JsonPrimitive)?.takeIf { it.isString }?.content != "node") {
        refuse("The signed release is a ${role?.toString() ?: "undefined"} release, not a node one")
    }
    // The workflow signs other files too, some with a `role` and `profiles` of their own
    // (`release-provenance.json`): the signature covers bytes, not a file name.
    val profiles = (release["profiles"] as? JsonObject)?.values.orEmpty()
    val measured = { image: JsonElement ->
        MEASUREMENTS.all { register -> (image as? JsonObject).string(register)?.let(MEASUREMENT::matches) == true }
    }
    val tag = release.string("tag")
    if (tag == null || profiles.isEmpty() || !profiles.all(measured)) {
        refuse("The signed file is not a release's published-mrtds.json: it lists no measured profiles")
    }
    if (expectedVersion != null && tag != expectedVersion) refuse("The signed release is $tag, not $expectedVersion")
    return PublishedMrtds.fromJson(release)
}

/** [verifySignedNodeRelease] of the file as text (its UTF-8 bytes are what was signed). */
@Suppress("LongParameterList")
fun verifySignedNodeRelease(
    publishedMrtds: String,
    bundle: Any,
    expectedVersion: String? = null,
    identity: String? = null,
    issuer: String? = null,
    signature: String? = null,
): PublishedMrtds = verifySignedNodeRelease(publishedMrtds.encodeToByteArray(), bundle, expectedVersion, identity, issuer, signature)

/**
 * Fetch a [SignedNodeRelease] from a server that wraps it as `{"data": ...}`, such as the
 * cloud's [cloudNodeReleaseUrl]. It is only fetched: pass it to [trustSignedRelease] (or
 * [verifySignedNodeRelease]) before using it.
 */
suspend fun fetchNodeRelease(
    url: String,
    httpClient: OkHttpClient = PlainJsonHttp.defaultClient(),
): SignedNodeRelease {
    val response = PlainJsonHttp(httpClient, role = null).execute("GET", url)
    if (!response.isSuccessful) refuse("Fetching the node release from $url failed: HTTP ${response.status}")
    val data =
        try {
            (releaseJson.parseToJsonElement(response.body) as? JsonObject)?.get("data") as? JsonObject
        } catch (_: Exception) {
            refuse("$url did not answer with JSON")
        }
    val version = data.string("version")
    val publishedMrtds = data.string("publishedMrtds")
    val bundle = data.string("bundle")
    if (version == null || publishedMrtds == null || bundle == null) refuse("$url did not answer with a signed node release")
    return SignedNodeRelease(version, publishedMrtds, bundle)
}

/** Where mero-tee publishes its releases' assets, as core's `calimero-tee-release` fetches them. */
const val GITHUB_RELEASES: String = "https://github.com/calimero-network/mero-tee/releases/download"

/**
 * Node release [version] straight from its GitHub release (`mero-tee-v{version}`):
 * `published-mrtds.json` and its cosign bundle, the files core's Rust client verifies (mero-js
 * `fetchGithubNodeRelease`). Only fetched: verify it with [trustSignedRelease].
 */
suspend fun fetchGithubNodeRelease(
    version: String,
    base: String = GITHUB_RELEASES,
    httpClient: OkHttpClient = PlainJsonHttp.defaultClient(),
): SignedNodeRelease {
    val tag = "mero-tee-v${releaseVersion(version, "version")}"
    val url = "${base.trimBase()}/$tag/published-mrtds.json"
    val http = PlainJsonHttp(httpClient, role = null)

    suspend fun get(target: String): String {
        val response = http.execute("GET", target)
        if (!response.isSuccessful) refuse("Fetching $target failed: HTTP ${response.status}")
        return response.body
    }
    return SignedNodeRelease(version, get(url), get("$url.bundle.json"))
}

/**
 * Where the cloud at [cloudUrl] (e.g. `https://cloud.calimero.network`) mirrors node release
 * [version]. Plain http is refused for anything but loopback ([TransportSecurity]).
 */
fun cloudNodeReleaseUrl(
    cloudUrl: String,
    version: String,
): String {
    TransportSecurity.checkServiceUrl(cloudUrl.trimBase(), "release mirror")
    return "${cloudUrl.trimBase()}/api/tee/node-releases/${releaseVersion(version, "version")}"
}

/**
 * The allowlists that trust one [profile] of a signed node [release], from an untrusted
 * source (mero-js `trustSignedRelease`): its signature is verified, its signed `tag` must be
 * `release.version` and no older than [minReleaseVersion], and then it is read as
 * [trustedMeasurementsFromReleases] reads a bundled file.
 *
 * ```kotlin
 * val version = claimedRelease(doc, minReleaseVersion = "2.3.87")
 * val release = fetchNodeRelease(cloudNodeReleaseUrl(DEFAULT_RELEASE_MIRROR, version))
 * val verifier = trustSignedRelease(release, PRODUCTION_PROFILE, minReleaseVersion = "2.3.87").verifier()
 * ```
 *
 * @param minReleaseVersion the oldest release accepted, e.g. `2.3.87`. Required: every release
 *   ever signed stays validly signed, so without a floor a node could present an old release,
 *   with a flaw since fixed, and a quote from that old image would pass.
 */
@Suppress("LongParameterList")
fun trustSignedRelease(
    release: SignedNodeRelease,
    profile: String,
    minReleaseVersion: String,
    allowDebugProfile: Boolean = false,
    identity: String? = null,
    issuer: String? = null,
): TrustedImages {
    val minimum = releaseVersion(minReleaseVersion, "minReleaseVersion")
    val version = releaseVersion(release.version, "The release version")
    val published = verifySignedNodeRelease(release.publishedMrtds, release.bundle, version, identity, issuer)
    if (compareReleaseVersions(version, minimum) < 0) refuse("Release $version is older than the minimum trusted, $minimum")
    return trustedMeasurementsFromReleases(listOf(published), profile, allowDebugProfile)
}

/** What [createSignedReleaseVerifier] trusts; mero-js `SignedReleaseVerifierOptions`. */
@Suppress("LongParameterList")
class SignedReleaseVerifierOptions(
    /**
     * Where to get the node's release, asked again each time the node is attested (a restarted
     * node may run a newer one). Untrusted: it is verified, and the quote must match it.
     */
    val release: suspend () -> SignedNodeRelease,
    /** The oldest release accepted; see [trustSignedRelease]. */
    val minReleaseVersion: String,
    /** The image profile to trust; [PRODUCTION_PROFILE] unless [allowDebugProfile]. */
    val profile: String = PRODUCTION_PROFILE,
    val allowDebugProfile: Boolean = false,
    /** Override the signer; defaults to [NODE_RELEASE_SIGNER]. */
    val identity: String? = null,
    val issuer: String? = null,
    val dcapVerify: DcapVerify = Dcap,
    val fetchCollateral: (suspend (ByteArray) -> DcapCollateral)? = null,
    /** The current time in milliseconds, for the quote's collateral. For tests. */
    val now: () -> Long = System::currentTimeMillis,
)

/**
 * A [createQuoteVerifier] whose trusted image is the signed release the node says it runs
 * (mero-js `createSignedReleaseVerifier`). On each attestation it gets the release, checks it
 * with [trustSignedRelease], and verifies the quote against that release's measurements only.
 * [com.calimero.mero.CloudPolicy.verifyQuote] takes one.
 */
fun createSignedReleaseVerifier(options: SignedReleaseVerifierOptions): QuoteVerifier {
    releaseVersion(options.minReleaseVersion, "minReleaseVersion")
    return quoteVerifier { args ->
        trustSignedRelease(
            options.release(),
            options.profile,
            options.minReleaseVersion,
            options.allowDebugProfile,
            options.identity,
            options.issuer,
        ).verifier(options.dcapVerify, options.fetchCollateral, options.now)
            .verify(args)
    }
}

private val RELEASE_VERSION = Regex("^(\\d+)\\.(\\d+)\\.(\\d+)(?:-([0-9A-Za-z.-]+))?(?:\\+[0-9A-Za-z.-]+)?$")

/** [value] if it is a release version (`2.3.87`, `2.3.88-rc.1`); throws otherwise. */
internal fun releaseVersion(
    value: String?,
    what: String,
): String {
    val match = value?.let { RELEASE_VERSION.find(it) }
    val pre = match?.groups?.get(4)?.value
    if (match == null || (pre != null && pre.split('.').any { it.isEmpty() })) {
        refuse("$what is ${value?.let(::jsonString) ?: "undefined"}, not a release version like 2.3.87")
    }
    return value
}

/**
 * Order two release versions as semver does, and as core's `compare_release_versions`:
 * numerically by `X.Y.Z`, a pre-release before its release, build metadata ignored.
 */
@Suppress("ReturnCount")
internal fun compareReleaseVersions(
    a: String,
    b: String,
): Int {
    val (aCore, aPre) = splitVersion(a)
    val (bCore, bPre) = splitVersion(b)
    for (i in 0 until 3) {
        if (aCore[i] != bCore[i]) return aCore[i].compareTo(bCore[i])
    }
    if (aPre == null || bPre == null) {
        return when {
            aPre == bPre -> 0
            aPre == null -> 1
            else -> -1
        }
    }
    val aIds = aPre.split('.')
    val bIds = bPre.split('.')
    for (i in 0 until maxOf(aIds.size, bIds.size)) {
        val x = aIds.getOrNull(i) ?: return -1
        val y = bIds.getOrNull(i) ?: return 1
        val xNum = x.all(Char::isDigit)
        val yNum = y.all(Char::isDigit)
        val order =
            when {
                xNum && yNum -> x.toBigInteger().compareTo(y.toBigInteger())
                xNum != yNum -> if (xNum) -1 else 1
                else -> x.compareTo(y).coerceIn(-1, 1)
            }
        if (order != 0) return order
    }
    return 0
}

private fun splitVersion(version: String): Pair<List<java.math.BigInteger>, String?> {
    val match = checkNotNull(RELEASE_VERSION.find(version))
    return listOf(match.groupValues[1], match.groupValues[2], match.groupValues[3]).map { it.toBigInteger() } to match.groups[4]?.value
}
