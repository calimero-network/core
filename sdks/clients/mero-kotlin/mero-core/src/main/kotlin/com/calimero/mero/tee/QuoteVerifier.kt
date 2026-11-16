package com.calimero.mero.tee

import com.calimero.mero.crypto.Hex
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put
import org.bouncycastle.util.encoders.Base64

/*
 * Verifying a TEE node's quote on the device, before trusting the key it binds — port of
 * mero-js `src/sealed/verify.ts`.
 *
 * A quote proves what runs in a TD only once it has been checked: its signatures against
 * Intel's root, the TCB status of the platform, the measurements of the image, and that its
 * report data is the nonce and the binding this client asked for. [createQuoteVerifier] does
 * all of that with nothing but the node: the node hands over the Intel-signed collateral with
 * its quote (`includeCollateral`), and a node cannot forge collateral, only choose which valid
 * collateral to serve. The DCAP verification itself is [DcapVerify]; the SDK's own is [Dcap],
 * a port of `@phala/dcap-qvl`, the library mero-js is handed.
 */

/**
 * The collateral a quote is verified against — TCB info, QE identity, CRLs and their chains —
 * in dcap-qvl's form (`QuoteCollateralV3`). What a node returned with
 * `includeCollateral`. Byte fields arrive as hex or as arrays of numbers; [fromJson] takes both.
 */
@Suppress("LongParameterList")
class DcapCollateral(
    val pckCrlIssuerChain: String,
    val rootCaCrl: ByteArray,
    val pckCrl: ByteArray,
    val tcbInfoIssuerChain: String,
    /** The TCB info JSON, exactly as Intel signed it. */
    val tcbInfo: String,
    val tcbInfoSignature: ByteArray,
    val qeIdentityIssuerChain: String,
    /** The QE identity JSON, exactly as Intel signed it. */
    val qeIdentity: String,
    val qeIdentitySignature: ByteArray,
    /** The PCK certificate chain (PEM) when the quote does not carry one. */
    val pckCertificateChain: String? = null,
) {
    /** The dcap-qvl JSON form, byte fields as hex. */
    fun toJson(): JsonObject =
        buildJsonObject {
            put("pck_crl_issuer_chain", pckCrlIssuerChain)
            put("root_ca_crl", Hex.encode(rootCaCrl))
            put("pck_crl", Hex.encode(pckCrl))
            put("tcb_info_issuer_chain", tcbInfoIssuerChain)
            put("tcb_info", tcbInfo)
            put("tcb_info_signature", Hex.encode(tcbInfoSignature))
            put("qe_identity_issuer_chain", qeIdentityIssuerChain)
            put("qe_identity", qeIdentity)
            put("qe_identity_signature", Hex.encode(qeIdentitySignature))
            pckCertificateChain?.let { put("pck_certificate_chain", it) }
        }

    companion object {
        /** Read dcap-qvl's JSON form. Throws [TeeTrustException] naming the first field that is not there. */
        fun fromJson(element: JsonElement): DcapCollateral {
            val o = element as? JsonObject ?: refuse("The collateral is not a JSON object")

            fun text(key: String): String = o.string(key) ?: refuse("The collateral has no $key")

            fun bytes(key: String): ByteArray =
                when (val v = o[key]) {
                    is JsonPrimitive ->
                        v.takeIf { it.isString }?.content?.let { runCatching { Hex.decodeUnsized(it, key) }.getOrNull() }
                    is JsonArray -> runCatching { ByteArray(v.size) { i -> byteOf((v[i] as JsonPrimitive).intOrNull) } }.getOrNull()
                    else -> null
                } ?: refuse("The collateral's $key is not hex or bytes")
            return DcapCollateral(
                pckCrlIssuerChain = text("pck_crl_issuer_chain"),
                rootCaCrl = bytes("root_ca_crl"),
                pckCrl = bytes("pck_crl"),
                tcbInfoIssuerChain = text("tcb_info_issuer_chain"),
                tcbInfo = text("tcb_info"),
                tcbInfoSignature = bytes("tcb_info_signature"),
                qeIdentityIssuerChain = text("qe_identity_issuer_chain"),
                qeIdentity = text("qe_identity"),
                qeIdentitySignature = bytes("qe_identity_signature"),
                pckCertificateChain = (o["pck_certificate_chain"] as? JsonPrimitive)?.takeIf { it !is JsonNull && it.isString }?.content,
            )
        }

        private fun byteOf(value: Int?): Byte {
            require(value != null && value in 0..255)
            return value.toByte()
        }
    }
}

/** An SGX enclave report: a QE's report, or an SGX quote's body (dcap-qvl `EnclaveReport`). */
@Suppress("LongParameterList")
class SgxEnclaveReport(
    val cpuSvn: ByteArray,
    val miscSelect: Long,
    val attributes: ByteArray,
    val mrEnclave: ByteArray,
    val mrSigner: ByteArray,
    val isvProdId: Int,
    val isvSvn: Int,
    val reportData: ByteArray,
)

/** A TDX 1.0 TD report, the body of a TDX quote (dcap-qvl `TDReport10`). */
@Suppress("LongParameterList")
class TdReport10(
    val teeTcbSvn: ByteArray,
    val mrSeam: ByteArray,
    val mrSignerSeam: ByteArray,
    val seamAttributes: ByteArray,
    val tdAttributes: ByteArray,
    val xfam: ByteArray,
    val mrTd: ByteArray,
    val mrConfigId: ByteArray,
    val mrOwner: ByteArray,
    val mrOwnerConfig: ByteArray,
    val rtMr0: ByteArray,
    val rtMr1: ByteArray,
    val rtMr2: ByteArray,
    val rtMr3: ByteArray,
    val reportData: ByteArray,
) {
    /** A copy with other TD attributes, for tests of the debug check. */
    fun withTdAttributes(tdAttributes: ByteArray): TdReport10 =
        TdReport10(
            teeTcbSvn, mrSeam, mrSignerSeam, seamAttributes, tdAttributes, xfam, mrTd, mrConfigId, mrOwner, mrOwnerConfig,
            rtMr0, rtMr1, rtMr2, rtMr3, reportData,
        )
}

/** A TDX 1.5 TD report: the 1.0 report and two more fields (dcap-qvl `TDReport15`). */
class TdReport15(
    val base: TdReport10,
    val teeTcbSvn2: ByteArray,
    val mrServiceTd: ByteArray,
)

/** A quote's body, by type: dcap-qvl's `{type: 'sgx' | 'td10' | 'td15', data}`. */
sealed class DcapReport(
    /** `sgx`, `td10` or `td15`, as dcap-qvl names it. */
    val type: String,
) {
    class Sgx(
        val report: SgxEnclaveReport,
    ) : DcapReport("sgx")

    class Td10(
        val report: TdReport10,
    ) : DcapReport("td10")

    class Td15(
        val report: TdReport15,
    ) : DcapReport("td15")

    /** The TD 1.0 part of a TDX report, or null for SGX (dcap-qvl `asTd10`). */
    val td10: TdReport10?
        get() =
            when (this) {
                is Td10 -> report
                is Td15 -> report.base
                is Sgx -> null
            }
}

/** What a DCAP verifier returns for a quote whose signature chain verified. */
class DcapVerifiedReport(
    /** The merged TCB status, e.g. `UpToDate`. */
    val status: String,
    val advisoryIds: List<String> = emptyList(),
    val report: DcapReport,
)

/**
 * Verify a quote's signature chain against `collateral` as of `nowSecs`, and appraise its TCB
 * level; throw when it does not verify. [Dcap] is the SDK's own; mero-js passes `verify` from
 * `@phala/dcap-qvl`, which has exactly this shape.
 */
fun interface DcapVerify {
    fun verify(
        quote: ByteArray,
        collateral: DcapCollateral,
        nowSecs: Long,
    ): DcapVerifiedReport
}

/** One image's measurements, all five registers, hex. A quote matches only when every register does. */
data class TrustedMeasurements(
    val mrtd: String,
    val rtmr0: String,
    val rtmr1: String,
    val rtmr2: String,
    val rtmr3: String,
)

/** What a [VerifyTransportQuote] is asked: does [quoteB64] verify, with report data `nonce ‖ reportDataSuffix` (both hex)? */
data class TransportQuote(
    val quoteB64: String,
    val nonce: String,
    val reportDataSuffix: String,
    /** What the node sent with the quote, when the verifier asked for it. */
    val collateral: DcapCollateral? = null,
)

/**
 * Decides whether a TEE node's quote is one you trust (mero-js `VerifyTransportQuote`):
 * verify [TransportQuote.quoteB64] — never by asking the node being attested — and check its
 * report data is `nonce ‖ reportDataSuffix`. Return true only when the quote is genuine, its
 * measurements are ones you accept and the report data matches; returning false or throwing
 * refuses it. [createQuoteVerifier] does all of this on the device.
 */
fun interface VerifyTransportQuote {
    suspend fun verify(quote: TransportQuote): Boolean

    /**
     * Ask the node for the collateral its quote verifies against, passed in as
     * [TransportQuote.collateral]. Off unless overridden, because a node that predates the
     * field refuses the request.
     */
    val includeCollateral: Boolean get() = false
}

/** A verifier that asks the node for collateral (mero-js `QuoteVerifier`). */
interface QuoteVerifier : VerifyTransportQuote {
    override val includeCollateral: Boolean get() = true
}

/** [verify] as a [QuoteVerifier]. */
fun quoteVerifier(verify: suspend (TransportQuote) -> Boolean): QuoteVerifier =
    object : QuoteVerifier {
        override suspend fun verify(quote: TransportQuote): Boolean = verify(quote)
    }

/** What [createQuoteVerifier] trusts; mero-js `QuoteVerifierOptions`. */
@Suppress("LongParameterList")
class QuoteVerifierOptions(
    /** DCAP verification. Defaults to the SDK's own, [Dcap]. */
    val dcapVerify: DcapVerify = Dcap,
    /**
     * The images trusted, each by all of its measurements. The way to say which code may read
     * the traffic: build it with [trustedMeasurementsFromReleases].
     */
    val allowedMeasurements: List<TrustedMeasurements>? = null,
    /**
     * MRTDs (hex) accepted. On its own this does not name an image: on GCP the MRTD measures
     * the platform's TD firmware, which every image shares, and the image is in RTMR1–3. So a
     * verifier needs either [allowedMeasurements], or this with all of [allowedRtmr1]–[allowedRtmr3].
     */
    val allowedMrtd: List<String>? = null,
    /** RTMR allowlists (hex); a register is not checked when its list is left out. */
    val allowedRtmr0: List<String>? = null,
    val allowedRtmr1: List<String>? = null,
    val allowedRtmr2: List<String>? = null,
    val allowedRtmr3: List<String>? = null,
    /** TCB statuses accepted. Defaults to `UpToDate`. `Revoked` is refused even when listed. */
    val allowedTcbStatuses: List<String>? = null,
    /**
     * Where to get collateral when the node sends none (it predates `includeCollateral`, or it
     * is an agent gate): for example [PccsCollateral.fetch]. Without it such a quote is refused.
     */
    val fetchCollateral: (suspend (ByteArray) -> DcapCollateral)? = null,
    /** The current time in milliseconds. For tests. */
    val now: () -> Long = System::currentTimeMillis,
)

private val MEASUREMENT_HEX = Regex("^[0-9a-f]{96}$")

/**
 * A verifier that checks a TEE node's quote on the device and accepts it only when every
 * check passes (mero-js `createQuoteVerifier`):
 *
 * - its signatures verify against Intel's root, with collateral that is valid now;
 * - the platform's TCB status is one allowed (`UpToDate` by default), and never `Revoked`;
 * - it is not a debug TD (TUD bit 0), whatever [QuoteVerifierOptions.dcapVerify] allows;
 * - its measurements are those of a trusted image: one of `allowedMeasurements` in full, and
 *   the MRTD and any RTMRs listed;
 * - its report data is the nonce followed by the binding the client computed.
 *
 * Any failure throws [TeeTrustException] with the reason. Mock quotes never pass.
 *
 * @throws TeeTrustException when the options would accept nothing, or hold a value that is not
 *   a 48-byte hex measurement.
 */
@Suppress("CyclomaticComplexMethod")
fun createQuoteVerifier(options: QuoteVerifierOptions): QuoteVerifier {
    val allowedMrtd = measurements(options.allowedMrtd.orEmpty(), "allowedMrtd")
    val allowedImages =
        options.allowedMeasurements.orEmpty().mapIndexed { index, image ->
            val label = "allowedMeasurements[$index]"
            listOf("mrtd" to image.mrtd, "rtmr0" to image.rtmr0, "rtmr1" to image.rtmr1, "rtmr2" to image.rtmr2, "rtmr3" to image.rtmr3)
                .joinToString(",") { (register, value) -> measurements(listOf(value), "$label.$register")[0] }
        }
    val allowedRtmr =
        listOf(
            measurements(options.allowedRtmr0.orEmpty(), "allowedRtmr0"),
            measurements(options.allowedRtmr1.orEmpty(), "allowedRtmr1"),
            measurements(options.allowedRtmr2.orEmpty(), "allowedRtmr2"),
            measurements(options.allowedRtmr3.orEmpty(), "allowedRtmr3"),
        )
    if (options.allowedMeasurements != null && allowedImages.isEmpty()) {
        refuse("allowedMeasurements is empty, so no quote would ever be accepted")
    }
    if (allowedImages.isEmpty()) {
        if (allowedMrtd.isEmpty()) refuse("No image is trusted: give allowedMeasurements, or allowedMrtd with allowedRtmr1-3")
        if (allowedRtmr.drop(1).any { it.isEmpty() }) {
            refuse(
                "allowedMrtd alone does not name an image: the MRTD measures the TD firmware, which " +
                    "images share, and the image is in RTMR1-3. Give allowedMeasurements, or allowedRtmr1-3 too",
            )
        }
    }
    val allowedStatuses = options.allowedTcbStatuses ?: listOf("UpToDate")

    return quoteVerifier { args ->
        val quote =
            try {
                Base64.decode(args.quoteB64)
            } catch (_: Exception) {
                refuse("The quote is not base64")
            }
        val against =
            args.collateral ?: options.fetchCollateral?.invoke(quote)
                ?: refuse("The node sent no collateral for its quote, and no fetchCollateral was given to get it")

        val verified =
            try {
                options.dcapVerify.verify(quote, against, Math.floorDiv(options.now(), 1000L))
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                throw TeeTrustException("The quote did not verify: ${e.message ?: e.toString()}", e)
            }

        if (verified.status == "Revoked" || verified.status !in allowedStatuses) {
            refuse("The platform's TCB status is ${verified.status}, which is not accepted")
        }
        val td =
            when (val report = verified.report) {
                is DcapReport.Td10 -> report.report
                is DcapReport.Td15 -> report.report.base
                is DcapReport.Sgx -> refuse("The quote is a ${report.type} quote, not a TDX one")
            }
        // TUD bit 0. A debug TD's memory and registers are open to the host, so its measurements
        // prove nothing about what runs in it. Checked here, not left to `dcapVerify`, which a
        // caller may swap for one that allows debug; attributes that cannot be read count as debug.
        if (td.tdAttributes.size != 8 || td.tdAttributes[0].toInt() and 0x01 != 0) {
            refuse("The quote is from a debug TD, or its TD attributes cannot be read")
        }
        val measured = listOf(td.mrTd, td.rtMr0, td.rtMr1, td.rtMr2, td.rtMr3).map(Hex::encode)
        if (allowedImages.isNotEmpty() && measured.joinToString(",") !in allowedImages) {
            refuse(
                "The measurements (MRTD ${measured[0]}, RTMR0-3 ${measured.drop(1).joinToString(", ")}) are not an image this verifier trusts",
            )
        }
        if (allowedMrtd.isNotEmpty() && measured[0] !in allowedMrtd) refuse("MRTD ${measured[0]} is not an image this verifier trusts")
        measured.drop(1).forEachIndexed { index, register ->
            val allowed = allowedRtmr[index]
            if (allowed.isNotEmpty() && register !in allowed) refuse("RTMR$index $register is not one this verifier trusts")
        }
        if (Hex.encode(td.reportData) != "${args.nonce}${args.reportDataSuffix}".lowercase()) {
            refuse("The quote does not commit to this nonce and binding")
        }
        true
    }
}

private fun measurements(
    values: List<String>,
    label: String,
): List<String> =
    values.map { value ->
        val normalized = value.trim().lowercase()
        if (!MEASUREMENT_HEX.matches(normalized)) refuse("$label holds ${jsonString(value)}, which is not a 48-byte hex measurement")
        normalized
    }

/** One profile of a release's image, as `published-mrtds.json` lists it. */
data class PublishedProfile(
    val mrtd: String,
    val rtmr0: String,
    val rtmr1: String,
    val rtmr2: String,
    val rtmr3: String,
    /** The TCB statuses the release accepts for it, in any case; `uptodate` when absent. */
    val allowedTcbStatuses: List<String>? = null,
)

/**
 * A mero-tee node release's `published-mrtds.json`: the measurements the release workflow
 * observed booting each profile of its image on TDX. Only the fields read here are kept.
 */
data class PublishedMrtds(
    val role: String? = null,
    val tag: String? = null,
    val profiles: Map<String, PublishedProfile>,
) {
    companion object {
        /** Read the file's JSON; a profile whose registers are not strings is left out. */
        fun fromJson(element: JsonElement): PublishedMrtds {
            val o = element as? JsonObject ?: refuse("The release is not a JSON object")
            val profiles = LinkedHashMap<String, PublishedProfile>()
            (o["profiles"] as? JsonObject)?.forEach { (name, value) ->
                val p = value as? JsonObject ?: return@forEach
                val registers = listOf("mrtd", "rtmr0", "rtmr1", "rtmr2", "rtmr3").map { p.string(it) }
                if (registers.any { it == null }) return@forEach
                val statuses = (p["allowed_tcb_statuses"] as? JsonArray)?.map { (it as? JsonPrimitive)?.content.orEmpty() }
                profiles[name] = PublishedProfile(registers[0]!!, registers[1]!!, registers[2]!!, registers[3]!!, registers[4]!!, statuses)
            }
            return PublishedMrtds(o.string("role"), o.string("tag"), profiles)
        }
    }
}

/** The allowlists [trustedMeasurementsFromReleases] builds: hand them to [QuoteVerifierOptions]. */
data class TrustedImages(
    val allowedMeasurements: List<TrustedMeasurements>,
    val allowedTcbStatuses: List<String>,
) {
    /** A [createQuoteVerifier] that trusts exactly these images. */
    fun verifier(
        dcapVerify: DcapVerify = Dcap,
        fetchCollateral: (suspend (ByteArray) -> DcapCollateral)? = null,
        now: () -> Long = System::currentTimeMillis,
    ): QuoteVerifier =
        createQuoteVerifier(
            QuoteVerifierOptions(
                dcapVerify = dcapVerify,
                allowedMeasurements = allowedMeasurements,
                allowedTcbStatuses = allowedTcbStatuses,
                fetchCollateral = fetchCollateral,
                now = now,
            ),
        )
}

/**
 * The one image profile a release publishes for production. The `debug` and
 * `debug-read-only` images are signed and published beside it, but run with SSH and the
 * operator's tooling inside the TD, so trusting one trusts an image the operator can read.
 */
const val PRODUCTION_PROFILE: String = "locked-read-only"

/** TCB statuses as DCAP reports them; the release lists them in lower case. */
internal val TCB_STATUSES =
    listOf(
        "UpToDate",
        "SWHardeningNeeded",
        "ConfigurationNeeded",
        "ConfigurationAndSWHardeningNeeded",
        "OutOfDate",
        "OutOfDateConfigurationNeeded",
    )

/**
 * The allowlists that trust exactly one [profile] of the given [releases]: each release's
 * image by all of its measurements, and only the TCB statuses every one of them accepts
 * (mero-js `trustedMeasurementsFromReleases`). Pass every release a node you talk to may run:
 * during a rollout, the old one and the new one.
 *
 * Where the file comes from is the trust decision. Shipping it with the app (checked out of a
 * release you verified) trusts that release; for one fetched at run time, verify it first
 * with [verifySignedNodeRelease] — [trustSignedRelease] does both.
 *
 * @param allowDebugProfile trust a profile other than [PRODUCTION_PROFILE]. Its image is
 *   readable by whoever operates the node, so only for testing.
 */
fun trustedMeasurementsFromReleases(
    releases: List<PublishedMrtds>,
    profile: String,
    allowDebugProfile: Boolean = false,
): TrustedImages {
    if (releases.isEmpty()) refuse("No release given, so no image would be trusted")
    var statuses: List<String>? = null
    val allowed =
        releases.mapIndexed { index, release ->
            val name = release.tag ?: "releases[$index]"
            if (release.role != null && release.role != "node") {
                refuse("$name lists the measurements of a ${release.role}, not of a node image")
            }
            val image =
                release.profiles[profile]
                    ?: refuse("$name has no profile ${jsonString(profile)} (it has: ${release.profiles.keys.joinToString(", ").ifEmpty { "none" }})")
            if (profile != PRODUCTION_PROFILE && !allowDebugProfile) {
                refuse(
                    "${jsonString(profile)} is not a production profile: its image is readable by whoever " +
                        "operates the node. Trust ${jsonString(PRODUCTION_PROFILE)}, or pass allowDebugProfile to test",
                )
            }
            val accepted = (image.allowedTcbStatuses ?: listOf("uptodate")).map { tcbStatus(it, name) }
            statuses = statuses?.filter { it in accepted } ?: accepted
            TrustedMeasurements(image.mrtd, image.rtmr0, image.rtmr1, image.rtmr2, image.rtmr3)
        }
    val common = statuses
    if (common.isNullOrEmpty()) refuse("The releases given accept no TCB status in common")
    return TrustedImages(allowed, common)
}

private fun tcbStatus(
    status: String,
    release: String,
): String =
    TCB_STATUSES.firstOrNull { it.equals(status, ignoreCase = true) }
        ?: refuse("$release accepts TCB status ${jsonString(status)}, which this verifier does not know")
