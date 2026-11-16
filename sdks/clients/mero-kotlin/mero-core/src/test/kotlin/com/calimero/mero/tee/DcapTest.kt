package com.calimero.mero.tee

import com.calimero.mero.tee.TeeFixtures.AT
import com.calimero.mero.tee.TeeFixtures.COLLATERAL
import com.calimero.mero.tee.TeeFixtures.QUOTE
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The built-in DCAP verifier (a port of `@phala/dcap-qvl`'s `verify`) on dcap-qvl's own sample
 * TDX quote and collateral, the pair mero-js's tests and core's `calimero-tee-attestation`
 * tests verify.
 */
class DcapTest {
    private val now = AT / 1000

    private fun collateral(
        tcbInfo: String = COLLATERAL.tcbInfo,
        qeIdentity: String = COLLATERAL.qeIdentity,
        pckCrl: ByteArray = COLLATERAL.pckCrl,
    ) = DcapCollateral(
        COLLATERAL.pckCrlIssuerChain,
        COLLATERAL.rootCaCrl,
        pckCrl,
        COLLATERAL.tcbInfoIssuerChain,
        tcbInfo,
        COLLATERAL.tcbInfoSignature,
        COLLATERAL.qeIdentityIssuerChain,
        qeIdentity,
        COLLATERAL.qeIdentitySignature,
    )

    @Test
    fun `verifies the sample quote, up to date, as a TDX 1_0 report`() {
        val verified = Dcap.verify(QUOTE, COLLATERAL, now)
        assertEquals("UpToDate", verified.status)
        val td = (verified.report as DcapReport.Td10).report
        assertEquals(48, td.mrTd.size)
        assertEquals(64, td.reportData.size)
        assertEquals(0, td.tdAttributes[0].toInt() and 0x01)
    }

    @Test
    fun `refuses a TCB info or QE identity Intel did not sign`() {
        assertRefuses("Signature is invalid for tcb_info") {
            Dcap.verify(QUOTE, collateral(tcbInfo = COLLATERAL.tcbInfo.replaceFirst("\"version\":3", "\"version\":3 ")), now)
        }
        assertRefuses("Signature is invalid for qe_identity") {
            Dcap.verify(QUOTE, collateral(qeIdentity = COLLATERAL.qeIdentity.replaceFirst("\"version\":2", "\"version\":2 ")), now)
        }
    }

    @Test
    fun `refuses collateral that is not yet, or no longer, valid`() {
        assertRefuses("TCBInfo issue date is in the future") { Dcap.verify(QUOTE, COLLATERAL, utcMillis(2020, 1, 1, 0, 0, 0, 0) / 1000) }
        assertRefuses("TCBInfo expired") { Dcap.verify(QUOTE, COLLATERAL, utcMillis(2040, 1, 1, 0, 0, 0, 0) / 1000) }
    }

    @Test
    fun `refuses a CRL whose signature does not verify`() {
        val crl = COLLATERAL.pckCrl.copyOf()
        crl[crl.size - 5] = (crl[crl.size - 5].toInt() xor 0x01).toByte()
        assertRefuses("CRL") { Dcap.verify(QUOTE, collateral(pckCrl = crl), now) }
    }

    @Test
    fun `refuses a quote that does not chain to the root it is given`() {
        // Any other CA as the anchor: Intel's CRLs and chains no longer authenticate.
        val other = strictBase64(FULCIO_AUTHORITIES[0].certChain[0], "cert")
        val e = assertRefuses("") { Dcap.verify(QUOTE, COLLATERAL, now, other) }
        assertTrue(e is TeeTrustException)
    }

    @Test
    fun `refuses a truncated quote`() {
        assertRefuses("Failed to decode quote") { Dcap.verify(QUOTE.copyOfRange(0, 600), COLLATERAL, now) }
    }

    @Test
    fun `reads collateral as hex or as arrays of bytes, and writes it back as hex`() {
        val json = COLLATERAL.toJson()
        val asArrays =
            JsonObject(
                json +
                    ("root_ca_crl" to JsonArray(COLLATERAL.rootCaCrl.map { JsonPrimitive(it.toInt() and 0xff) })),
            )
        val read = DcapCollateral.fromJson(asArrays)
        assertArrayEquals(COLLATERAL.rootCaCrl, read.rootCaCrl)
        assertEquals("UpToDate", Dcap.verify(QUOTE, read, now).status)
        assertRefuses("The collateral has no tcb_info") { DcapCollateral.fromJson(JsonObject(json - "tcb_info")) }
    }
}
