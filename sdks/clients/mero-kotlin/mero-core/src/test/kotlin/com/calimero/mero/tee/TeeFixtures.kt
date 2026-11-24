package com.calimero.mero.tee

import com.calimero.mero.crypto.Hex
import kotlinx.serialization.json.Json
import org.bouncycastle.util.encoders.Base64
import org.junit.Assert.assertTrue
import org.junit.Assert.fail

/**
 * The fixtures of mero-js `src/sealed/fixtures/` (copied byte for byte into
 * `fixtures/sealed/`): real mero-tee releases with their cosign bundles, and a real TDX quote
 * with the Intel-signed collateral for it, from dcap-qvl's own samples.
 */
internal object TeeFixtures {
    fun bytes(name: String): ByteArray =
        checkNotNull(TeeFixtures::class.java.classLoader?.getResourceAsStream("fixtures/$name")) { "missing fixture $name" }
            .use { it.readBytes() }

    fun text(name: String): String = String(bytes(name), Charsets.UTF_8)

    val QUOTE: ByteArray by lazy { bytes("sealed/tdx_quote.bin") }
    val COLLATERAL: DcapCollateral by lazy { DcapCollateral.fromJson(Json.parseToJsonElement(text("sealed/tdx_quote_collateral.json"))) }

    /**
     * The sample collateral is Intel's TCB evaluation data 17 (June 2025), below the floor every
     * verifier enforces ([DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER], 20). To test the other checks on
     * the real quote, [GENUINE] verifies its signatures against the genuine collateral while the
     * floor reads this: the same collateral naming 20 (only Intel can re-sign it).
     */
    val CURRENT: DcapCollateral by lazy { COLLATERAL.withEvaluationData(DEFAULT_MIN_TCB_EVALUATION_DATA_NUMBER) }

    /** [Dcap], against the genuine sample collateral when handed one that reads as [CURRENT]. */
    val GENUINE = DcapVerify { quote, collateral, nowSecs -> Dcap.verify(quote, if (collateral.tcbInfo == CURRENT.tcbInfo) COLLATERAL else collateral, nowSecs) }

    /** A moment inside that collateral's validity window (2025-06-27). */
    const val AT = 1_751_000_000_000L
    val QUOTE_B64: String by lazy { Base64.toBase64String(QUOTE) }

    /** The sample quote's own TD report, read once from the verified report. */
    val SAMPLE: DcapVerifiedReport by lazy { Dcap.verify(QUOTE, COLLATERAL, AT / 1000) }
    val SAMPLE_TD: TdReport10 by lazy { checkNotNull(SAMPLE.report.td10) }

    /** The sample's image: every register, as a release's published-mrtds.json lists it. */
    val IMAGE: TrustedMeasurements by lazy {
        TrustedMeasurements(
            Hex.encode(SAMPLE_TD.mrTd),
            Hex.encode(SAMPLE_TD.rtMr0),
            Hex.encode(SAMPLE_TD.rtMr1),
            Hex.encode(SAMPLE_TD.rtMr2),
            Hex.encode(SAMPLE_TD.rtMr3),
        )
    }
    val NONCE: String by lazy { Hex.encode(SAMPLE_TD.reportData.copyOfRange(0, 32)) }
    val SUFFIX: String by lazy { Hex.encode(SAMPLE_TD.reportData.copyOfRange(32, 64)) }
}

/**
 * This collateral as it reads naming Intel TCB evaluation data [number] instead of its own. Only
 * Intel can re-sign it, so its signatures no longer verify for any other number.
 */
internal fun DcapCollateral.withEvaluationData(number: Int): DcapCollateral =
    DcapCollateral(
        pckCrlIssuerChain = pckCrlIssuerChain,
        rootCaCrl = rootCaCrl,
        pckCrl = pckCrl,
        tcbInfoIssuerChain = tcbInfoIssuerChain,
        tcbInfo = tcbInfo.replace(Regex("\"tcbEvaluationDataNumber\":\\d+"), "\"tcbEvaluationDataNumber\":$number"),
        tcbInfoSignature = tcbInfoSignature,
        qeIdentityIssuerChain = qeIdentityIssuerChain,
        qeIdentity = qeIdentity,
        qeIdentitySignature = qeIdentitySignature,
        pckCertificateChain = pckCertificateChain,
    )

/** Run [block], which must throw an exception whose message contains [expected]. */
internal inline fun assertRefuses(
    expected: String,
    block: () -> Unit,
): Throwable {
    try {
        block()
    } catch (e: Throwable) {
        assertTrue("expected a message containing <$expected>, got <${e.message}>", e.message.orEmpty().contains(expected))
        return e
    }
    fail("expected a refusal containing <$expected>, but nothing was thrown")
    error("unreachable")
}
