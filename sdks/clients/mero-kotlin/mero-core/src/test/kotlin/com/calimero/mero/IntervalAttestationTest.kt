package com.calimero.mero

import com.calimero.mero.crypto.Ed25519
import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.base64
import com.calimero.mero.tee.TeeTrustException
import kotlinx.coroutines.runBlocking
import okhttp3.OkHttpClient
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** mero-js `runtime/attestation.test.ts`'s interval form: each refusal core's `verify_interval` makes. */
class IntervalAttestationTest {
    private val transportKey = ByteArray(32) { 5 }
    private val seed = ByteArray(32) { 7 }
    private val nonce = ByteArray(32) { 1 }
    private val quoteNonce = ByteArray(32) { 2 }
    private val nowMs = 1_800_000_000_000L
    private val policy = CloudPolicy(allowMock = true, now = { nowMs })
    private val http = OkHttpClient()

    private fun doc(
        signedNonce: ByteArray = nonce,
        boundKey: ByteArray = Ed25519.publicKey(seed),
        age: Long = 0,
        badSignature: Boolean = false,
    ): Pair<AttestationDoc, IntervalProof> {
        val key = Ed25519.publicKey(seed)
        val issuedAt = nowMs / 1000 - age
        val report = quoteNonce + intervalKeyBinding(transportKey, boundKey, issuedAt)
        val quote = MOCK_QUOTE_HEADER.encodeToByteArray() + report
        val signature = Ed25519.sign(seed, intervalMessage(signedNonce, ByteArray(32), transportKey))
        if (badSignature) signature[40] = (signature[40].toInt() xor 1).toByte()
        val interval =
            IntervalProof(
                key = Hex.encode(key),
                quoteNonce = Hex.encode(quoteNonce),
                issuedAt = issuedAt,
                signature = Hex.encode(signature),
            )
        val d =
            AttestationDoc(
                quote = base64(quote),
                transportKey = Hex.encode(transportKey),
                mock = true,
                interval = interval,
            )
        return d to interval
    }

    private fun check(d: Pair<AttestationDoc, IntervalProof>) =
        runBlocking { verifyInterval(d.first, d.second, nonce, ByteArray(32), transportKey, policy, http) }

    private fun refused(
        d: Pair<AttestationDoc, IntervalProof>,
        why: String,
    ) {
        try {
            check(d)
            fail("verified, expected a refusal: $why")
        } catch (e: TeeTrustException) {
            assertTrue("${e.message} names $why", e.message.orEmpty().contains(why))
        }
    }

    @Test
    fun aSignedNonceUnderTheBoundKeyVerifies() {
        check(doc())
    }

    @Test
    fun aSignatureOverAnotherNonceIsRefused() = refused(doc(signedNonce = ByteArray(32) { 9 }), "did not sign this nonce")

    @Test
    fun aStaleQuoteIsRefused() = refused(doc(age = INTERVAL_QUOTE_MAX_AGE_SECS + 30), "old, more than")

    @Test
    fun aQuoteDatedAheadIsRefused() = refused(doc(age = -600), "ahead of this clock")

    @Test
    fun aQuoteBindingAnotherKeyIsRefused() = refused(doc(boundKey = ByteArray(32) { 3 }), "does not bind")

    @Test
    fun aBadSignatureIsRefused() = refused(doc(badSignature = true), "did not sign this nonce")
}
