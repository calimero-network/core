package com.calimero.mero.tee

import com.calimero.mero.crypto.Hex
import com.calimero.mero.tee.TeeFixtures.AT
import com.calimero.mero.tee.TeeFixtures.COLLATERAL
import com.calimero.mero.tee.TeeFixtures.IMAGE
import com.calimero.mero.tee.TeeFixtures.NONCE
import com.calimero.mero.tee.TeeFixtures.QUOTE
import com.calimero.mero.tee.TeeFixtures.QUOTE_B64
import com.calimero.mero.tee.TeeFixtures.SAMPLE_TD
import com.calimero.mero.tee.TeeFixtures.SUFFIX
import kotlinx.coroutines.runBlocking
import org.bouncycastle.util.encoders.Base64
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Port of mero-js `src/sealed/verify.test.ts`, on the same real quote and collateral. */
class QuoteVerifierTest {
    private val args = TransportQuote(QUOTE_B64, NONCE, SUFFIX, COLLATERAL)

    @Suppress("LongParameterList")
    private fun real(
        allowedMeasurements: List<TrustedMeasurements>? = listOf(IMAGE),
        allowedMrtd: List<String>? = null,
        allowedRtmr0: List<String>? = null,
        allowedRtmr1: List<String>? = null,
        allowedRtmr2: List<String>? = null,
        allowedRtmr3: List<String>? = null,
        fetchCollateral: (suspend (ByteArray) -> DcapCollateral)? = null,
        now: () -> Long = { AT },
    ) = createQuoteVerifier(
        QuoteVerifierOptions(
            allowedMeasurements = allowedMeasurements,
            allowedMrtd = allowedMrtd,
            allowedRtmr0 = allowedRtmr0,
            allowedRtmr1 = allowedRtmr1,
            allowedRtmr2 = allowedRtmr2,
            allowedRtmr3 = allowedRtmr3,
            fetchCollateral = fetchCollateral,
            now = now,
        ),
    )

    private fun QuoteVerifier.check(quote: TransportQuote = args): Boolean = runBlocking { verify(quote) }

    @Test
    fun `accepts a genuine quote from a trusted image that commits to the binding`() {
        val verifier = real()
        assertTrue(verifier.check())
        assertTrue(verifier.includeCollateral)
    }

    @Test
    fun `the sample verifies as up to date`() {
        assertEquals("UpToDate", TeeFixtures.SAMPLE.status)
        assertEquals("td10", TeeFixtures.SAMPLE.report.type)
    }

    @Test
    fun `refuses a quote that commits to another binding`() {
        assertRefuses("does not commit to this nonce and binding") { real().check(args.copy(reportDataSuffix = "00".repeat(32))) }
    }

    @Test
    fun `refuses an image it does not trust`() {
        assertRefuses("are not an image this verifier trusts") { real(listOf(IMAGE.copy(rtmr3 = "ab".repeat(48)))).check() }
    }

    @Test
    fun `never combines the registers of different images`() {
        val other = "ab".repeat(48)
        val split = real(listOf(IMAGE.copy(rtmr1 = other), IMAGE.copy(rtmr2 = other), IMAGE.copy(rtmr3 = other)))
        assertRefuses("are not an image this verifier trusts") { split.check() }
        assertTrue(real(listOf(IMAGE.copy(rtmr1 = other), IMAGE)).check())
    }

    @Test
    fun `accepts MRTD allowlists only together with the image registers`() {
        val lists = { mrtd: String ->
            real(null, listOf(mrtd), allowedRtmr1 = listOf(IMAGE.rtmr1), allowedRtmr2 = listOf(IMAGE.rtmr2), allowedRtmr3 = listOf(IMAGE.rtmr3))
        }
        assertTrue(lists(IMAGE.mrtd).check())
        assertRefuses("MRTD") { lists("ab".repeat(48)).check() }
    }

    @Test
    fun `checks an RTMR only when asked to`() {
        assertTrue(real(allowedRtmr0 = listOf(Hex.encode(SAMPLE_TD.rtMr0))).check())
        assertRefuses("RTMR0") { real(allowedRtmr0 = listOf("cd".repeat(48))).check() }
    }

    @Test
    fun `refuses a tampered quote`() {
        val tampered = QUOTE.copyOf()
        tampered[200] = (tampered[200].toInt() xor 0x01).toByte() // a byte of the signed TD report body
        assertRefuses("did not verify") { real().check(args.copy(quoteB64 = Base64.toBase64String(tampered))) }
    }

    @Test
    fun `refuses collateral outside its validity window`() {
        assertRefuses("did not verify") { real(now = { utcMillis(2040, 1, 1, 0, 0, 0, 0) }).check() }
    }

    @Test
    fun `gets collateral itself when the node sends none, and refuses without a way to`() {
        val asked = mutableListOf<ByteArray>()
        val noCollateral = args.copy(collateral = null)
        assertTrue(
            real(fetchCollateral = { q ->
                asked += q
                COLLATERAL
            }).check(noCollateral),
        )
        assertArrayEquals(QUOTE, asked.single())
        assertRefuses("no collateral") { real().check(noCollateral) }
    }

    // ---- policy, with a stand-in DCAP verifier ----------------------------------------------

    private fun report(
        status: String,
        td: TdReport10 = SAMPLE_TD,
    ) = DcapVerifiedReport(status, report = DcapReport.Td10(td))

    private fun stub(
        report: DcapVerifiedReport,
        statuses: List<String>? = null,
        now: () -> Long = System::currentTimeMillis,
    ) = createQuoteVerifier(
        QuoteVerifierOptions(dcapVerify = { _, _, _ -> report }, allowedMeasurements = listOf(IMAGE), allowedTcbStatuses = statuses, now = now),
    )

    @Test
    fun `accepts only up-to-date platforms by default`() {
        assertRefuses("TCB status is OutOfDate") { stub(report("OutOfDate")).check() }
        assertTrue(stub(report("OutOfDate"), listOf("UpToDate", "OutOfDate")).check())
    }

    @Test
    fun `refuses a revoked platform even when told to accept it`() {
        assertRefuses("Revoked") { stub(report("Revoked"), listOf("Revoked")).check() }
    }

    // Whatever dcapVerify was given: a debug TD's memory and registers are open to the host.
    @Test
    fun `refuses a debug TD, or one whose attributes it cannot read, whatever dcapVerify allows`() {
        val debug = ByteArray(8).also { it[0] = 0x01 }
        assertRefuses("debug TD") { stub(report("UpToDate", SAMPLE_TD.withTdAttributes(debug))).check() }
        val td15 = DcapVerifiedReport("UpToDate", report = DcapReport.Td15(TdReport15(SAMPLE_TD.withTdAttributes(debug), ByteArray(16), ByteArray(48))))
        assertRefuses("debug TD") { stub(td15).check() }
        assertRefuses("debug TD") { stub(report("UpToDate", SAMPLE_TD.withTdAttributes(ByteArray(0)))).check() }
        assertRefuses("debug TD") { stub(report("UpToDate", SAMPLE_TD.withTdAttributes(ByteArray(4)))).check() }
        assertTrue(stub(report("UpToDate", SAMPLE_TD.withTdAttributes(ByteArray(8)))).check())
    }

    @Test
    fun `refuses a quote that is not TDX`() {
        val sgx = SgxEnclaveReport(ByteArray(16), 0, ByteArray(16), ByteArray(32), ByteArray(32), 0, 0, ByteArray(64))
        assertRefuses("not a TDX one") { stub(DcapVerifiedReport("UpToDate", report = DcapReport.Sgx(sgx))).check() }
    }

    @Test
    fun `passes the time now, in seconds`() {
        var seen: Triple<ByteArray, DcapCollateral, Long>? = null
        val verifier =
            createQuoteVerifier(
                QuoteVerifierOptions(
                    dcapVerify = { q, c, t ->
                        seen = Triple(q, c, t)
                        report("UpToDate")
                    },
                    allowedMeasurements = listOf(IMAGE),
                    now = { 1_234_567_890 },
                ),
            )
        assertTrue(verifier.check())
        assertArrayEquals(QUOTE, seen!!.first)
        assertTrue(seen!!.second === COLLATERAL)
        assertEquals(1_234_567L, seen!!.third)
    }

    @Test
    fun `will not be built to accept nothing, or with a measurement that is not one`() {
        assertRefuses("No image is trusted") { createQuoteVerifier(QuoteVerifierOptions()) }
        assertRefuses("allowedMeasurements is empty") { createQuoteVerifier(QuoteVerifierOptions(allowedMeasurements = emptyList())) }
        assertRefuses("allowedMeasurements[0].rtmr2") {
            createQuoteVerifier(QuoteVerifierOptions(allowedMeasurements = listOf(IMAGE.copy(rtmr2 = "aabb"))))
        }
        assertRefuses("allowedRtmr2") {
            createQuoteVerifier(QuoteVerifierOptions(allowedMeasurements = listOf(IMAGE), allowedRtmr2 = listOf("zz")))
        }
    }

    @Test
    fun `will not trust an MRTD alone, which names the firmware and not the image`() {
        assertRefuses("does not name an image") { createQuoteVerifier(QuoteVerifierOptions(allowedMrtd = listOf(IMAGE.mrtd))) }
        assertRefuses("does not name an image") {
            createQuoteVerifier(
                QuoteVerifierOptions(allowedMrtd = listOf(IMAGE.mrtd), allowedRtmr1 = listOf(IMAGE.rtmr1), allowedRtmr2 = listOf(IMAGE.rtmr2)),
            )
        }
    }

    // ---- trustedMeasurementsFromReleases ----------------------------------------------------

    private fun release(
        tag: String,
        rtmr3: String,
        statuses: List<String> = listOf("uptodate", "outofdate"),
        role: String? = "node",
    ) = PublishedMrtds(
        role = role,
        tag = tag,
        profiles =
            mapOf(
                "locked-read-only" to PublishedProfile(IMAGE.mrtd, IMAGE.rtmr0, IMAGE.rtmr1, IMAGE.rtmr2, rtmr3, statuses),
                "debug" to PublishedProfile(IMAGE.mrtd, IMAGE.rtmr0, IMAGE.rtmr1, IMAGE.rtmr2, "dd".repeat(48), statuses),
            ),
    )

    @Test
    fun `trusts each release's image of one profile, by all of its measurements`() {
        val trusted = trustedMeasurementsFromReleases(listOf(release("2.3.76", "aa".repeat(48)), release("2.3.78", IMAGE.rtmr3)), "locked-read-only")
        assertEquals(listOf(IMAGE.copy(rtmr3 = "aa".repeat(48)), IMAGE), trusted.allowedMeasurements)
        assertEquals(listOf("UpToDate", "OutOfDate"), trusted.allowedTcbStatuses)
    }

    @Test
    fun `builds a verifier that accepts the release image and not another profile of it`() {
        assertTrue(trustedMeasurementsFromReleases(listOf(release("2.3.78", IMAGE.rtmr3)), "locked-read-only").verifier(now = { AT }).check())
        val debug = trustedMeasurementsFromReleases(listOf(release("2.3.78", IMAGE.rtmr3)), "debug", allowDebugProfile = true)
        assertRefuses("are not an image this verifier trusts") { debug.verifier(now = { AT }).check() }
    }

    @Test
    fun `trusts only the production profile unless debug profiles are asked for`() {
        val base = release("2.3.78", IMAGE.rtmr3)
        val withDebugReadOnly =
            base.copy(
                profiles =
                    base.profiles +
                        ("debug-read-only" to PublishedProfile(IMAGE.mrtd, IMAGE.rtmr0, IMAGE.rtmr1, IMAGE.rtmr2, "ee".repeat(48), listOf("uptodate"))),
            )
        for (profile in listOf("debug", "debug-read-only")) {
            assertRefuses("\"$profile\" is not a production profile") { trustedMeasurementsFromReleases(listOf(withDebugReadOnly), profile) }
            assertEquals(1, trustedMeasurementsFromReleases(listOf(withDebugReadOnly), profile, allowDebugProfile = true).allowedMeasurements.size)
        }
        assertEquals(listOf(IMAGE), trustedMeasurementsFromReleases(listOf(withDebugReadOnly), "locked-read-only").allowedMeasurements)
    }

    @Test
    fun `accepts only the TCB statuses every release accepts`() {
        val trusted =
            trustedMeasurementsFromReleases(listOf(release("2.3.76", IMAGE.rtmr3, listOf("uptodate")), release("2.3.78", IMAGE.rtmr3)), "locked-read-only")
        assertEquals(listOf("UpToDate"), trusted.allowedTcbStatuses)
    }

    @Test
    fun `refuses what it cannot read as a node release`() {
        assertRefuses("No release") { trustedMeasurementsFromReleases(emptyList(), "locked-read-only") }
        assertRefuses("has no profile \"prod\"") { trustedMeasurementsFromReleases(listOf(release("2.3.78", IMAGE.rtmr3)), "prod") }
        assertRefuses("not of a node image") { trustedMeasurementsFromReleases(listOf(release("2.3.78", IMAGE.rtmr3, role = "kms")), "locked-read-only") }
        assertRefuses("does not know") {
            trustedMeasurementsFromReleases(listOf(release("2.3.78", IMAGE.rtmr3, listOf("sometimes"))), "locked-read-only")
        }
        assertRefuses("no TCB status in common") {
            trustedMeasurementsFromReleases(
                listOf(release("2.3.76", IMAGE.rtmr3, listOf("uptodate")), release("2.3.78", IMAGE.rtmr3, listOf("outofdate"))),
                "locked-read-only",
            )
        }
    }
}
