package com.calimero.mero.tee

import com.calimero.mero.AppHost
import com.calimero.mero.AssertionJSON
import com.calimero.mero.CalimeroClient
import com.calimero.mero.CalimeroClientOptions
import com.calimero.mero.CloudPolicy
import com.calimero.mero.PasskeyCancelled
import com.calimero.mero.ServedPageHandoff
import com.calimero.mero.ServedPagePasskey
import com.calimero.mero.ServedPageSession
import com.calimero.mero.ServedPageSignInError
import com.calimero.mero.WebAuthenticating
import com.calimero.mero.WebAuthenticationCancelled
import com.calimero.mero.b64url
import com.calimero.mero.crypto.concat
import com.calimero.mero.fromB64url
import com.calimero.mero.signInWithServedPage
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import org.bouncycastle.asn1.sec.SECNamedCurves
import org.bouncycastle.crypto.digests.SHA256Digest
import org.bouncycastle.crypto.generators.HKDFBytesGenerator
import org.bouncycastle.crypto.params.HKDFParameters
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.math.BigInteger
import java.net.URI
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/** [AppHost] and [ServedPageHandoff] against vectors from mdma and `@calimero-apps/passkey/native` (Swift `ServedPageHandoffTests`). */
class ServedPageTest {
    // ---------------------------------------------------------------- AppHost

    // Vectors from mdma `mdma_common.app_host` (core `app_host::label`).
    @Test
    fun `labels match core and the manager`() {
        assertEquals("pjfzih7fgm2e33figvvm2hcvlbr3egcxy3jjn6zhuzdlxkz4hfdq", AppHost.label("com.calimero.chat"))
        assertEquals("5eolix2xlsehjkbd2l5262oevb65zvq4dkgo66zlxpfc4os34una", AppHost.clusterLabel("eu-shared-1", "com.calimero.chat"))
    }

    @Test
    fun `origin is the package's host or the cluster's own`() {
        assertEquals("https://pjfzih7fgm2e33figvvm2hcvlbr3egcxy3jjn6zhuzdlxkz4hfdq.apps.calimero.network", AppHost.origin("com.calimero.chat"))
        assertEquals(
            "https://5eolix2xlsehjkbd2l5262oevb65zvq4dkgo66zlxpfc4os34una.apps.calimero.network",
            AppHost.origin("com.calimero.chat", cluster = "eu-shared-1"),
        )
    }

    // ---------------------------------------------------------------- sign-in hand-off

    private fun jsHandoff() = ServedPageHandoff(fromB64url(JS_PRIVATE_KEY), JS_STATE)

    private fun answer(
        state: String = JS_STATE,
        box: String = JS_BOX,
    ) = "mero-chat://signed-in#state=$state&box=$box"

    private fun refusal(block: () -> Unit): String {
        try {
            block()
        } catch (e: ServedPageSignInError) {
            return e.message.orEmpty()
        }
        fail("expected a ServedPageSignInError")
        error("unreachable")
    }

    @Test
    fun `opens what the web page sealed`() {
        val handoff = jsHandoff()
        assertEquals(JS_PUBLIC_KEY, handoff.publicKey)
        val got = handoff.open(answer())
        assertEquals(ServedPageSession(1, "acc_1", "v2.claims.sig", 1_900_000_000.0, "rt_1", "https://n1.example", listOf("https://n2.example")), got)
        assertEquals("rt_1", got.session.refreshToken)
        assertEquals(1_900_000_000L, got.session.expiresAt)
    }

    @Test
    fun `refuses a replayed answer`() {
        val handoff = jsHandoff()
        handoff.open(answer())
        assertEquals("this request was already answered", refusal { handoff.open(answer()) })
    }

    @Test
    fun `refuses another state`() {
        assertEquals("the answer is not for this request", refusal { jsHandoff().open(answer(state = "c29tZS1vdGhlci1zaWduLWluLXN0YXRl")) })
    }

    @Test
    fun `another key cannot open the box`() {
        assertEquals("the answer could not be opened", refusal { ServedPageHandoff(state = JS_STATE).open(answer()) })
    }

    @Test
    fun `refuses an answer without a session`() {
        refusal { jsHandoff().open("mero-chat://signed-in") }
        refusal { jsHandoff().open(answer(box = "AAAA")) }
    }

    @Test
    fun `the page URL carries state and key in the fragment`() {
        val handoff = ServedPageHandoff()
        val url = URI(handoff.url("https://abc.apps.calimero.network", "1.2.0"))
        assertEquals("/1.2.0/", url.path)
        assertNull(url.query)
        assertEquals("native-sign-in&state=${handoff.state}&key=${handoff.publicKey}", url.rawFragment)
        assertEquals(65, fromB64url(handoff.publicKey).size)
        assertNotEquals(handoff.state, ServedPageHandoff().state)
    }

    // ---------------------------------------------------------------- approvals

    @Test
    fun `opens the assertion the web page sealed`() {
        assertEquals(AssertionJSON("cred_1", "ad", "cd", "sig"), jsHandoff().openApproval(JS_APPROVAL))
    }

    @Test
    fun `a sign-in cannot be read as an approval or the other way`() {
        refusal { jsHandoff().openApproval(answer()) }
        refusal { jsHandoff().open(JS_APPROVAL) }
    }

    @Test
    fun `refuses a replayed approval`() {
        val handoff = jsHandoff()
        handoff.openApproval(JS_APPROVAL)
        refusal { handoff.openApproval(JS_APPROVAL) }
    }

    @Test
    fun `a decline is a cancelled passkey`() {
        try {
            jsHandoff().openApproval("mero-chat://approved#state=$JS_STATE&cancelled=1")
            fail("a decline is not an assertion")
        } catch (_: PasskeyCancelled) {
        }
    }

    @Test
    fun `the approval URL carries the challenge in the fragment`() {
        val h = ServedPageHandoff()
        val url = h.url("https://x.apps.calimero.network", "3.1.72", ServedPageHandoff.APPROVE_FRAGMENT, listOf("challenge" to "AAAA", "rpId" to "calimero.network"))
        assertTrue(url.startsWith("https://x.apps.calimero.network/3.1.72/#"))
        assertEquals("native-approve&state=${h.state}&key=${h.publicKey}&challenge=AAAA&rpId=calimero.network", URI(url).rawFragment)
    }

    @Test
    fun `the consent page returns to the served page without a fragment`() {
        val to = ServedPageHandoff.consentReturnTo("https://x.apps.calimero.network/", "3.1.72")
        assertEquals("https://x.apps.calimero.network/3.1.72/?native-consent=1", to)
        assertNull(URI(to).fragment)
    }

    // ---------------------------------------------------------------- sign-in and approval through a page

    /** The apps host: serves releases 1.0.0 and 1.2.0 of every package. */
    private val appsHost =
        FakeNet { request, _ ->
            if (request.url.encodedPath == "/.well-known/calimero.json") {
                FakeNet.respond(request, 200, """{"package":"com.calimero.chat","versions":["1.0.0","1.2.0"],"deployments":[],"api":[]}""")
            } else {
                null
            }
        }

    private fun client() = CalimeroClient(CalimeroClientOptions("https://n1.test", policy = CloudPolicy(), httpClient = appsHost.client))

    /** The served page, answered in-process: seals [payload] to the key in the URL as `@calimero-apps/passkey/native` does. */
    private class SealingPage(
        val payload: String,
        val info: String = ServedPageHandoff.INFO,
    ) : WebAuthenticating {
        val opened = mutableListOf<String>()

        override suspend fun authenticate(
            url: String,
            callback: String,
        ): String {
            opened += url
            val params =
                url
                    .substringAfter('#')
                    .split('&')
                    .filter { '=' in it }
                    .associate { it.substringBefore('=') to it.substringAfter('=') }
            val state = params.getValue("state")
            return "$callback#state=$state&box=${seal(params.getValue("key"), state, info, payload)}"
        }
    }

    @Test
    fun `signs in through the package's page and uses the session`() =
        runBlocking {
            val client = client()
            val page = SealingPage(Json.encodeToString(ServedPageSession.serializer(), SIGNED_IN))
            val got = client.signInWithServedPage(PACKAGE, "mero-chat://signed-in", page)
            val opened = URI(page.opened.single())
            assertEquals("${AppHost.label(PACKAGE)}.apps.calimero.network", opened.host)
            assertEquals("/1.2.0/", opened.path)
            assertEquals(SIGNED_IN, got)
            assertEquals("v2.claims.sig", client.session?.sessionToken)
            assertEquals("rt_1", client.session?.refreshToken)
        }

    @Test
    fun `opens the named cluster's host`() =
        runBlocking {
            val page = SealingPage(Json.encodeToString(ServedPageSession.serializer(), SIGNED_IN))
            client().signInWithServedPage(PACKAGE, "mero-chat://signed-in", page, cluster = "eu-shared-1")
            assertEquals("${AppHost.clusterLabel("eu-shared-1", PACKAGE)}.apps.calimero.network", URI(page.opened.single()).host)
        }

    @Test
    fun `a dismissed page signs nobody in`() =
        runBlocking {
            val client = client()
            try {
                client.signInWithServedPage(PACKAGE, "mero-chat://signed-in", { _, _ -> throw WebAuthenticationCancelled() })
                fail("a dismissed page is not a sign-in")
            } catch (_: WebAuthenticationCancelled) {
            }
            assertNull(client.session)
        }

    @Test
    fun `approves a step-up on the served page`() =
        runBlocking {
            val assertion = AssertionJSON("cred_1", "ad", "cd", "sig")
            val payload = """{"v":1,"assertion":${Json.encodeToString(AssertionJSON.serializer(), assertion)}}"""
            val page = SealingPage(payload, ServedPageHandoff.APPROVE_INFO)
            val passkey = ServedPagePasskey(AppHost.origin(PACKAGE), "mero-chat://approved", page, appsHost.client)
            assertEquals(assertion, passkey.get(byteArrayOf(1, 2, 3), "calimero.network", listOf("c1", "c2")))
            val fragment = URI(page.opened.single()).rawFragment
            assertTrue(fragment, fragment.endsWith("&challenge=AQID&rpId=calimero.network&allow=c1,c2"))
            val dismissed = ServedPagePasskey(AppHost.origin(PACKAGE), "mero-chat://approved", { _, _ -> throw WebAuthenticationCancelled() }, appsHost.client)
            try {
                dismissed.get(byteArrayOf(1), "calimero.network", null)
                fail("a dismissed page approves nothing")
            } catch (_: PasskeyCancelled) {
            }
        }

    @Test
    fun `a host serving no frontend is refused`() =
        runBlocking {
            val none = FakeNet { request, _ -> FakeNet.respond(request, 404, "{}") }
            try {
                AppHost.latestVersion(AppHost.origin(PACKAGE), none.client)
                fail("no release")
            } catch (e: ServedPageSignInError) {
                assertFalse(e.message.isNullOrEmpty())
            }
        }

    companion object {
        const val PACKAGE = "com.calimero.chat"

        val SIGNED_IN = ServedPageSession(1, "acc_1", "v2.claims.sig", 1_900_000_000.0, "rt_1", "https://n1.test", listOf("https://n2.test"))

        // Sealed by apps `@calimero-apps/passkey/native` (`sealNativeSession`), WebCrypto.
        const val JS_PRIVATE_KEY = "EpEiMtOxmJcC8Oyk4yxIbQYSzNzstVYDlfteYP99sc0"
        const val JS_PUBLIC_KEY = "BPm9Ej3fpM78ghV7ScdMix1F2coNyd4CzOo4wYgGl6J7HwWpT_RF9F2UFB2hmWMC18FNL7qERY4X4HC1qTzcRto"
        const val JS_STATE = "c3RhdGUtZm9yLXRoaXMtc2lnbi1pbg"
        const val JS_BOX =
            "BGXAHK69TLTGc5H45ACET70ViGXy_NxiBMP-cyei7hsPwDzi7MKUiC7fW8VglZEa8PjpOLhchHRKFKk1Yl_qnowMAVY8i-E8" +
                "-fQFYNUyYQrs3VcdqyQcQ6vAXLym7vjHXmIC8Rt2Vu4PFi_wyBGthxCsqp8VtVrmiACH-yqSw2ENb6F8WXUhIl9UInB-osdL" +
                "nEWIwqQJcVswEAPjbKlEtWCwY1PZVUVGwBr7PwJ5oSwITiQDeYI9caY5LL2yFXPYuLRWr65Q8g5148xjWgTbf6E8M-nf_mi9" +
                "vA4Vb06bx-L2vF6W-vpz6q_BO0Gq6vONxvcqmeIeHfQTPylNxuRfpAOp"

        // Sealed by apps `@calimero-apps/passkey/native` (`approveNative`), WebCrypto.
        const val JS_APPROVAL =
            "mero-chat://approved#state=c3RhdGUtZm9yLXRoaXMtc2lnbi1pbg&box=BGqRdlG34snUtVCv1fANYLwMq3Mi2-8jey_OJk-v5mj3A2AEulVm2VZeRGLWGy3elP1Y70Gm-2DtmCPSaYmk2N0fTp6LaB-u9muFZDJ2oh4IDknAWoZShiqFEIEDMh3B_SEL-ROmQUbeAAh-WUxipNxyXKonr9mRsJ1uRRv49K31ThwaIeujjSJGpo6_dGLnGlwyR8MMyFxUKv-tm-loT6ZNSjkfjXD_QhtMJSUAWA5Kmp6D4hZ8vbEjCvb3i_3XY1s3FpXSWcqHKqA"

        /** What the page does: ECDH with a fresh key, HKDF-SHA256, AES-256-GCM over [state]. */
        fun seal(
            publicKey: String,
            state: String,
            info: String,
            payload: String,
        ): String {
            val curve = SECNamedCurves.getByName("secp256r1")
            val mine =
                BigInteger(
                    1,
                    com.calimero.mero.crypto
                        .randomBytes(32),
                ).mod(curve.n.subtract(BigInteger.ONE)).add(BigInteger.ONE)
            val theirs = curve.curve.decodePoint(fromB64url(publicKey))
            val shared =
                theirs
                    .multiply(mine)
                    .normalize()
                    .affineXCoord.encoded
            val key = ByteArray(32)
            HKDFBytesGenerator(SHA256Digest()).apply { init(HKDFParameters(shared, null, info.encodeToByteArray())) }.generateBytes(key, 0, 32)
            val iv =
                com.calimero.mero.crypto
                    .randomBytes(12)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, SecretKeySpec(key, "AES"), GCMParameterSpec(128, iv))
            cipher.updateAAD(state.encodeToByteArray())
            val sealed = cipher.doFinal(payload.encodeToByteArray())
            return b64url(
                concat(
                    curve.g
                        .multiply(mine)
                        .normalize()
                        .getEncoded(false),
                    iv, sealed,
                ),
            )
        }
    }
}
