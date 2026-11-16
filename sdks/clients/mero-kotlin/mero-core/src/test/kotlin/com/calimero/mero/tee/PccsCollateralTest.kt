package com.calimero.mero.tee

import com.calimero.mero.crypto.Hex
import com.calimero.mero.tee.TeeFixtures.AT
import com.calimero.mero.tee.TeeFixtures.COLLATERAL
import com.calimero.mero.tee.TeeFixtures.QUOTE
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.URLEncoder

/** [PccsCollateral] against a stand-in PCCS serving the sample quote's real collateral, as dcap-qvl's `getCollateral` reads it. */
class PccsCollateralTest {
    private fun enc(pem: String) = URLEncoder.encode(pem, "UTF-8").replace("+", "%20")

    private val pccs =
        FakeNet { req, _ ->
            val path = req.url.encodedPath + (req.url.encodedQuery?.let { "?$it" } ?: "")
            when {
                path.startsWith("/sgx/certification/v4/pckcrl?") ->
                    FakeNet.respondBytes(req, 200, COLLATERAL.pckCrl, "application/pkix-crl", mapOf("SGX-PCK-CRL-Issuer-Chain" to enc(COLLATERAL.pckCrlIssuerChain)))
                path.startsWith("/tdx/certification/v4/tcb?fmspc=") ->
                    FakeNet.respond(
                        req,
                        200,
                        buildJsonObject {
                            put("tcbInfo", Json.parseToJsonElement(COLLATERAL.tcbInfo))
                            put("signature", JsonPrimitive(Hex.encode(COLLATERAL.tcbInfoSignature)))
                        }.toString(),
                        headers = mapOf("TCB-Info-Issuer-Chain" to enc(COLLATERAL.tcbInfoIssuerChain)),
                    )
                path == "/tdx/certification/v4/qe/identity?update=standard" ->
                    FakeNet.respond(
                        req,
                        200,
                        buildJsonObject {
                            put("enclaveIdentity", Json.parseToJsonElement(COLLATERAL.qeIdentity))
                            put("signature", JsonPrimitive(Hex.encode(COLLATERAL.qeIdentitySignature)))
                        }.toString(),
                        headers = mapOf("SGX-Enclave-Identity-Issuer-Chain" to enc(COLLATERAL.qeIdentityIssuerChain)),
                    )
                path == "/sgx/certification/v4/rootcacrl" -> FakeNet.respond(req, 200, Hex.encode(COLLATERAL.rootCaCrl), "text/plain")
                else -> null
            }
        }

    @Test
    fun `fetches collateral the quote then verifies against`() {
        val fetched = runBlocking { PccsCollateral.fetch(QUOTE, "https://pccs.example/", pccs.client) }
        assertEquals(COLLATERAL.tcbInfo, fetched.tcbInfo)
        assertEquals(COLLATERAL.qeIdentity, fetched.qeIdentity)
        assertTrue(fetched.pckCertificateChain!!.contains("BEGIN CERTIFICATE"))
        assertEquals("UpToDate", Dcap.verify(QUOTE, fetched, AT / 1000).status)
        val fmspc = Json.parseToJsonElement(COLLATERAL.tcbInfo).let { (it as kotlinx.serialization.json.JsonObject)["fmspc"]!!.toString().trim('"') }
        assertTrue(pccs.requests.any { it == "https://pccs.example/tdx/certification/v4/tcb?fmspc=$fmspc" })
        assertTrue(pccs.requests.first().startsWith("https://pccs.example/sgx/certification/v4/pckcrl?ca="))
    }

    @Test
    fun `serves an agent quote verifier`() {
        val sample = TeeFixtures.SAMPLE_TD
        val verifier =
            createQuoteVerifier(
                QuoteVerifierOptions(allowedMeasurements = listOf(TeeFixtures.IMAGE), fetchCollateral = { q -> PccsCollateral.fetch(q, "https://pccs.example", pccs.client) }, now = { AT }),
            )
        val quote = TransportQuote(TeeFixtures.QUOTE_B64, Hex.encode(sample.reportData.copyOfRange(0, 32)), Hex.encode(sample.reportData.copyOfRange(32, 64)))
        assertTrue(runBlocking { verifier.verify(quote) })
    }

    @Test
    fun `a PCCS that does not answer fails the fetch`() {
        val down = FakeNet { req, _ -> FakeNet.respond(req, 503, "{}") }
        assertRefuses("Failed to fetch PCK CRL: 503") { runBlocking { PccsCollateral.fetch(QUOTE, "https://pccs.example", down.client) } }
    }
}
