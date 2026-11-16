package com.calimero.mero.tee

import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.sha256
import com.calimero.mero.tee.TeeFixtures.AT
import com.calimero.mero.tee.TeeFixtures.COLLATERAL
import com.calimero.mero.tee.TeeFixtures.NONCE
import com.calimero.mero.tee.TeeFixtures.QUOTE_B64
import com.calimero.mero.tee.TeeFixtures.SUFFIX
import com.calimero.mero.tee.TeeFixtures.text
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.long
import kotlinx.serialization.json.put
import org.bouncycastle.util.encoders.Base64
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Port of mero-js `src/sealed/release.test.ts` (all but the sealed fetch, in `SealedTransportTest`). */
class NodeReleaseTest {
    private class Release(
        val publishedMrtds: String,
        val bundle: String,
        val signature: String,
    )

    private fun release(version: String) =
        Release(
            text("sealed/mero-tee-v$version.published-mrtds.json"),
            text("sealed/mero-tee-v$version.published-mrtds.json.bundle.json"),
            text("sealed/mero-tee-v$version.published-mrtds.json.sig"),
        )

    private val r86 = release("2.3.86")
    private val r87 = release("2.3.87")

    /** 2.3.87's published-mrtds.json with one byte changed. */
    private val tampered = text("sealed/mero-tee-v2.3.87.tampered.published-mrtds.json")
    private val provenanceBody = text("sealed/mero-tee-v2.3.87.release-provenance.json")
    private val provenanceBundle = text("sealed/mero-tee-v2.3.87.release-provenance.json.bundle.json")
    private val kmsBody = text("sealed/mero-kms-v2.3.87.kms-attestation-policy.json")
    private val kmsBundle = text("sealed/mero-kms-v2.3.87.kms-attestation-policy.json.bundle.json")
    private val kmsIdentity = "https://github.com/calimero-network/mero-tee/.github/workflows/release-kms.yaml@refs/heads/master"

    private fun signed(version: String) = release(version).let { SignedNodeRelease(version, it.publishedMrtds, it.bundle) }

    /** The 2.3.87 bundle with [edit] applied to its parsed JSON. */
    private fun edited(edit: (MutableMap<String, JsonElement>, MutableMap<String, JsonElement>) -> Unit): String {
        val bundle = Json.parseToJsonElement(r87.bundle).jsonObject.toMutableMap()
        val rekor = bundle.getValue("rekorBundle").jsonObject.toMutableMap()
        val payload = rekor.getValue("Payload").jsonObject.toMutableMap()
        edit(bundle, payload)
        rekor["Payload"] = JsonObject(payload)
        bundle["rekorBundle"] = JsonObject(rekor)
        return JsonObject(bundle).toString()
    }

    private fun certOf(bundle: String): String = (Json.parseToJsonElement(bundle).jsonObject["cert"] as JsonPrimitive).content

    private fun leafDer(bundle: String) = pemCertificate(String(Base64.decode(certOf(bundle))), "cert")

    private fun toPem(der: ByteArray) = "-----BEGIN CERTIFICATE-----\n${Base64.toBase64String(der)}\n-----END CERTIFICATE-----\n"

    private fun flipBase64(
        value: String,
        index: Int,
    ): String {
        val bytes = Base64.decode(value)
        bytes[index] = (bytes[index].toInt() xor 0x01).toByte()
        return Base64.toBase64String(bytes)
    }

    // ---- verifySignedNodeRelease on real releases -------------------------------------------

    @Test
    fun `verifies 2_3_86 and 2_3_87, from text or bytes, bundle as text or JSON`() {
        val v86 = verifySignedNodeRelease(r86.publishedMrtds, r86.bundle, signature = r86.signature)
        assertEquals("node", v86.role)
        assertEquals("2.3.86", v86.tag)
        val v87 =
            verifySignedNodeRelease(
                r87.publishedMrtds.encodeToByteArray(),
                Json.parseToJsonElement(r87.bundle),
                expectedVersion = "2.3.87",
                signature = r87.signature,
            )
        assertTrue(v87.profiles.keys.containsAll(listOf("debug", "locked-read-only")))
        assertTrue(Regex("^[0-9a-f]{96}$").matches(v87.profiles.getValue("locked-read-only").rtmr3))
    }

    @Test
    fun `refuses a file with one byte changed`() {
        assertRefuses("The file's SHA-256 is not the one Rekor logged") { verifySignedNodeRelease(tampered, r87.bundle) }
    }

    @Test
    fun `refuses one release's file with another's bundle`() {
        assertRefuses("The file's SHA-256 is not the one Rekor logged") { verifySignedNodeRelease(r87.publishedMrtds, r86.bundle) }
    }

    @Test
    fun `refuses a signer other than the one pinned`() {
        assertRefuses("The release was signed by \"${NODE_RELEASE_SIGNER.identity}\", not by $kmsIdentity") {
            verifySignedNodeRelease(r87.publishedMrtds, r87.bundle, identity = kmsIdentity)
        }
        // A genuine file genuinely signed by the KMS workflow is not a node release.
        assertRefuses("The release was signed by \"$kmsIdentity\", not by ${NODE_RELEASE_SIGNER.identity}") {
            verifySignedNodeRelease(kmsBody, kmsBundle)
        }
    }

    @Test
    fun `refuses an OIDC issuer other than the one pinned`() {
        assertRefuses("The signing certificate's OIDC issuer is \"${NODE_RELEASE_SIGNER.issuer}\", not https://accounts.google.com") {
            verifySignedNodeRelease(r87.publishedMrtds, r87.bundle, issuer = "https://accounts.google.com")
        }
    }

    @Test
    fun `refuses a signed entry timestamp that was altered`() {
        val bundle =
            Json.parseToJsonElement(r87.bundle).jsonObject.let { b ->
                val rekor = b.getValue("rekorBundle").jsonObject
                val set = flipBase64((rekor["SignedEntryTimestamp"] as JsonPrimitive).content, 20)
                JsonObject(b + ("rekorBundle" to JsonObject(rekor + ("SignedEntryTimestamp" to JsonPrimitive(set))))).toString()
            }
        assertRefuses("Rekor's signed entry timestamp does not verify") { verifySignedNodeRelease(r87.publishedMrtds, bundle) }
    }

    @Test
    fun `refuses a logged time, or index, that was altered`() {
        for (field in listOf("integratedTime", "logIndex")) {
            val bundle = edited { _, payload -> payload[field] = JsonPrimitive((payload.getValue(field) as JsonPrimitive).long - 1) }
            assertRefuses("Rekor's signed entry timestamp does not verify") { verifySignedNodeRelease(r87.publishedMrtds, bundle) }
        }
    }

    @Test
    fun `refuses an entry from a log the trust root does not know`() {
        val bundle = edited { _, payload -> payload["logID"] = JsonPrimitive("ab".repeat(32)) }
        assertRefuses("which the Sigstore trust root does not know") { verifySignedNodeRelease(r87.publishedMrtds, bundle) }
    }

    @Test
    fun `refuses another release's certificate, or a byte flipped in this one`() {
        val swapped = edited { b, _ -> b["cert"] = JsonPrimitive(certOf(r86.bundle)) }
        assertRefuses("The bundle's certificate is not the one Rekor logged") { verifySignedNodeRelease(r87.publishedMrtds, swapped) }
        val der = leafDer(r87.bundle)
        der[der.size - 10] = (der[der.size - 10].toInt() xor 0x01).toByte() // a byte of the issuer's signature
        val flipped = edited { b, _ -> b["cert"] = JsonPrimitive(Base64.toBase64String(toPem(der).encodeToByteArray())) }
        assertRefuses("The bundle's certificate is not the one Rekor logged") { verifySignedNodeRelease(r87.publishedMrtds, flipped) }
    }

    @Test
    fun `refuses a signature that is not the logged one, or the detached one's`() {
        val other = edited { b, _ -> b["base64Signature"] = Json.parseToJsonElement(r86.bundle).jsonObject.getValue("base64Signature") }
        assertRefuses("The bundle's signature is not the one Rekor logged") { verifySignedNodeRelease(r87.publishedMrtds, other) }
        assertRefuses("The detached signature is not the one in the bundle") {
            verifySignedNodeRelease(r87.publishedMrtds, r87.bundle, signature = r86.signature)
        }
    }

    @Test
    fun `refuses a file whose tag is not the version expected`() {
        assertRefuses("The signed release is 2.3.87, not 2.3.86") {
            verifySignedNodeRelease(r87.publishedMrtds, r87.bundle, expectedVersion = "2.3.86")
        }
    }

    @Test
    fun `refuses a signed file that is not a node release`() {
        assertRefuses("The signed release is a \"kms\" release, not a node one") {
            verifySignedNodeRelease(kmsBody, kmsBundle, identity = kmsIdentity)
        }
        // Signed by the node workflow, with a role and profiles, but no measurements.
        assertRefuses("not a release's published-mrtds.json") { verifySignedNodeRelease(provenanceBody, provenanceBundle) }
    }

    @Test
    fun `refuses what is not a bundle`() {
        assertRefuses("The bundle is not JSON") { verifySignedNodeRelease(r87.publishedMrtds, "not json") }
        assertRefuses("is not a cosign bundle") { verifySignedNodeRelease(r87.publishedMrtds, JsonObject(emptyMap())) }
        val bigIndex = edited { _, payload -> payload["logIndex"] = JsonPrimitive(1L shl 60) }
        assertRefuses("is not a cosign bundle") { verifySignedNodeRelease(r87.publishedMrtds, bigIndex) }
    }

    // ---- the certificate chain --------------------------------------------------------------

    private val leaf get() = parseCertificate(leafDer(r87.bundle))
    private val logged get() =
        (
            Json
                .parseToJsonElement(r87.bundle)
                .jsonObject
                .getValue("rekorBundle")
                .jsonObject
                .getValue("Payload")
                .jsonObject["integratedTime"] as JsonPrimitive
        ).long * 1000

    @Test
    fun `chains the real leaf to Fulcio when Rekor logged it, and not ten minutes later`() {
        Sigstore.chainsToFulcio(leaf, logged)
        assertRefuses("not at 2026-09-28T10:29:25.000Z when Rekor logged it") { Sigstore.chainsToFulcio(leaf, leaf.notAfter + 1000) }
    }

    @Test
    fun `refuses a leaf whose issuer signature does not verify`() {
        val der = leafDer(r87.bundle)
        der[der.size - 10] = (der[der.size - 10].toInt() xor 0x01).toByte()
        assertRefuses("was not issued by a Fulcio CA in the Sigstore trust root") { Sigstore.chainsToFulcio(parseCertificate(der), logged) }
    }

    @Test
    fun `names the CA when its trust-root window does not cover the time`() {
        // The intermediate that issued the leaf, used before the trust root says it was.
        assertRefuses("the trust root says it was not issuing certificates then") {
            Sigstore.chainsToFulcio(leaf.copy(notBefore = 0), utcMillis(2022, 1, 1, 0, 0, 0, 0))
        }
    }

    @Test
    fun `embeds a trust root that is consistent - log ids are their keys, chains verify`() {
        for (log in REKOR_LOGS) assertEquals(log.logId, Hex.encode(sha256(strictBase64(log.publicKey, "key"))))
        for (authority in FULCIO_AUTHORITIES) {
            val chain = authority.certChain.map { parseCertificate(strictBase64(it, "cert")) }
            chain.forEachIndexed { index, certificate ->
                val parent = chain.getOrNull(index + 1) ?: certificate
                assertEquals(Curve.P384, certificate.curve)
                assertEquals(Hash.SHA384, certificate.signatureHash)
                assertTrue(verifyEcdsa(parent.spki, parent.curve, certificate.signatureHash, certificate.signature, certificate.tbs))
            }
        }
    }

    // ---- trustSignedRelease -----------------------------------------------------------------

    @Test
    fun `trusts one profile of a signed release at or above the minimum`() {
        val trusted = trustSignedRelease(signed("2.3.87"), "locked-read-only", minReleaseVersion = "2.3.86")
        val image =
            Json
                .parseToJsonElement(r87.publishedMrtds)
                .jsonObject
                .getValue("profiles")
                .jsonObject
                .getValue("locked-read-only")
                .jsonObject
        val reg = { k: String -> (image[k] as JsonPrimitive).content }
        assertEquals(listOf(TrustedMeasurements(reg("mrtd"), reg("rtmr0"), reg("rtmr1"), reg("rtmr2"), reg("rtmr3"))), trusted.allowedMeasurements)
        assertTrue("UpToDate" in trusted.allowedTcbStatuses)
        assertRefuses("not a production profile") { trustSignedRelease(signed("2.3.87"), "debug", minReleaseVersion = "2.3.87") }
        assertNotNull(trustSignedRelease(signed("2.3.87"), "debug", minReleaseVersion = "2.3.87", allowDebugProfile = true))
    }

    @Test
    fun `refuses a release older than the minimum, compared as versions and not strings`() {
        assertRefuses("Release 2.3.86 is older than the minimum trusted, 2.3.87") {
            trustSignedRelease(signed("2.3.86"), "locked-read-only", minReleaseVersion = "2.3.87")
        }
        assertRefuses("older than the minimum") { trustSignedRelease(signed("2.3.87"), "locked-read-only", minReleaseVersion = "2.3.100") }
        trustSignedRelease(signed("2.3.87"), "locked-read-only", minReleaseVersion = "2.3.87-rc.1")
        trustSignedRelease(signed("2.3.87"), "locked-read-only", minReleaseVersion = "2.3.9")
    }

    @Test
    fun `requires a minimum that is a version`() {
        for (junk in listOf("", "latest", "2.3", "2.3.x", "v2.3.87", "2.3.87-", "../2.3.87")) {
            assertRefuses("minReleaseVersion is") { trustSignedRelease(signed("2.3.87"), "locked-read-only", minReleaseVersion = junk) }
        }
    }

    @Test
    fun `orders versions as semver does`() {
        assertTrue(compareReleaseVersions("2.3.87-rc.1", "2.3.87") < 0)
        assertTrue(compareReleaseVersions("2.3.87-rc.2", "2.3.87-rc.10") < 0)
        assertTrue(compareReleaseVersions("2.3.87-1", "2.3.87-alpha") < 0)
        assertTrue(compareReleaseVersions("2.3.87-alpha", "2.3.87-alpha.1") < 0)
        assertEquals(0, compareReleaseVersions("2.3.87+build.1", "2.3.87"))
    }

    @Test
    fun `refuses a release that claims a version other than its file's`() {
        assertRefuses("The signed release is 2.3.87, not 2.3.88") {
            trustSignedRelease(signed("2.3.87").copy(version = "2.3.88"), "locked-read-only", minReleaseVersion = "2.3.86")
        }
        assertRefuses("The release version is \"latest\"") {
            trustSignedRelease(signed("2.3.87").copy(version = "latest"), "locked-read-only", minReleaseVersion = "2.3.86")
        }
    }

    @Test
    fun `refuses a release whose file is not the one signed`() {
        assertRefuses("The file's SHA-256 is not the one Rekor logged") {
            trustSignedRelease(signed("2.3.87").copy(publishedMrtds = tampered), "locked-read-only", minReleaseVersion = "2.3.86")
        }
    }

    @Test
    fun `refuses a profile the release does not have`() {
        assertRefuses("has no profile \"prod\"") { trustSignedRelease(signed("2.3.87"), "prod", minReleaseVersion = "2.3.86") }
    }

    // ---- fetching a release -----------------------------------------------------------------

    private fun releaseBody(r: SignedNodeRelease) =
        buildJsonObject {
            put(
                "data",
                buildJsonObject {
                    put("version", r.version)
                    put("publishedMrtds", r.publishedMrtds)
                    put("bundle", r.bundle)
                },
            )
        }.toString()

    @Test
    fun `unwraps data from a mirror, which is only fetched`() {
        val net = FakeNet { req, _ -> FakeNet.respond(req, 200, releaseBody(signed("2.3.87"))) }
        val url = cloudNodeReleaseUrl("https://cloud.example/", "2.3.87")
        assertEquals("https://cloud.example/api/tee/node-releases/2.3.87", url)
        assertEquals(signed("2.3.87"), runBlocking { fetchNodeRelease(url, net.client) })
        assertEquals(listOf(url), net.requests)
        assertRefuses("not a release version") { cloudNodeReleaseUrl("https://cloud.example", "../admin") }
        assertEquals("https://cloud.calimero.network", DEFAULT_RELEASE_MIRROR)
    }

    @Test
    fun `refuses what is not a release`() {
        val url = "https://cloud.example/api/tee/node-releases/2.3.87"
        val empty = FakeNet { req, _ -> FakeNet.respond(req, 200, """{"data":{}}""") }
        assertRefuses("did not answer with a signed node release") { runBlocking { fetchNodeRelease(url, empty.client) } }
        val missing = FakeNet { req, _ -> FakeNet.respond(req, 404, "{}") }
        assertRefuses("HTTP 404") { runBlocking { fetchNodeRelease(url, missing.client) } }
    }

    // ---- the release a node names -----------------------------------------------------------

    private val args = TransportQuote(QUOTE_B64, NONCE, SUFFIX, COLLATERAL)

    private fun verifier(
        minReleaseVersion: String = "2.3.86",
        release: suspend () -> SignedNodeRelease,
    ) = createSignedReleaseVerifier(SignedReleaseVerifierOptions(release, minReleaseVersion, "locked-read-only", now = { AT }))

    @Test
    fun `verifies the quote against the signed release, and refuses an image not of it`() {
        var asked = 0
        val verify =
            verifier {
                asked++
                signed("2.3.87")
            }
        assertTrue(verify.includeCollateral)
        assertRefuses("are not an image this verifier trusts") { runBlocking { verify.verify(args) } }
        assertEquals(1, asked)
    }

    @Test
    fun `refuses before the quote when the release does not pass`() {
        assertRefuses("older than the minimum") { runBlocking { verifier("2.3.87") { signed("2.3.86") }.verify(args) } }
        assertRefuses("SHA-256") { runBlocking { verifier { signed("2.3.87").copy(publishedMrtds = tampered) }.verify(args) } }
        assertRefuses("minReleaseVersion") { verifier("latest") { signed("2.3.87") } }
    }
}
