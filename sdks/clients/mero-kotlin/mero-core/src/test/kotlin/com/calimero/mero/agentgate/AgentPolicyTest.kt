package com.calimero.mero.agentgate

import com.calimero.mero.crypto.Hex
import com.calimero.mero.tee.TeeFixtures
import com.calimero.mero.tee.TeeFixtures.AT
import com.calimero.mero.tee.TeeFixtures.COLLATERAL
import com.calimero.mero.tee.TeeFixtures.QUOTE_B64
import com.calimero.mero.tee.TeeFixtures.SAMPLE
import com.calimero.mero.tee.TeeFixtures.SAMPLE_TD
import com.calimero.mero.tee.TransportQuote
import com.calimero.mero.tee.assertRefuses
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertTrue
import org.junit.Test

/** Port of mero-js `src/agent-gate/policy.test.ts`, on the same real quote and signed KMS files. */
class AgentPolicyTest {
    /** The sample quote's image as an agent allowlist, written the way a release may write it. */
    private fun policy() =
        AgentPolicy(
            schemaVersion = 1,
            role = "agent",
            profile = "locked-read-only",
            allowedTcbStatuses = listOf(SAMPLE.status.lowercase()),
            allowedMrtd = listOf("0x${Hex.encode(SAMPLE_TD.mrTd).uppercase()}"),
            allowedRtmr0 = listOf(Hex.encode(SAMPLE_TD.rtMr0)),
            allowedRtmr1 = listOf(Hex.encode(SAMPLE_TD.rtMr1)),
            allowedRtmr2 = listOf(Hex.encode(SAMPLE_TD.rtMr2)),
            allowedRtmr3 = listOf(Hex.encode(SAMPLE_TD.rtMr3)),
        )

    private val args =
        TransportQuote(QUOTE_B64, Hex.encode(SAMPLE_TD.reportData.copyOfRange(0, 32)), Hex.encode(SAMPLE_TD.reportData.copyOfRange(32, 64)))

    private fun verifier(p: AgentPolicy) = agentQuoteVerifier(p, fetchCollateral = { COLLATERAL }, now = { AT })

    @Test
    fun `accepts a genuine quote of an image the policy lists, fetching its collateral`() {
        assertTrue(runBlocking { verifier(policy()).verify(args) })
    }

    @Test
    fun `refuses a register outside the policy`() {
        assertRefuses("RTMR3") { runBlocking { verifier(policy().copy(allowedRtmr3 = listOf("a".repeat(96)))).verify(args) } }
    }

    @Test
    fun `refuses a TCB status the policy does not allow`() {
        val other = if (SAMPLE.status == "OutOfDate") "UpToDate" else "OutOfDate"
        assertRefuses("TCB status") { runBlocking { verifier(policy().copy(allowedTcbStatuses = listOf(other))).verify(args) } }
    }

    @Test
    fun `accepts nothing from an empty list, as the gate's own verifier`() {
        assertRefuses("allowed_rtmr0 is empty") { verifier(policy().copy(allowedRtmr0 = emptyList())) }
    }

    @Test
    fun `refuses a status no DCAP verifier reports`() {
        assertRefuses("unknown TCB status") { verifier(policy().copy(allowedTcbStatuses = listOf("Fine"))) }
    }

    private val kmsBody = TeeFixtures.text("sealed/mero-kms-v2.3.87.kms-attestation-policy.json")
    private val kmsBundle = TeeFixtures.text("sealed/mero-kms-v2.3.87.kms-attestation-policy.json.bundle.json")

    @Test
    fun `is a file the KMS release workflow signed, and an agent allowlist`() {
        // Genuinely signed by the KMS workflow, but the KMS's own policy, not an agent one.
        assertRefuses("not an agent one") { verifySignedAgentPolicy(kmsBody, kmsBundle) }
    }

    @Test
    fun `refuses a file signed by any other workflow`() {
        val node = TeeFixtures.text("sealed/mero-tee-v2.3.87.published-mrtds.json")
        val nodeBundle = TeeFixtures.text("sealed/mero-tee-v2.3.87.published-mrtds.json.bundle.json")
        assertRefuses("release-kms.yaml") { verifySignedAgentPolicy(node, nodeBundle) }
    }

    @Test
    fun `refuses a file whose bytes are not the signed ones`() {
        assertRefuses("SHA-256") { verifySignedAgentPolicy("$kmsBody ", kmsBundle) }
    }
}
