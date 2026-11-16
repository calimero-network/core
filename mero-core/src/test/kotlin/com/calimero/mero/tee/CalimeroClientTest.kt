package com.calimero.mero.tee

import com.calimero.mero.AppError
import com.calimero.mero.BadOrigin
import com.calimero.mero.CalimeroClient
import com.calimero.mero.CalimeroClientOptions
import com.calimero.mero.CalimeroConnectOptions
import com.calimero.mero.CalimeroSession
import com.calimero.mero.CloudPolicy
import com.calimero.mero.CloudReleasePolicy
import com.calimero.mero.CloudRetryOptions
import com.calimero.mero.Committed
import com.calimero.mero.ConfirmCancelled
import com.calimero.mero.ConfirmRequired
import com.calimero.mero.ConsentOutcome
import com.calimero.mero.ConsentRequest
import com.calimero.mero.ConsentResult
import com.calimero.mero.ConsentScope
import com.calimero.mero.ConsentTeamMembership
import com.calimero.mero.Deployer
import com.calimero.mero.DirectoryError
import com.calimero.mero.DirectoryFailure
import com.calimero.mero.DirectoryNode
import com.calimero.mero.Explicit
import com.calimero.mero.LoginRequired
import com.calimero.mero.MOCK_QUOTE_HEADER
import com.calimero.mero.NotFound
import com.calimero.mero.NotLeader
import com.calimero.mero.OpenConsentOptions
import com.calimero.mero.OriginNotRegistered
import com.calimero.mero.OriginTaken
import com.calimero.mero.PasskeyCancelled
import com.calimero.mero.QuotaExceeded
import com.calimero.mero.RecoveryNotReady
import com.calimero.mero.SignInOptions
import com.calimero.mero.SignUpOptions
import com.calimero.mero.SignUpRefusal
import com.calimero.mero.SignUpRefused
import com.calimero.mero.SoftPasskey
import com.calimero.mero.UnavailableError
import com.calimero.mero.WebAuthenticationCancelled
import com.calimero.mero.WrongNode
import com.calimero.mero.api.models.ApikeyCreateRequestUpgrades
import com.calimero.mero.api.models.ConsentPageResult200Response
import com.calimero.mero.api.models.SignUp
import com.calimero.mero.app.CalimeroAppBackend
import com.calimero.mero.app.OrgConsentNotApproved
import com.calimero.mero.app.OrgInvitation
import com.calimero.mero.app.PasskeyRequired
import com.calimero.mero.app.ServedPageConsent
import com.calimero.mero.app.servedPageConsent
import com.calimero.mero.confirmationCode
import com.calimero.mero.consentReturn
import com.calimero.mero.consentReturnTo
import com.calimero.mero.crypto.Ed25519
import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.SeedSigner
import com.calimero.mero.crypto.base64
import com.calimero.mero.crypto.concat
import com.calimero.mero.errorFrom
import com.calimero.mero.freshCollateral
import com.calimero.mero.fromB64url
import com.calimero.mero.newRecoverySeed
import com.calimero.mero.orderNodes
import com.calimero.mero.pageDeployment
import com.calimero.mero.parseRecoverySeed
import com.calimero.mero.recoveryPublicKey
import com.calimero.mero.recoverySeedText
import com.calimero.mero.testkit.RecordingOrgConsent
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import okhttp3.Request
import okhttp3.Response
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.security.KeyFactory
import java.security.Signature
import java.security.spec.X509EncodedKeySpec
import java.util.concurrent.atomic.AtomicInteger
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds
import com.calimero.mero.api.models.OrgRole as ApiOrgRole
import com.calimero.mero.api.models.TeamRole as ApiTeamRole

/**
 * [CalimeroClient] against stand-in cloud nodes, through the real sealed channel and a mock
 * attestation (mero-js `client-routes.test.ts`, `session-recovery.test.ts`). Synthetic data only.
 */
class CalimeroClientTest {
    private val old = CalimeroSession("acc-1", "v2.old.sig", 0, "rt_old")
    private val new = CalimeroSession("acc-1", "v2.new.sig", 0, "rt_new")

    private fun json(
        status: Int,
        body: String,
    ) = Answer(status, listOf("content-type" to "application/json"), listOf(body))

    private fun refused(
        status: Int,
        code: String,
        details: String = "{}",
    ) = json(status, """{"error":"$code","message":"${code.lowercase()} (stand-in)","details":$details}""")

    private fun bearer(head: JsonObject): String? =
        (head["headers"] as JsonArray)
            .map { it as JsonArray }
            .firstOrNull { it[0].jsonPrimitive.content == "authorization" }
            ?.get(1)
            ?.jsonPrimitive
            ?.content

    private fun path(head: JsonObject) = head["path"]!!.jsonPrimitive.content

    /** A cluster of stand-in nodes on one fake network; each attests with a mock quote. */
    private class Cluster(
        vararg answers: (JsonObject, String) -> Answer,
        /** Hosts other than the nodes (a node directory, say). */
        val extra: (Request) -> Response? = { null },
    ) {
        val nodes = answers.mapIndexed { i, answer -> StandInNode("https://node$i.example", answer) }
        val net =
            FakeNet { request, body ->
                nodes.firstOrNull { request.url.toString().startsWith(it.base) }?.let { node ->
                    if (request.url.encodedPath == "/attestation") attest(node, request) else node.handle(request, body)
                } ?: extra(request)
            }

        private fun attest(
            node: StandInNode,
            request: Request,
        ): Response {
            val nonce = Hex.decode(request.url.queryParameter("nonce")!!, "nonce", 32)
            val binding = transportKeyBinding(ByteArray(32), node.transportKey)
            val quote = concat(MOCK_QUOTE_HEADER.encodeToByteArray(), nonce, binding)
            return FakeNet.respond(
                request,
                200,
                """{"quote":"${base64(quote)}","transportKey":"${Hex.encode(node.transportKey)}","mock":true,"release":{"version":"0.0.0","profile":"dev"}}""",
            )
        }

        fun client(
            allowMock: Boolean = true,
            onSession: ((CalimeroSession?) -> Unit)? = null,
        ) = CalimeroClient(
            CalimeroClientOptions(
                baseUrl = nodes[0].base,
                nodes = nodes.drop(1).map { it.base },
                policy = CloudPolicy(allowMock = allowMock),
                httpClient = net.client,
                retry = CloudRetryOptions(baseDelayMs = 1),
                onSession = onSession,
            ),
        )
    }

    @Test
    fun `refuses a mock quote unless the client allowed mock attestation`() =
        runBlocking {
            val cluster = Cluster({ _, _ -> json(200, """{"index":1,"result":"v","events":[]}""") })
            try {
                cluster.client(allowMock = false).view("app-1", "get")
                fail("expected the mock quote to be refused")
            } catch (e: TeeTrustException) {
                assertTrue(e.message, e.message!!.contains("mock quote refused"))
            }
            assertEquals(0, cluster.nodes[0].requests.size)
            assertEquals(
                "v",
                cluster
                    .client()
                    .view("app-1", "get")
                    .result.jsonPrimitive.content,
            )
        }

    @Test
    fun `calls an app's intents with a minted idempotency key, and views with strong consistency`() =
        runBlocking {
            val cluster = Cluster({ _, _ -> json(200, """{"index":7,"result":{"ok":true},"events":[{"kind":"Set","data":1}]}""") })
            val client = cluster.client()
            client.useSession(old)
            val call = client.call("app 1", "set", buildJsonObject { put("key", "a") })
            client.view("app 1", "get", stale = true)
            assertEquals(7, call.index)
            assertEquals("Set", call.events.single().kind)
            val (callHead, callBody) = cluster.nodes[0].requests[0]
            assertEquals("/apps/app%201/intents", path(callHead))
            assertEquals("Bearer v2.old.sig", bearer(callHead))
            val sent = Json.parseToJsonElement(callBody).jsonObject
            assertTrue(sent["idempotencyKey"]!!.jsonPrimitive.content.startsWith("node:c-"))
            val view = Json.parseToJsonElement(cluster.nodes[0].requests[1].second).jsonObject
            assertEquals("stale", view["consistency"]!!.jsonPrimitive.content)
        }

    @Test
    fun `retries a 503 with the same idempotency key, then gives up as UnavailableError`() =
        runBlocking {
            val tries = AtomicInteger()
            val cluster =
                Cluster({ _, _ ->
                    if (tries.incrementAndGet() < 3) refused(503, "TIMEOUT") else json(200, """{"index":1,"result":null}""")
                })
            val client = cluster.client()
            client.call("app-1", "set")
            val keys = cluster.nodes[0].requests.map { Json.parseToJsonElement(it.second).jsonObject["idempotencyKey"] }
            assertEquals(3, keys.size)
            assertEquals(1, keys.toSet().size)

            val down = Cluster({ _, _ -> refused(503, "UNAVAILABLE") })
            try {
                down.client().call("app-1", "set")
                fail("expected UnavailableError")
            } catch (e: UnavailableError) {
                assertEquals("unavailable", e.reason)
                assertEquals(Committed.Unknown, e.committed)
            }
            assertEquals(7, down.nodes[0].requests.size)
        }

    @Test
    fun `refreshes once on a 401, shared by concurrent calls, and repeats each call`() =
        runBlocking {
            val refreshes = AtomicInteger()
            val cluster =
                Cluster({ head, body ->
                    if (path(head) == "/auth/refresh") {
                        refreshes.incrementAndGet()
                        Thread.sleep(50)
                        if (body.contains("rt_old")) {
                            json(200, """{"accountId":"acc-1","sessionToken":"v2.new.sig","expiresAt":0,"refreshToken":"rt_new"}""")
                        } else {
                            refused(401, "UNAUTHENTICATED")
                        }
                    } else if (bearer(head) == "Bearer v2.new.sig") {
                        json(200, """{"index":3,"result":"v1"}""")
                    } else {
                        refused(401, "UNAUTHENTICATED")
                    }
                })
            val seen = mutableListOf<CalimeroSession?>()
            val client = cluster.client(onSession = { synchronized(seen) { seen += it } })
            client.useSession(old)
            val answers =
                List(4) {
                    async {
                        client
                            .call("app-1", "set")
                            .result.jsonPrimitive.content
                    }
                }.awaitAll()
            assertEquals(List(4) { "v1" }, answers)
            assertEquals(1, refreshes.get())
            assertEquals(new, client.session)
            assertEquals(listOf<CalimeroSession?>(new), seen)
        }

    @Test
    fun `surfaces LoginRequired when the refresh is refused`() =
        runBlocking {
            val cluster = Cluster({ _, _ -> refused(401, "UNAUTHENTICATED") })
            val client = cluster.client()
            client.useSession(old)
            try {
                client.view("app-1", "get")
                fail("expected LoginRequired")
            } catch (_: LoginRequired) {
            }
            assertEquals(listOf("/apps/app-1/views", "/auth/refresh"), cluster.nodes[0].requests.map { path(it.first) })
        }

    @Test
    fun `moves to the next node on 421 WRONG_NODE, but not on NOT_LEADER`() =
        runBlocking {
            val cluster = Cluster({ _, _ -> refused(421, "WRONG_NODE") }, { _, _ -> json(200, """{"index":1,"result":"b"}""") })
            val client = cluster.client()
            assertEquals(
                "b",
                client
                    .view("app-1", "get")
                    .result.jsonPrimitive.content,
            )
            assertEquals(cluster.nodes[1].base, client.nodeUrl)

            val lone = Cluster({ _, _ -> refused(421, "WRONG_NODE") })
            try {
                lone.client().view("app-1", "get")
                fail("expected WrongNode")
            } catch (_: WrongNode) {
            }

            val follower = Cluster({ _, _ -> refused(421, "NOT_LEADER", """{"leaderUrl":"https://leader.example"}""") }, { _, _ -> json(200, "{}") })
            try {
                follower.client().ops.status()
                fail("expected NotLeader")
            } catch (e: NotLeader) {
                assertEquals("https://leader.example", e.leaderUrl)
            }
            assertEquals(0, follower.nodes[1].requests.size)
        }

    @Test
    fun `answers 412 CONFIRM_REQUIRED with the passkey, and a declined prompt as ConfirmCancelled`() =
        runBlocking {
            val passkey = SoftPasskey("localhost", "http://localhost:3000")
            passkey.create(buildJsonObject { put("challenge", "AAAA") })
            val cluster =
                Cluster({ _, body ->
                    if (body.contains("confirm")) {
                        // core 92ac297a1: deleting an organization answers the apps it removed.
                        json(200, """{"apps":[]}""")
                    } else {
                        refused(412, "CONFIRM_REQUIRED", """{"challengeId":"c1","challenge":"AQID","rpId":"localhost"}""")
                    }
                })
            val client = cluster.client()
            client.orgs.delete(org = "org-1", passkey = passkey)
            val confirm = Json.parseToJsonElement(cluster.nodes[0].requests[1].second).jsonObject["confirm"]!!.jsonObject
            assertEquals("c1", confirm["challengeId"]!!.jsonPrimitive.content)
            val assertion = confirm["assertion"]!!.jsonObject
            assertEquals(passkey.id, assertion["credentialId"]!!.jsonPrimitive.content)

            val declining =
                object : com.calimero.mero.PasskeyProvider by passkey {
                    override suspend fun get(
                        challenge: ByteArray,
                        rpId: String,
                        allowCredentials: List<String>?,
                    ) = throw PasskeyCancelled()
                }
            try {
                client.orgs.delete(org = "org-1", passkey = declining)
                fail("expected ConfirmCancelled")
            } catch (_: ConfirmCancelled) {
            }
        }

    @Test
    fun `streams an app's events as sealed server-sent events`() =
        runBlocking {
            val cluster =
                Cluster({ _, _ ->
                    Answer(
                        200,
                        listOf("content-type" to "text/event-stream"),
                        listOf("id: 1\nevent: Posted\ndata: {\"n\":1}\n\n", ": keep-alive\n\nid: 2\nevent: Posted\ndata: plain\n\n"),
                    )
                })
            val events =
                withTimeout(5_000) {
                    cluster
                        .client()
                        .events("app-1", from = 4)
                        .take(2)
                        .toList()
                }
            assertEquals(listOf("1", "2"), events.map { it.id })
            assertEquals(
                1,
                events[0]
                    .data.jsonObject["n"]!!
                    .jsonPrimitive.content
                    .toInt(),
            )
            assertEquals(JsonPrimitive("plain"), events[1].data)
            assertEquals("/apps/app-1/events?from=4", path(cluster.nodes[0].requests[0].first))
        }

    @Test
    fun `asks for consent and reads where the page sent the person back`() =
        runBlocking {
            val code = confirmationCode("r1")
            val cluster = Cluster({ _, _ -> json(200, """{"requestId":"r1","url":"https://node0.example/consent/r1","expiresAt":9,"code":"$code"}""") })
            var opened: String? = null
            val started =
                cluster.client().openConsentPage(ConsentRequest.Release(ConsentScope("org-1"), "2.3.110", "mero-chat://consent")) { opened = it }
            assertEquals("https://node0.example/consent/r1", opened)
            assertEquals("r1", started.requestId)
            assertEquals("the confirmation code the page shows", code, started.code)
            assertEquals(9L, started.expiresAt)
            val sent = Json.parseToJsonElement(cluster.nodes[0].requests[0].second).jsonObject
            assertEquals("release", sent["kind"]!!.jsonPrimitive.content)
            assertEquals("org-1", sent["scope"]!!.jsonObject["org"]!!.jsonPrimitive.content)
            assertEquals("approved", consentReturn("mero-chat://consent?request_id=r1&result=approved")!!.result)
            assertNull(consentReturn("mero-chat://consent?request_id=r1&result=maybe"))

            // confirm runs with the answer before the page opens; false keeps it closed.
            var shown: String? = null
            opened = null
            cluster.client().openConsentPage(
                ConsentRequest.Release(ConsentScope("org-1"), "2.3.110", "mero-chat://consent"),
                OpenConsentOptions({ opened = it }) { s ->
                    shown = s.code
                    false
                },
            )
            assertEquals(code, shown)
            assertNull(opened)

            // A code that is not the page's for the request: refused, and the page is not opened.
            val lying = Cluster({ _, _ -> json(200, """{"requestId":"r1","url":"https://node0.example/consent/r1","code":"AAA-AAA"}""") })
            try {
                lying.client().openConsentPage(ConsentRequest.Release(ConsentScope("org-1"), "2.3.110", "mero-chat://consent")) { opened = it }
                fail("expected CONSENT_CODE_MISMATCH")
            } catch (e: com.calimero.mero.CalimeroError) {
                assertEquals("CONSENT_CODE_MISMATCH", e.code)
            }
            assertNull(opened)
        }

    @Test
    fun `asks for the organization and invitation kinds and types their outcomes`() =
        runBlocking {
            var polls = 0
            val cluster =
                Cluster({ head, _ ->
                    when (path(head)) {
                        "/consent/requests" -> json(200, """{"requestId":"r2","url":"https://node0.example/consent/r2","expiresAt":9,"code":"${confirmationCode("r2")}"}""")
                        "/consent/r2/result" ->
                            if (++polls < 3) {
                                json(200, """{"requestId":"r2","kind":"createInvite","status":"pending","decidedAt":null}""")
                            } else {
                                json(200, """{"requestId":"r2","kind":"createInvite","status":"approved","decidedAt":12,"outcome":{"token":"inv_secret"}}""")
                            }
                        "/consent/r3/result" -> json(200, """{"requestId":"r3","kind":"createOrg","status":"approved","decidedAt":12,"outcome":{"orgId":"org-9"}}""")
                        "/consent/r4/result" -> json(200, """{"requestId":"r4","kind":"acceptInvite","status":"approved","decidedAt":12,"outcome":{"org":"org-1"}}""")
                        "/consent/r5/result" -> json(200, """{"requestId":"r5","kind":"createOrg","status":"declined","decidedAt":12}""")
                        "/consent/r6/result" -> json(200, """{"requestId":"r6","kind":"somethingNew","status":"approved","decidedAt":12,"outcome":{"x":"y"}}""")
                        "/consent/r7/result" -> json(200, """{"requestId":"r7","kind":"release","status":"approved","decidedAt":12}""")
                        "/consent/r8/result" -> json(200, """{"requestId":"r8","kind":"createOrg","status":"pending","decidedAt":null}""")
                        else -> refused(404, "NOT_FOUND")
                    }
                })
            val client = cluster.client()

            // Each kind's wire body: kind, subject, returnTo (no scope).
            val bodies =
                listOf(
                    ConsentRequest.CreateOrg("Acme", "https://app.example/consent"),
                    ConsentRequest.CreateInvite("org-1", ApiOrgRole.ADMIN, "https://app.example/consent", teams = listOf("t1"), ttlSecs = 3600, uses = 1, allApps = true),
                    ConsentRequest.CreateInvite("org-1", ApiOrgRole.GUEST, "https://app.example/consent"),
                    ConsentRequest.AcceptInvite("inv_secret", "https://app.example/consent"),
                )
            for (body in bodies) assertEquals("r2", client.requestConsent(body).requestId)
            val sent = cluster.nodes[0].requests.map { Json.parseToJsonElement(it.second).jsonObject }
            assertEquals(
                listOf(
                    """{"kind":"createOrg","subject":{"name":"Acme"},"returnTo":"https://app.example/consent"}""",
                    """{"kind":"createInvite","subject":{"org":"org-1","role":"admin","teams":["t1"],"ttlSecs":3600,"uses":1,"allApps":true},"returnTo":"https://app.example/consent"}""",
                    """{"kind":"createInvite","subject":{"org":"org-1","role":"guest"},"returnTo":"https://app.example/consent"}""",
                    """{"kind":"acceptInvite","subject":{"token":"inv_secret"},"returnTo":"https://app.example/consent"}""",
                ),
                sent.map { it.toString() },
            )

            // The token never reaches a string a log would print.
            val accept = ConsentRequest.AcceptInvite("inv_secret", "https://app.example/consent")
            assertFalse(accept.toString().contains("inv_secret"))
            assertEquals(accept, ConsentRequest.AcceptInvite("inv_secret", "https://app.example/consent"))
            try {
                ConsentRequest.CreateInvite("org-1", ApiOrgRole.OWNER, "https://app.example/consent")
                fail("an owner invitation is refused")
            } catch (_: IllegalArgumentException) {
            }

            // wait polls until decided; the invitation's token is typed and redacted in toString.
            val invited = client.waitForConsent("r2", 5.seconds, intervalMs = 1)
            assertEquals(3, polls)
            assertTrue(invited.isApproved)
            val outcome = invited.outcome as ConsentOutcome.InviteCreated
            assertEquals("inv_secret", outcome.token)
            assertFalse(invited.toString(), invited.toString().contains("inv_secret"))

            // resume reads the app's own request from the node; anything else is not a consent return.
            assertEquals(ConsentOutcome.OrgCreated("org-9"), client.resumeConsent("https://app.example/consent?request_id=r3&result=approved", "r3")!!.outcome)
            assertNull(client.resumeConsent("https://app.example/consent?foo=bar", "r3"))
            assertEquals(ConsentOutcome.InviteAccepted("org-1"), client.consentResult("r4").outcome)
            val declined = client.consentResult("r5")
            assertTrue(declined.isDeclined)
            assertNull(declined.outcome)
            assertEquals(ConsentOutcome.Other("somethingNew", buildJsonObject { put("x", "y") }), client.consentResult("r6").outcome)
            assertEquals(ConsentOutcome.Release, client.consentResult("r7").outcome)

            // A request still pending when the timeout passes answers the pending result.
            assertTrue(client.waitForConsent("r8", 20.milliseconds, intervalMs = 5).isPending)
        }

    @Test
    fun `refuses a consent return that is not the decision on the app's own request`() =
        runBlocking {
            val cluster =
                Cluster({ head, _ ->
                    when (path(head)) {
                        "/consent/mine/result" -> json(200, """{"requestId":"mine","kind":"createInvite","status":"pending","decidedAt":null}""")
                        "/consent/swapped/result" -> json(200, """{"requestId":"other","kind":"createInvite","status":"approved","decidedAt":1,"outcome":{"token":"t"}}""")
                        "/consent/declined/result" -> json(200, """{"requestId":"declined","kind":"createOrg","status":"declined","decidedAt":1}""")
                        else -> refused(404, "NOT_FOUND")
                    }
                })
            val client = cluster.client()

            suspend fun refusal(
                href: String,
                own: String?,
            ): com.calimero.mero.CalimeroError =
                try {
                    client.resumeConsent(href, own)
                    fail("expected a refusal")
                    error("unreachable")
                } catch (e: com.calimero.mero.CalimeroError) {
                    e
                }
            val back = "https://app.example/consent?result=approved&request_id="

            // The app started none: a return still names one.
            val notStarted = refusal(back + "mine", null)
            assertEquals(CalimeroClient.CONSENT_NOT_STARTED, notStarted.code)
            assertEquals("mine", notStarted.details["returned"]!!.jsonPrimitive.content)
            // The link names another request than the app's: refused before asking the node.
            val before = cluster.nodes[0].requests.size
            val mismatch = refusal(back + "forged", "mine")
            assertEquals(CalimeroClient.CONSENT_REQUEST_MISMATCH, mismatch.code)
            assertEquals("forged", mismatch.details["returned"]!!.jsonPrimitive.content)
            assertEquals(before, cluster.nodes[0].requests.size)
            // The page came back "approved", but the node says the app's request is pending.
            val pending = refusal(back + "mine", "mine")
            assertEquals(CalimeroClient.CONSENT_NOT_DECIDED, pending.code)
            assertEquals("mine", pending.details["requestId"]!!.jsonPrimitive.content)
            // A newer request replaced the app's: the node answers 404 for it.
            val replaced = refusal(back + "gone", "gone")
            assertEquals(CalimeroClient.CONSENT_REPLACED, replaced.code)
            assertEquals(404, replaced.status)
            assertTrue(replaced.cause is com.calimero.mero.NotFound)
            // The node answered for another request than asked.
            assertEquals(CalimeroClient.CONSENT_REQUEST_MISMATCH, refusal(back + "swapped", "swapped").code)
            // A decline is a decision: the link's `result` is not trusted, the node's status is.
            val declined = client.resumeConsent(back + "declined", "declined")!!
            assertTrue(declined.isDeclined)
            assertNull(declined.outcome)
        }

    @Test
    fun `returns native consent to the package's served page`() {
        assertEquals("https://x.apps.calimero.network/3.1.72/?native-consent=1", consentReturnTo("https://x.apps.calimero.network/", "3.1.72"))
        assertEquals("https://x.apps.calimero.network/3.1.72/?native-consent=1", consentReturnTo("https://x.apps.calimero.network", "3.1.72"))
        try {
            consentReturnTo("https://x.apps.calimero.network", "../evil")
            fail("a path is not a version")
        } catch (_: IllegalArgumentException) {
        }
    }

    @Test
    fun `asks for the organization management kinds and types their outcomes`() =
        runBlocking {
            val cluster =
                Cluster({ head, _ ->
                    when (path(head)) {
                        "/consent/requests" -> json(200, """{"requestId":"r9","url":"https://node0.example/consent/r9","expiresAt":9,"code":"${confirmationCode("r9")}"}""")
                        "/consent/ren/result" -> json(200, """{"requestId":"ren","kind":"renameOrg","status":"approved","decidedAt":1,"outcome":{"orgId":"org-1","name":"Acme Labs"}}""")
                        "/consent/del/result" -> json(200, """{"requestId":"del","kind":"deleteOrg","status":"approved","decidedAt":1,"outcome":{"apps":["a1","a2"]}}""")
                        "/consent/left/result" -> json(200, """{"requestId":"left","kind":"leaveOrg","status":"approved","decidedAt":1}""")
                        else -> refused(404, "NOT_FOUND")
                    }
                })
            val client = cluster.client()
            val to = "https://app.example/consent"
            val requests =
                listOf(
                    ConsentRequest.LeaveOrg("org-1", to),
                    ConsentRequest.RemoveMember("org-1", "acc-2", to),
                    ConsentRequest.SetMemberRole("org-1", "acc-2", ApiOrgRole.OWNER, to),
                    ConsentRequest.SetMemberTeams(
                        "org-1",
                        "acc-2",
                        listOf(ConsentTeamMembership("t1", ApiTeamRole.READ_ONLY, 4), ConsentTeamMembership("t2")),
                        to,
                    ),
                    ConsentRequest.RemoveMemberTeams("org-1", "acc-2", listOf("t1", "t2"), to),
                    ConsentRequest.RevokeInvite("org-1", "key-1", to),
                    ConsentRequest.RenameOrg("org-1", "Acme Labs", to),
                    ConsentRequest.DeleteOrg("org-1", to),
                    ConsentRequest.SetCapabilities("org-1", 3, to, account = "acc-2", team = "t1"),
                    ConsentRequest.SetCapabilities("org-1", 0, to),
                    ConsentRequest.SetAllApps("org-1", "acc-2", true, to),
                )
            for (request in requests) assertEquals("r9", client.requestConsent(request).requestId)
            val subject = """"returnTo":"https://app.example/consent""""
            assertEquals(
                listOf(
                    """{"kind":"leaveOrg","subject":{"org":"org-1"},$subject}""",
                    """{"kind":"removeMember","subject":{"org":"org-1","account":"acc-2"},$subject}""",
                    """{"kind":"setMemberRole","subject":{"org":"org-1","account":"acc-2","role":"owner"},$subject}""",
                    """{"kind":"setMemberTeams","subject":{"org":"org-1","account":"acc-2","teams":[{"team":"t1","role":"readOnly","capabilities":4},{"team":"t2"}]},$subject}""",
                    """{"kind":"removeMemberTeams","subject":{"org":"org-1","account":"acc-2","teams":["t1","t2"]},$subject}""",
                    """{"kind":"revokeInvite","subject":{"org":"org-1","inviteKey":"key-1"},$subject}""",
                    """{"kind":"renameOrg","subject":{"org":"org-1","name":"Acme Labs"},$subject}""",
                    """{"kind":"deleteOrg","subject":{"org":"org-1"},$subject}""",
                    """{"kind":"setCapabilities","subject":{"org":"org-1","account":"acc-2","team":"t1","capabilities":3},$subject}""",
                    """{"kind":"setCapabilities","subject":{"org":"org-1","capabilities":0},$subject}""",
                    """{"kind":"setAllApps","subject":{"org":"org-1","account":"acc-2","allApps":true},$subject}""",
                ),
                cluster.nodes[0].requests.map { Json.parseToJsonElement(it.second).toString() },
            )
            try {
                ConsentRequest.SetCapabilities("org-1", -1, to)
                fail("negative capabilities are refused")
            } catch (_: IllegalArgumentException) {
            }

            assertEquals(ConsentOutcome.OrgRenamed("org-1", "Acme Labs"), client.consentResult("ren").outcome)
            assertEquals(ConsentOutcome.OrgDeleted(listOf("a1", "a2")), client.consentResult("del").outcome)
            assertEquals(ConsentOutcome.LeftOrg, client.consentResult("left").outcome)
        }

    @Test
    fun `types every organization kind's outcome`() {
        fun outcome(raw: String) = Json.parseToJsonElement(raw).jsonObject
        val plain =
            mapOf(
                "leaveOrg" to ConsentOutcome.LeftOrg,
                "removeMember" to ConsentOutcome.MemberRemoved,
                "setMemberRole" to ConsentOutcome.MemberRoleSet,
                "setMemberTeams" to ConsentOutcome.MemberTeamsSet,
                "removeMemberTeams" to ConsentOutcome.MemberTeamsRemoved,
                "revokeInvite" to ConsentOutcome.InviteRevoked,
                "setCapabilities" to ConsentOutcome.CapabilitiesSet,
                "setAllApps" to ConsentOutcome.AllAppsSet,
            )
        for ((kind, expected) in plain) {
            assertEquals(expected, ConsentOutcome.of(kind, null))
            assertEquals(kind, expected.kind)
        }
        // An outcome missing its fields, or of the wrong shape, stays raw.
        assertEquals(ConsentOutcome.Other("renameOrg", outcome("""{"orgId":"o"}""")), ConsentOutcome.of("renameOrg", outcome("""{"orgId":"o"}""")))
        assertEquals(ConsentOutcome.Other("deleteOrg", outcome("""{"apps":[1]}""")), ConsentOutcome.of("deleteOrg", outcome("""{"apps":[1]}""")))
        assertEquals(ConsentOutcome.OrgDeleted(emptyList()), ConsentOutcome.of("deleteOrg", outcome("""{"apps":[]}""")))
        // Both outcomes read back as sent.
        for (text in listOf(
            """{"requestId":"r","kind":"renameOrg","status":"approved","decidedAt":3,"outcome":{"orgId":"o","name":"n"}}""",
            """{"requestId":"r","kind":"deleteOrg","status":"approved","decidedAt":3,"outcome":{"apps":["a"]}}""",
        )) {
            val result = Json.decodeFromString(ConsentResult.serializer(), text)
            assertEquals(Json.parseToJsonElement(text), Json.parseToJsonElement(Json.encodeToString(ConsentResult.serializer(), result)))
        }
    }

    @Test
    fun `types the facade's consent result outcome`() {
        fun answer(text: String) = Json.decodeFromString(ConsentPageResult200Response.serializer(), text)
        val renamed = ConsentResult.from(answer("""{"requestId":"r","kind":"renameOrg","status":"approved","decidedAt":1,"outcome":{"orgId":"o","name":"n"}}"""))
        assertEquals(ConsentOutcome.OrgRenamed("o", "n"), renamed.outcome)
        val deleted = ConsentResult.from(answer("""{"requestId":"r","kind":"deleteOrg","status":"approved","decidedAt":1,"outcome":{"apps":["a"]}}"""))
        assertEquals(ConsentOutcome.OrgDeleted(listOf("a")), deleted.outcome)
        val invite = ConsentResult.from(answer("""{"requestId":"r","kind":"createInvite","status":"approved","decidedAt":1,"outcome":{"token":"t"}}"""))
        assertEquals(ConsentOutcome.InviteCreated("t"), invite.outcome)
        assertEquals(ConsentOutcome.MemberRemoved, ConsentResult.from(answer("""{"requestId":"r","kind":"removeMember","status":"approved","decidedAt":1}""")).outcome)
        assertNull(ConsentResult.from(answer("""{"requestId":"r","kind":"renameOrg","status":"declined","decidedAt":1}""")).outcome)
    }

    @Test
    fun `types consent outcomes by kind`() {
        fun outcome(raw: String?) = raw?.let { Json.parseToJsonElement(it).jsonObject }
        assertEquals(ConsentOutcome.Upgrade, ConsentOutcome.of("upgrade", null))
        assertEquals(ConsentOutcome.Outbound, ConsentOutcome.of("outbound", outcome("{}")))
        assertEquals(ConsentOutcome.OrgCreated("o"), ConsentOutcome.of("createOrg", outcome("""{"orgId":"o"}""")))
        // A known kind without its field is not invented: it stays raw.
        assertEquals(ConsentOutcome.Other("createOrg", null), ConsentOutcome.of("createOrg", null))
        assertEquals(ConsentOutcome.Other("createInvite", outcome("""{"token":1}""")), ConsentOutcome.of("createInvite", outcome("""{"token":1}""")))
        // The result reads back as it was sent.
        val text = """{"requestId":"r","kind":"createInvite","status":"approved","decidedAt":3,"outcome":{"token":"t"}}"""
        val result = Json.decodeFromString(ConsentResult.serializer(), text)
        assertEquals(ConsentOutcome.InviteCreated("t"), result.outcome)
        assertEquals(Json.parseToJsonElement(text), Json.parseToJsonElement(Json.encodeToString(ConsentResult.serializer(), result)))
    }

    @Test
    fun `computes the consent page's confirmation code as core does`() {
        val code = confirmationCode("r1")
        assertTrue(code, Regex("^[0-9A-HJKMNP-TV-Z]{3}-[0-9A-HJKMNP-TV-Z]{3}$").matches(code))
        // SHA-256("abc") starts ba7816bf: its first 30 bits as six Crockford base32 digits (mero-js agrees).
        assertEquals("Q9W-1DF", confirmationCode("abc"))
    }

    @Test
    fun `maps the app backend onto orgs, teams and app instances`() =
        runBlocking {
            val cluster =
                Cluster({ head, body ->
                    when (path(head)) {
                        "/orgs/org-1/invites" -> json(200, """{"token":"inv_abc"}""")
                        "/orgs/org-1" -> json(200, """{"orgId":"org-1","name":"Acme","members":{"acc-1":"owner"},"teams":{"t1":{"teamId":"t1","name":"general","join":"everyone"}}}""")
                        "/orgs/org-1/apps" ->
                            json(200, """{"apps":[{"app":"a1","team":"t1","name":"chat","package":{"package":"com.calimero.chat","version":"1.0.0"}},{"app":"a2","package":{"package":"other","version":"1"}}],"next":null}""")
                        "/apps" -> json(200, """{"appId":"a3"}""").also { assertTrue(body, body.contains(""""access":{"kind":"team","team":"t1"}""")) }
                        "/invites/inv_abc/redeem" -> json(200, """{"org":"org-1"}""")
                        else -> refused(404, "NOT_FOUND")
                    }
                })
            val passkey = softPasskey()
            val backend = CalimeroAppBackend(cluster.client(), "com.calimero.chat", "acc-1") { passkey }
            val subgroups = backend.listSubgroups("org-1")
            assertEquals("org-1/t1", subgroups.single().groupId)
            assertEquals(listOf("a1"), subgroups.single().contextIds)
            assertTrue(subgroups.single().isPublic)
            assertEquals("a3", backend.createContext("org-1/t1", "com.calimero.chat", "random", JsonObject(emptyMap())))
            val invitation = backend.createInvitation("org-1")
            assertEquals(OrgInvitation("org-1", "inv_abc", name = "Acme"), OrgInvitation.decode(invitation.encoded()))
            assertEquals("org-1", backend.joinNamespace("org-1", invitation).namespaceId)
            assertTrue(!backend.isMember("missing"))
        }

    // ---------------------------------------------------------------- organization actions on the consent page

    /** A node that answers the organization routes directly, so a test sees whether they were called. */
    private fun orgNode(): Cluster =
        Cluster({ head, _ ->
            when (path(head)) {
                "/orgs" -> json(200, """{"orgId":"org_direct"}""")
                "/orgs/org_consent/metadata" -> json(200, "{}")
                "/orgs/org_consent" -> json(200, """{"orgId":"org_consent","name":"Acme","members":{"acc-1":"owner"},"teams":{}}""")
                else -> refused(404, "NOT_FOUND")
            }
        })

    private fun orgRoutes(cluster: Cluster): List<String> =
        cluster.nodes[0].requests.map { path(it.first) }.filter {
            it == "/orgs" || it.endsWith("/invites") || it.endsWith("/redeem")
        }

    @Test
    fun `with consent the backend founds, invites and joins on the consent page`() =
        runBlocking {
            val cluster = orgNode()
            val consent = RecordingOrgConsent()
            val passkey = softPasskey()
            val backend = CalimeroAppBackend(cluster.client(), "com.calimero.chat", "acc-1", { passkey }, consent)
            val founded = backend.foundNamespace("Acme")
            assertEquals("org_consent", founded.namespaceId)
            assertNull(founded.nameError)
            val invitation = backend.createInvitation("org_consent")
            assertEquals("inv_consent", invitation.token)
            assertEquals("org_joined", backend.joinNamespace("", invitation).namespaceId)
            val to = RecordingOrgConsent.RETURN_TO
            assertEquals(
                listOf(
                    ConsentRequest.CreateOrg("Acme", to).toJson(),
                    ConsentRequest.CreateInvite("org_consent", ApiOrgRole.MEMBER, to, teams = emptyList()).toJson(),
                    ConsentRequest.AcceptInvite("inv_consent", to).toJson(),
                ),
                consent.requests.map { it.toJson() },
            )
            assertEquals("the node's organization routes are not called", emptyList<String>(), orgRoutes(cluster))
            val metadata =
                cluster.nodes[0]
                    .requests
                    .single { path(it.first) == "/orgs/org_consent/metadata" }
                    .second
            assertTrue(metadata, metadata.contains(""""package":"com.calimero.chat"""") && metadata.contains(""""name":"Acme""""))
        }

    @Test
    fun `a declined consent throws and changes nothing`() =
        runBlocking {
            val cluster = orgNode()
            val backend = CalimeroAppBackend(cluster.client(), "com.calimero.chat", "acc-1", { null }, RecordingOrgConsent(status = "declined"))
            try {
                backend.foundNamespace("Acme")
                fail("a decline founds nothing")
            } catch (e: OrgConsentNotApproved) {
                assertEquals(OrgConsentNotApproved("createOrg", "declined"), e)
                assertEquals("Declined. Nothing was changed.", e.message)
            }
            try {
                backend.joinNamespace("org-1", OrgInvitation("org-1", "inv_x"))
                fail("a decline joins nothing")
            } catch (e: OrgConsentNotApproved) {
                assertEquals("acceptInvite", e.kind)
            }
            assertTrue(cluster.nodes[0].requests.isEmpty())
        }

    @Test
    fun `the name is kept from the consent page when the metadata write is refused`() =
        runBlocking {
            val cluster = Cluster({ _, _ -> refused(403, "FORBIDDEN") })
            val backend = CalimeroAppBackend(cluster.client(), "com.calimero.chat", "acc-1", { null }, RecordingOrgConsent())
            val founded = backend.foundNamespace("Acme")
            assertEquals("org_consent", founded.namespaceId)
            assertEquals("the workspace name could not be saved", founded.nameError)
        }

    @Test
    fun `declining an invite or a join throws and calls no organization route`() =
        runBlocking {
            val cluster = orgNode()
            val backend = CalimeroAppBackend(cluster.client(), "com.calimero.chat", "acc-1", { null }, RecordingOrgConsent(status = "declined"))
            try {
                backend.createInvitation("org_consent")
                fail("a declined invite makes no token")
            } catch (e: OrgConsentNotApproved) {
                assertEquals("createInvite", e.kind)
                assertEquals("Declined. Nothing was changed.", e.message)
            }
            try {
                backend.joinNamespace("org_consent", OrgInvitation("org_consent", "inv_org_consent"))
                fail("a declined join joins nothing")
            } catch (e: OrgConsentNotApproved) {
                assertEquals("acceptInvite", e.kind)
            }
            assertEquals(emptyList<String>(), orgRoutes(cluster))
        }

    @Test
    fun `a page left without deciding is not an approval`() =
        runBlocking {
            val backend = CalimeroAppBackend(orgNode().client(), "com.calimero.chat", "acc-1", { null }, RecordingOrgConsent(status = "pending"))
            try {
                backend.foundNamespace("Acme")
                fail("an undecided request founds nothing")
            } catch (e: OrgConsentNotApproved) {
                assertEquals("pending", e.status)
                assertEquals("Not approved. Nothing was changed.", e.message)
            }
        }

    @Test
    fun `an approval without its outcome is refused`() =
        runBlocking {
            val backend = CalimeroAppBackend(orgNode().client(), "com.calimero.chat", "acc-1", { null }, RecordingOrgConsent(outcome = "null"))
            try {
                backend.foundNamespace("Acme")
                fail("an approved createOrg without an org id")
            } catch (e: OrgConsentNotApproved) {
                fail("not a decline: $e")
            } catch (e: IllegalStateException) {
                assertTrue(e.message, e.message!!.contains("createOrg"))
            }
        }

    @Test
    fun `a package-bound session runs the whole app without a refused route`() =
        runBlocking {
            val fake = FakeOrgCluster().apply { boundPackage = "com.calimero.chat" }
            val cluster = Cluster(fake.node)
            val consent = ClusterConsent(fake)
            val backend = CalimeroAppBackend(cluster.client(), "com.calimero.chat", FakeOrgCluster.ME, { null }, consent)

            // Create a workspace: on the consent page, with Chat's app team.
            val org = backend.foundNamespace("Acme").namespaceId
            val appTeam = consent.appTeams.getValue(org)
            assertEquals(listOf("Acme"), backend.listNamespaces().map { it.name })

            // #general: an open channel reaches the app team.
            val general = backend.createContext(org, "", "general", JsonObject(emptyMap()))
            val created =
                cluster.nodes[0]
                    .requests
                    .last { path(it.first) == "/apps" }
                    .second
            assertTrue(created, created.contains(""""access":{"kind":"team","team":"$appTeam"}"""))
            assertEquals(listOf(general), backend.listContexts(org))
            assertEquals("the app team is not a channel", emptyList<Any>(), backend.listSubgroups(org))

            // A private channel: its team is made under the app team.
            val secret = backend.createSubgroup(org, "secret", isPublic = false)
            val secretTeam = secret.substringAfter('/')
            assertEquals(appTeam, fake.teams.getValue(secretTeam).parent)
            val inSecret = backend.createContext(secret, "", "secret", JsonObject(emptyMap()))
            val subgroups = backend.listSubgroups(org)
            assertEquals(listOf(secret), subgroups.map { it.groupId })
            assertEquals(listOf(inSecret), subgroups.single().contextIds)
            assertEquals(listOf(general), backend.listContexts(org))

            // Invite: on the consent page, naming the app team, so the invitee has Chat.
            val invitation = backend.createInvitation(org)
            assertEquals("Acme", invitation.name)
            assertEquals(listOf(appTeam), fake.inviteTeams[invitation.token])

            // Join: on the consent page.
            assertEquals(org, backend.joinNamespace("", invitation).namespaceId)

            assertEquals(listOf("createOrg", "createInvite", "acceptInvite"), consent.kinds)
            assertEquals(emptyList<String>(), fake.refused)
        }

    @Test
    fun `without the consent page a package-bound session is refused`() =
        runBlocking {
            val fake = FakeOrgCluster().apply { boundPackage = "com.calimero.chat" }
            val backend = CalimeroAppBackend(Cluster(fake.node).client(), "com.calimero.chat", FakeOrgCluster.ME)
            try {
                backend.foundNamespace("Acme")
                fail("POST /orgs is refused to a package-bound session")
            } catch (_: com.calimero.mero.CalimeroError) {
            }
            val org = fake.consentOrg("Legacy", "com.calimero.other").first
            try {
                backend.createSubgroup(org, "x", isPublic = false)
                fail("a team outside the package's app team is refused")
            } catch (_: com.calimero.mero.CalimeroError) {
            }
            assertEquals(listOf("POST /orgs", "POST /orgs/$org/teams"), fake.refused)
        }

    @Test
    fun `an org without an app team keeps org access and invites to no team`() =
        runBlocking {
            val fake = FakeOrgCluster()
            val cluster = Cluster(fake.node)
            val passkey = softPasskey()
            val backend = CalimeroAppBackend(cluster.client(), "com.calimero.chat", FakeOrgCluster.ME) { passkey }
            val org = backend.foundNamespace("Acme").namespaceId
            backend.createContext(org, "", "general", JsonObject(emptyMap()))
            val created =
                cluster.nodes[0]
                    .requests
                    .last { path(it.first) == "/apps" }
                    .second
            assertTrue(created, created.contains(""""access":{"kind":"org"}"""))
            val token = backend.createInvitation(org).token
            assertEquals(emptyList<String>(), fake.inviteTeams[token])
            val team = backend.createSubgroup(org, "t", isPublic = true)
            assertNull(fake.teams.getValue(team.substringAfter('/')).parent)
        }

    @Test
    fun `the app team is the top-level team marked with the package`() {
        fun app(pkg: String) = buildJsonObject { put("app", buildJsonObject { put("package", pkg) }) }
        val org =
            com.calimero.mero.OrgInfo(
                raw = JsonObject(emptyMap()),
                orgId = "o",
                name = "",
                members = emptyMap(),
                teams =
                    listOf(
                        com.calimero.mero.TeamInfo("t_nested", parent = "t_chat", metadata = app("com.chat")),
                        com.calimero.mero.TeamInfo("t_other", metadata = app("com.other")),
                        com.calimero.mero.TeamInfo("t_plain"),
                        com.calimero.mero.TeamInfo("t_chat", metadata = app("com.chat")),
                        com.calimero.mero.TeamInfo("t_chat2", metadata = app("com.chat")),
                    ).associateBy { it.teamId },
                metadata = JsonNull,
                memberMetadata = emptyMap(),
                releaseApprovers = emptyList(),
                publishes = emptyList(),
                newWorkStopped = null,
            )
        assertEquals("t_chat", org.appTeam("com.chat"))
        assertEquals("t_other", org.appTeam("com.other"))
        assertNull(org.appTeam("com.none"))
        assertNull(org.appTeam(""))
    }

    /** The node's consent routes for [ServedPageConsent]: request `r1`, decided as [result] says. */
    private fun consentNode(result: String): Cluster =
        Cluster(
            { head, _ ->
                when (path(head)) {
                    "/consent/requests" -> json(200, """{"requestId":"r1","url":"https://node0.example/consent/r1","expiresAt":9,"code":"${confirmationCode("r1")}"}""")
                    "/consent/r1/result" -> json(200, result)
                    else -> refused(404, "NOT_FOUND")
                }
            },
            extra = { request ->
                if (request.url.encodedPath == "/.well-known/calimero.json") FakeNet.respond(request, 200, """{"versions":["1.0.0","3.1.72"]}""") else null
            },
        )

    @Test
    fun `served page consent returns through the package's page and reads the node's decision`() =
        runBlocking {
            val cluster = consentNode("""{"requestId":"r1","kind":"createInvite","status":"approved","decidedAt":1,"outcome":{"token":"inv_1"}}""")
            val opened = mutableListOf<String>()
            val consent =
                ServedPageConsent(
                    cluster.client(),
                    "https://x.apps.calimero.network",
                    "mero-chat://consent",
                    { url, callback ->
                        opened += url
                        "$callback?request_id=r1&result=approved"
                    },
                    cluster.net.client,
                )
            val result = consent.decide { ConsentRequest.CreateInvite("org_1", ApiOrgRole.MEMBER, it, teams = listOf("t_chat")) }
            assertEquals(ConsentOutcome.InviteCreated("inv_1"), result.outcome)
            assertEquals(listOf("https://node0.example/consent/r1"), opened)
            val asked =
                cluster.nodes[0]
                    .requests
                    .single { path(it.first) == "/consent/requests" }
                    .second
            assertTrue(asked, asked.contains(""""returnTo":"https://x.apps.calimero.network/3.1.72/?native-consent=1""""))
            assertTrue(asked, asked.contains(""""kind":"createInvite"""") && asked.contains(""""teams":["t_chat"]"""))
        }

    @Test
    fun `served page consent refuses a return for another request`() =
        runBlocking {
            val cluster = consentNode("""{"requestId":"r1","kind":"createOrg","status":"approved","decidedAt":1,"outcome":{"orgId":"org-9"}}""")
            val consent =
                ServedPageConsent(cluster.client(), "https://x.apps.calimero.network", "mero-chat://consent", { _, cb -> "$cb?request_id=r2&result=approved" }, cluster.net.client)
            try {
                consent.decide { ConsentRequest.CreateOrg("Acme", it) }
                fail("another request's return is refused")
            } catch (e: com.calimero.mero.CalimeroError) {
                assertEquals("CONSENT_REQUEST_MISMATCH", e.code)
            }
        }

    @Test
    fun `served page consent refuses a return while the request is still pending`() =
        runBlocking {
            val cluster = consentNode("""{"requestId":"r1","kind":"createOrg","status":"pending","decidedAt":null}""")
            val consent =
                ServedPageConsent(cluster.client(), "https://x.apps.calimero.network", "mero-chat://consent", { _, cb -> "$cb?request_id=r1&result=approved" }, cluster.net.client)
            try {
                consent.decide { ConsentRequest.CreateOrg("Acme", it) }
                fail("a pending request's return is refused")
            } catch (e: com.calimero.mero.CalimeroError) {
                assertEquals("CONSENT_NOT_DECIDED", e.code)
            }
            // Left without deciding: the request's own (pending) result, which the backend refuses.
            val left = ServedPageConsent(cluster.client(), "https://x.apps.calimero.network", "mero-chat://consent", { _, _ -> throw WebAuthenticationCancelled() }, cluster.net.client)
            val backend = CalimeroAppBackend(cluster.client(), "com.calimero.chat", "acc-1", { null }, left)
            try {
                backend.foundNamespace("Acme")
                fail("an undecided request founds nothing")
            } catch (e: OrgConsentNotApproved) {
                assertEquals(OrgConsentNotApproved("createOrg", "pending"), e)
            }
        }

    @Test
    fun `a development client has no served page consent`() {
        assertNull(Cluster({ _, _ -> refused(404, "NOT_FOUND") }).client(allowMock = true).servedPageConsent("com.calimero.chat", "mero-chat://consent", { _, _ -> "" }))
    }

    @Test
    fun `reads the app's own error code and named fields`() {
        val error =
            errorFrom(422, Json.parseToJsonElement("""{"error":"APP_ERROR","message":"taken","details":{"code":"Taken","fields":{"name":"ada"}}}"""))
        assertTrue(error is AppError)
        assertEquals("Taken", (error as AppError).appCode)
        assertEquals("ada", error.fields["name"]!!.jsonPrimitive.content)
    }

    @Test
    fun `a soft passkey signs assertions its registered key verifies`() =
        runBlocking {
            val passkey = SoftPasskey("localhost", "http://localhost:3000")
            passkey.create(buildJsonObject { put("challenge", "AQID") })
            val restored = SoftPasskey.restore(passkey.export())
            val assertion = restored.get(byteArrayOf(9, 9), "localhost")
            val exported = Json.parseToJsonElement(passkey.export()).jsonObject
            val public = KeyFactory.getInstance("EC").generatePublic(X509EncodedKeySpec(fromB64url(exported["publicKey"]!!.jsonPrimitive.content)))
            val verifier = Signature.getInstance("SHA256withECDSA").apply { initVerify(public) }
            verifier.update(
                concat(
                    fromB64url(assertion.authenticatorData),
                    com.calimero.mero.crypto
                        .sha256(fromB64url(assertion.clientDataJson)),
                ),
            )
            assertTrue(verifier.verify(fromB64url(assertion.signature)))
            assertEquals(passkey.id, assertion.credentialId)
        }

    // ---------------------------------------------------------------- core rc.7+ (mero-js #318–#323)

    private fun sent(
        cluster: Cluster,
        index: Int,
        node: Int = 0,
    ) = Json.parseToJsonElement(cluster.nodes[node].requests[index].second).jsonObject

    private fun header(
        head: JsonObject,
        name: String,
    ): String? =
        (head["headers"] as JsonArray)
            .map { it as JsonArray }
            .firstOrNull { it[0].jsonPrimitive.content == name }
            ?.get(1)
            ?.jsonPrimitive
            ?.content

    /** Answers 412 with a challenge until the request carries `confirm`. */
    private fun stepUp(answer: String = "{}"): (JsonObject, String) -> Answer =
        { _, body ->
            if (body.contains("\"confirm\"")) {
                json(200, answer)
            } else {
                refused(412, "CONFIRM_REQUIRED", """{"challengeId":"c1","challenge":"AQID","rpId":"localhost"}""")
            }
        }

    private suspend fun softPasskey(): SoftPasskey =
        SoftPasskey("localhost", "http://localhost:3000").also { it.create(buildJsonObject { put("challenge", "AAAA") }) }

    @Test
    fun `fails a call over to the next node when one stays unavailable, with the same idempotency key`() =
        runBlocking {
            val cluster = Cluster({ _, _ -> refused(503, "UNAVAILABLE") }, { _, _ -> json(200, """{"index":2,"result":"b"}""") })
            val client = cluster.client()
            assertEquals(
                "b",
                client
                    .call("app-1", "set")
                    .result.jsonPrimitive.content,
            )
            assertEquals(cluster.nodes[1].base, client.nodeUrl)
            assertEquals(7, cluster.nodes[0].requests.size)
            val keys = (cluster.nodes[0].requests + cluster.nodes[1].requests).map { Json.parseToJsonElement(it.second).jsonObject["idempotencyKey"] }
            assertEquals(1, keys.toSet().size)

            val down = Cluster({ _, _ -> refused(503, "UNAVAILABLE") }, { _, _ -> refused(503, "MIGRATING") })
            try {
                down.client().view("app-1", "get")
                fail("expected UnavailableError")
            } catch (e: UnavailableError) {
                assertEquals("migrating", e.reason)
            }
            assertEquals("each node tried once per request", listOf(7, 7), down.nodes.map { it.requests.size })
        }

    @Test
    fun `connects to a cluster by name through the node directory, its zone first`() =
        runBlocking {
            val listing =
                """{"cluster":"eu-1","refreshAfterSeconds":30,"nodes":[
                {"id":"n0","url":"https://node0.example/","zone":"a","weight":1},
                {"id":"bad","url":"ftp://x.example","zone":"b","weight":1},
                {"id":"zero","url":"https://z.example","zone":"b","weight":0},
                {"id":"n1","url":"https://node1.example","zone":"b","weight":3}]}"""
            val cluster =
                Cluster(
                    { _, _ -> json(200, """{"index":1,"result":"a"}""") },
                    { _, _ -> json(200, """{"index":1,"result":"b"}""") },
                    extra = { request ->
                        when (request.url.toString()) {
                            "https://directory.example/v2/eu-1" -> FakeNet.respond(request, 200, listing)
                            "https://directory.example/v2/nowhere" -> FakeNet.respond(request, 404, "{}")
                            else -> null
                        }
                    },
                )
            val client =
                CalimeroClient.connect(
                    CalimeroConnectOptions("eu-1", CloudPolicy.development, directory = "https://directory.example/v2/", zone = "b", httpClient = cluster.net.client),
                )
            assertEquals(listOf("https://node1.example", "https://node0.example"), client.nodes)
            assertEquals(
                "b",
                client
                    .view("app-1", "get")
                    .result.jsonPrimitive.content,
            )
            try {
                CalimeroClient.connect(CalimeroConnectOptions("nowhere", CloudPolicy.development, directory = "https://directory.example/v2", httpClient = cluster.net.client))
                fail("expected DirectoryError")
            } catch (e: DirectoryError) {
                assertEquals(DirectoryFailure.NotFound, e.reason)
                assertEquals("nowhere", e.cluster)
            }
            // A node's weight is its chance to come first: the roll lands past the first node's share.
            val nodes = listOf(DirectoryNode("https://a.example", weight = 1), DirectoryNode("https://b.example", weight = 3))
            assertEquals(listOf("https://b.example", "https://a.example"), orderNodes(nodes, null) { 0.5 })
            assertEquals(listOf("https://a.example", "https://b.example"), orderNodes(nodes, null) { 0.1 })
        }

    @Test
    fun `answers the passkey step-ups on invites, members and API keys`() =
        runBlocking {
            val passkey = softPasskey()
            val cluster = Cluster(stepUp("""{"token":"inv_1","keyId":"k1","key":"cal_x"}"""))
            val client = cluster.client()
            assertEquals("inv_1", client.orgs.inviteCreate(org = "org-1", role = ApiOrgRole.MEMBER, teams = listOf("t1"), passkey = passkey).token)
            assertEquals("/orgs/org-1/invites", path(cluster.nodes[0].requests[1].first))
            assertEquals("c1", sent(cluster, 1)["confirm"]!!.jsonObject["challengeId"]!!.jsonPrimitive.content)
            assertEquals("member", sent(cluster, 1)["role"]!!.jsonPrimitive.content)

            client.orgs.removeMember(org = "org-1", account = "acc-2", passkey = passkey)
            assertEquals(
                "DELETE",
                cluster.nodes[0]
                    .requests[3]
                    .first["method"]!!
                    .jsonPrimitive.content,
            )
            assertTrue(sent(cluster, 3).containsKey("confirm"))
            assertEquals("cal_x", client.auth.apikeyCreate(label = "ci", passkey = passkey).key)
            client.orgs.teamSetMember(org = "org-1", team = "t1", account = "acc-2", role = ApiTeamRole.LEAD, passkey = passkey)
            assertEquals("lead", sent(cluster, 7)["role"]!!.jsonPrimitive.content)

            // The app layer without a passkey: nothing is sent.
            val sentBefore = cluster.nodes[0].requests.size
            try {
                CalimeroAppBackend(client, "com.calimero.chat", "acc-1").leaveNamespace("org-1")
                fail("expected PasskeyRequired")
            } catch (e: PasskeyRequired) {
                assertEquals("leaveNamespace", e.method)
            }
            assertEquals(sentBefore, cluster.nodes[0].requests.size)
            CalimeroAppBackend(client, "com.calimero.chat", "acc-1") { passkey }.leaveNamespace("org-1")
            assertTrue(
                Json
                    .parseToJsonElement(
                        cluster.nodes[0]
                            .requests
                            .last()
                            .second,
                    ).jsonObject
                    .containsKey("confirm"),
            )
        }

    @Test
    fun `sends a blob's content type and reads it back, with a grant when given`() =
        runBlocking {
            val cluster =
                Cluster({ head, _ ->
                    if (head["method"]!!.jsonPrimitive.content == "POST") {
                        json(200, """{"blobId":"B58","id":"ab","size":3}""")
                    } else {
                        Answer(200, listOf("content-type" to "image/png"), listOf("png"))
                    }
                })
            val client = cluster.client()
            client.uploadBlob("app-1", byteArrayOf(1, 2, 3), "image/png")
            client.uploadBlob("app-1", byteArrayOf(1))
            assertEquals("image/png", header(cluster.nodes[0].requests[0].first, "content-type"))
            assertEquals("application/octet-stream", header(cluster.nodes[0].requests[1].first, "content-type"))
            val content = client.blobContent("app-1", "B58", grant = "g+1/2")
            assertEquals("image/png", content.contentType)
            assertEquals("png", content.data.decodeToString())
            assertEquals("/apps/app-1/blobs/B58?grant=g%2B1%2F2", path(cluster.nodes[0].requests[2].first))
            assertEquals("png", client.apps.blobDownload(app = "app-1", id = "B58", grant = "g").decodeToString())
        }

    @Test
    fun `speaks the deployments, deployers and xcalls routes`() =
        runBlocking {
            val passkey = softPasskey()
            val cluster =
                Cluster({ head, body ->
                    when (path(head)) {
                        "/apps/app-1/xcalls" -> json(200, """{"pending":2,"deadLetters":[{"seq":4,"app":"app-2","method":"ping","reason":"gone","atMs":9}]}""")
                        "/deployments/d1" ->
                            json(200, """{"id":"d1","name":"Chat","owner":{"kind":"org","name":"Acme"},"apps":[{"app":"chat","package":"com.calimero.chat","version":"1.0.0"}]}""")
                        "/deployers" ->
                            if (head["method"]!!.jsonPrimitive.content == "GET") {
                                json(200, """{"deployments":"granted","deployers":"owner","owner":{"account":"acc-1"},"grants":[{"key":"org:o1","deployer":{"org":"o1"},"until":null,"untilSecs":null,"grantedAt":"t","grantedBy":"acc-1"}]}""")
                            } else {
                                stepUp("""{"key":"org:o1","deployer":{"org":"o1"},"grant":{"grantedAt":1,"grantedBy":"acc-1"}}""")(head, body)
                            }
                        "/apps" -> json(200, """{"appId":"a1","contextId":"c1","group":1,"replicas":[0],"compute":false}""")
                        "/deployers/org%3Ao1" -> stepUp("""{"key":"org:o1"}""")(head, body)
                        else ->
                            stepUp(
                                """{"id":"d2","deployment":{"id":"d2","name":"Chat","owner":{"account":"acc-1"},"apps":{},"createdBy":"acc-1","createdAt":1},"host":null}""",
                            )(head, body)
                    }
                })
            val client = cluster.client()
            val xcalls = client.apps.xcalls(app = "app-1")
            assertEquals(2L, xcalls.pending)
            assertEquals("ping", xcalls.deadLetters.single().method)
            assertEquals(
                "Acme",
                client.deployments
                    .get(id = "d1")
                    .owner.name,
            )
            val info = client.deployments.deployerList()
            assertEquals(Deployer.Org("o1"), info.grants.single().deployer)
            assertEquals(Deployer.Account("acc-1"), info.owner)
            assertEquals("org:o1", client.deployments.deployerGrant(deployer = Deployer.Org("o1"), passkey = passkey).key)
            assertEquals("o1", sent(cluster, 4)["deployer"]!!.jsonObject["org"]!!.jsonPrimitive.content)
            assertEquals("d2", client.deployments.create(name = "Chat", owner = Deployer.Account("acc-1"), passkey = passkey).id)
            assertEquals("acc-1", sent(cluster, 6)["owner"]!!.jsonObject["account"]!!.jsonPrimitive.content)
            client.deployments.deployerRevoke(key = Deployer.Org("o1").key, passkey = passkey)
            assertEquals("/deployers/org%3Ao1", path(cluster.nodes[0].requests[8].first))
            assertEquals("a1", client.apps.create(name = "chat", `package` = "com.calimero.chat", deployment = "d1").appId)
            assertEquals("d1", sent(cluster, 9)["deployment"]!!.jsonPrimitive.content)
            assertEquals("d1", pageDeployment("https://chat.example/1.0.0/?deployment=d1"))
            assertNull(pageDeployment("https://chat.example/1.0.0/"))
        }

    // ---------------------------------------------------------------- core rc.8–rc.10 (mero-js #326–#331)

    @Test
    fun `signs up with an invite and answers the sign-up's own passkey proof`() =
        runBlocking {
            val passkey = SoftPasskey("localhost", "http://localhost:3000")
            val cluster =
                Cluster({ head, body ->
                    when (path(head)) {
                        "/sign-up" -> json(200, """{"policy":"invite","orgInvites":true,"signUpInvites":true,"inviteRequired":true,"cluster":"eu-shared-1","passkeyTag":"t"}""")
                        "/auth/passkey/register/begin" -> json(200, """{"challengeId":"r1","options":{"challenge":"AAAA","rp":{"id":"localhost"}}}""")
                        else -> stepUp("""{"accountId":"acc-9","sessionToken":"v2.s.sig","expiresAt":1,"refreshToken":"rt_9"}""")(head, body)
                    }
                })
            val client = cluster.client()
            val policy = client.deployments.signUpPublic()
            assertEquals(SignUp.INVITE, policy.policy)
            assertTrue(policy.inviteRequired)
            val session = client.signUp(passkey, "ada", SignUpOptions(invite = "inv_1"))
            assertEquals("acc-9", session.accountId)
            assertEquals("v2.s.sig", client.session?.sessionToken)
            assertEquals("inv_1", sent(cluster, 1)["invite"]!!.jsonPrimitive.content)
            assertNull(sent(cluster, 1)["origin"])
            // The first finish is answered 412; its retry carries the new passkey's assertion, the
            // registration and the invite again.
            val retry = sent(cluster, 3)
            assertEquals("c1", retry["confirm"]!!.jsonObject["challengeId"]!!.jsonPrimitive.content)
            assertEquals(
                passkey.id,
                retry["confirm"]!!
                    .jsonObject["assertion"]!!
                    .jsonObject["credentialId"]!!
                    .jsonPrimitive.content,
            )
            assertTrue(retry.containsKey("registration"))
            assertEquals("inv_1", retry["invite"]!!.jsonPrimitive.content)
        }

    @Test
    fun `types sign-up refusals and names the origin a sign-in was refused for`() =
        runBlocking {
            val refusal = { message: String -> errorFrom(403, Json.parseToJsonElement("""{"error":"SIGN_UP_REFUSED","message":"$message"}""")) as SignUpRefused }
            assertEquals(SignUpRefusal.InviteOnly, refusal("sign-up refused: this cluster takes new accounts by invite only").reason)
            assertEquals(SignUpRefusal.Closed, refusal("sign-up refused: this cluster takes only accounts its operator or owner pre-authorized with a sign-up invite").reason)
            assertEquals(SignUpRefusal.UnknownInvite, refusal("sign-up refused: unknown invite").reason)
            assertEquals(SignUpRefusal.ExpiredInvite, refusal("sign-up refused: the invite has expired").reason)
            assertEquals(SignUpRefusal.OrgGone, refusal("sign-up refused: the invite's organization is gone").reason)
            assertEquals(SignUpRefusal.Other, refusal("no").reason)
            assertTrue(errorFrom(429, Json.parseToJsonElement("""{"error":"QUOTA_EXCEEDED"}""")) is QuotaExceeded)
            assertTrue(errorFrom(409, Json.parseToJsonElement("""{"error":"ORIGIN_TAKEN"}""")) is OriginTaken)
            assertEquals("BAD_RP_ID", (errorFrom(400, Json.parseToJsonElement("""{"error":"BAD_RP_ID"}""")) as BadOrigin).code)
            assertEquals(5L, (errorFrom(409, Json.parseToJsonElement("""{"error":"RECOVERY_NOT_READY","details":{"readyAt":5}}""")) as RecoveryNotReady).readyAt)

            val cluster =
                Cluster({ head, body ->
                    if (body.contains("evil")) {
                        refused(403, "ORIGIN_NOT_REGISTERED")
                    } else if (path(head) == "/auth/passkey/login/begin") {
                        json(200, """{"challengeId":"l1","options":{"challenge":"AQID","rpId":"chat.example"}}""")
                    } else {
                        json(200, """{"accountId":"acc-1","sessionToken":"v2.t.sig","refreshToken":"rt"}""")
                    }
                })
            val client = cluster.client()
            try {
                client.signIn(softPasskey(), SignInOptions(origin = "https://evil.example"))
                fail("expected OriginNotRegistered")
            } catch (e: OriginNotRegistered) {
                assertEquals("https://evil.example", e.origin)
            }
            val passkey = SoftPasskey("chat.example", "https://chat.example").also { it.create(buildJsonObject { put("challenge", "AAAA") }) }
            client.signIn(passkey, SignInOptions(origin = "https://chat.example"))
            assertEquals("https://chat.example", sent(cluster, 1)["origin"]!!.jsonPrimitive.content)
        }

    @Test
    fun `revokes API keys with the passkey and makes upgrade keys`() =
        runBlocking {
            val passkey = softPasskey()
            val cluster = Cluster(stepUp("""{"keyId":"k1","key":"cal_x"}"""))
            val client = cluster.client()
            client.auth.apikeyRevoke(key = "k 1", passkey = passkey)
            assertEquals("/auth/apikeys/k%201", path(cluster.nodes[0].requests[1].first))
            assertEquals(
                "DELETE",
                cluster.nodes[0]
                    .requests[1]
                    .first["method"]!!
                    .jsonPrimitive.content,
            )
            assertTrue(sent(cluster, 1).containsKey("confirm"))
            client.auth.apikeyCreate(label = "ci", upgrades = ApikeyCreateRequestUpgrades(app = "app-1"), passkey = passkey)
            val upgrades = sent(cluster, 2)["upgrades"]!!.jsonObject
            assertEquals(setOf("app"), upgrades.keys)
            assertEquals("app-1", upgrades["app"]!!.jsonPrimitive.content)
        }

    @Test
    fun `registers package origins with the manager's passkey`() =
        runBlocking {
            val passkey = softPasskey()
            val cluster =
                Cluster({ head, body ->
                    when (head["method"]!!.jsonPrimitive.content) {
                        "GET" ->
                            json(
                                200,
                                """{"package":"com.x","origins":[{"package":"com.x","origin":"https://x.example","rpId":"x.example","addedBy":"acc-1",""" +
                                    """"addedAt":"2026-11-05T10:00:00Z","served":false}]}""",
                            )
                        "POST" ->
                            stepUp(
                                """{"package":"com.x","origin":"https://x.example","rpId":"x.example","addedBy":"acc-1",""" +
                                    """"addedAt":"2026-11-05T10:00:00Z","served":false}""",
                            )(head, body)
                        else -> stepUp("""{"package":"com.x","origin":"https://x.example","removed":true}""")(head, body)
                    }
                })
            val client = cluster.client()
            assertEquals(
                "x.example",
                client.packages
                    .originsList(`package` = "com.x")
                    .origins
                    .single()
                    .rpId,
            )
            assertEquals("/package-origins/com.x", path(cluster.nodes[0].requests[0].first))
            // core 92ac297a1: the add answers the typed `PackageOrigin`, the remove `{ package, origin, removed }`.
            assertEquals(
                "2026-11-05T10:00:00Z",
                client.packages
                    .originsAdd(`package` = "com.x", origin = "https://x.example", rpId = "x.example", passkey = passkey)
                    .addedAt,
            )
            assertEquals("x.example", sent(cluster, 2)["rpId"]!!.jsonPrimitive.content)
            assertTrue(
                client.packages
                    .originsRemove(`package` = "com.x", origin = "https://x.example", passkey = passkey)
                    .removed,
            )
            assertEquals("https://x.example", sent(cluster, 4)["origin"]!!.jsonPrimitive.content)
        }

    @Test
    fun `recovers an organization with a seed that signs here, as core's recovery does`() =
        runBlocking {
            val seed = ByteArray(32) { it.toByte() }
            val text = recoverySeedText(seed)
            assertEquals("00010203-04050607-08090a0b-0c0d0e0f-10111213-14151617-18191a1b-1c1d1e1f", text)
            assertTrue(parseRecoverySeed(text.uppercase().replace("-", " ")).contentEquals(seed))
            assertEquals(32, fromB64url(recoveryPublicKey(seed)).size)
            assertEquals(32, newRecoverySeed().size)

            val passkey = softPasskey()
            val cluster =
                Cluster({ head, body ->
                    when (path(head)) {
                        "/auth/me" -> json(200, """{"accountId":"acc-2","name":"b","passkeys":[],"orgs":[],"via":"session","passkeyId":null,"approvedReleases":[],"outboundApprovals":{},"app":"com.x","origin":"https://x.example"}""")
                        "/orgs/o1/recovery/start" -> json(200, """{"startedAt":10,"readyAt":20}""")
                        "/orgs/o1" -> json(200, """{"orgId":"o1","name":"Acme","recovery":{"publicKey":"pk","setAt":3,"pending":{"account":"acc-2","startedAt":10,"readyAt":20}}}""")
                        "/orgs" -> json(200, """{"orgId":"o1"}""")
                        else -> stepUp("""{"org":"o1","previousOwners":["acc-1"]}""")(head, body)
                    }
                })
            val client = cluster.client()
            assertEquals("o1", client.orgs.create(name = "Acme", recoveryKey = recoveryPublicKey(seed)).orgId)
            assertEquals(recoveryPublicKey(seed), sent(cluster, 0)["recoveryKey"]!!.jsonPrimitive.content)
            assertEquals(20L, client.startRecovery("o1", seed).readyAt)
            val start = sent(cluster, 2)
            val nonce = start["nonce"]!!.jsonPrimitive.content
            assertTrue(
                Ed25519.verify(
                    fromB64url(recoveryPublicKey(seed)),
                    fromB64url(start["signature"]!!.jsonPrimitive.content),
                    "calimero.org-recovery.v1\u0000o1\u0000acc-2\u0000$nonce".encodeToByteArray(),
                ),
            )
            assertEquals(
                "acc-2",
                client
                    .getOrg("o1")
                    .recovery
                    ?.pending
                    ?.account,
            )
            assertEquals(listOf("acc-1"), client.orgs.recoveryFinish(org = "o1", passkey = passkey).previousOwners)
            client.orgs.recoverySet(org = "o1", publicKey = Explicit(null), passkey = passkey)
            assertEquals(JsonNull, sent(cluster, 7)["publicKey"])
            client.orgs.recoveryCancel(org = "o1", passkey = passkey)
            assertEquals("/orgs/o1/recovery/cancel", path(cluster.nodes[0].requests[9].first))
            val me = client.auth.me()
            assertEquals("com.x", me.app)
            assertEquals("https://x.example", me.origin)
        }

    @Test
    fun `uploads a blob's bytes as they are`() =
        runBlocking {
            val cluster = Cluster({ _, _ -> json(200, """{"blobId":"B58","id":"ab","size":3,"index":4}""") })
            val blob = cluster.client().apps.blobUpload(app = "app-1", body = byteArrayOf(1, 2, 3))
            assertEquals("B58", blob.blobId)
            assertEquals(3L, blob.propertySize)
            assertEquals("/apps/app-1/blobs", path(cluster.nodes[0].requests[0].first))
            assertEquals("application/octet-stream", header(cluster.nodes[0].requests[0].first, "content-type"))
        }

    @Test
    fun `refuses collateral older than maxCollateralAgeDays, and only then`() =
        runBlocking {
            val now =
                java.time.Instant
                    .parse("2026-10-30T12:00:00Z")
                    .toEpochMilli()
            val issued = { iso: String ->
                val fetch: suspend (ByteArray) -> DcapCollateral = {
                    DcapCollateral("", ByteArray(0), ByteArray(0), "", """{"issueDate":"$iso"}""", ByteArray(0), "", "{}", ByteArray(0))
                }
                fetch
            }
            freshCollateral(issued("2026-10-28T00:00:00Z"), 7.0) { now }(ByteArray(0))
            try {
                freshCollateral(issued("2026-10-01T00:00:00Z"), 7.0) { now }(ByteArray(0))
                fail("expected stale collateral to be refused")
            } catch (e: TeeTrustException) {
                assertTrue(e.message, e.message!!.contains("issued 29 days ago, more than the 7 this client accepts"))
            }
            val any = issued("2020-01-01T00:00:00Z")
            assertTrue(freshCollateral(any, null) === any)
            assertEquals(DEFAULT_COLLATERAL_MIRROR, CloudReleasePolicy("2.3.109").collateralSource)
        }

    @Test
    fun `the generated facade goes through the sealed transport and answers a 412 with the client's passkey`() =
        runBlocking {
            val cluster = Cluster(stepUp("""{"token":"inv_1"}"""))
            val client = cluster.client()
            client.passkey = softPasskey()
            val token = client.orgs.inviteCreate(org = "org 1", role = ApiOrgRole.MEMBER, teams = listOf("t1")).token
            assertEquals("inv_1", token)
            assertEquals("/orgs/org%201/invites", path(cluster.nodes[0].requests[1].first))
            assertEquals("member", sent(cluster, 1)["role"]!!.jsonPrimitive.content)
            assertEquals("c1", sent(cluster, 1)["confirm"]!!.jsonObject["challengeId"]!!.jsonPrimitive.content)
            // Optional fields left out, never sent as null.
            assertTrue(!sent(cluster, 0).containsKey("ttlSecs"))
        }

    @Test
    fun `an app transfer answers null, which the generated facade returns as Unit`() =
        runBlocking {
            // core 92ac297a1: `POST /apps/{app}/transfer` answers `null` (`{ type: null }`).
            val cluster = Cluster(stepUp("null"))
            val client = cluster.client()
            client.passkey = softPasskey()
            val answer: Unit = client.apps.transfer(app = "app 1", to = "acc-2")
            assertEquals(Unit, answer)
            assertEquals("/apps/app%201/transfer", path(cluster.nodes[0].requests[1].first))
            assertEquals("acc-2", sent(cluster, 1)["to"]!!.jsonPrimitive.content)
            assertEquals("c1", sent(cluster, 1)["confirm"]!!.jsonObject["challengeId"]!!.jsonPrimitive.content)
        }

    @Test
    fun `the generated facade throws the typed refusal, and a 412 without a passkey reaches the caller`() =
        runBlocking {
            val cluster = Cluster({ head, _ -> if (path(head).startsWith("/apps/")) refused(404, "NOT_FOUND") else refused(412, "CONFIRM_REQUIRED", "{}") })
            val client = cluster.client()
            try {
                client.apps.get(app = "app-1")
                fail("expected NotFound")
            } catch (e: NotFound) {
                assertEquals(404, e.status)
            }
            try {
                client.orgs.inviteCreate(org = "org-1", role = ApiOrgRole.MEMBER)
                fail("expected ConfirmRequired")
            } catch (e: ConfirmRequired) {
                assertEquals(412, e.status)
            }
        }

    @Test
    fun `the generated facade signs a 428 with the client's operator key`() =
        runBlocking {
            val statement = "release-deadline-lift 1.0.0".encodeToByteArray()
            val cluster =
                Cluster({ _, body ->
                    if (body.contains("\"operatorProof\"")) {
                        // core 92ac297a1: lifting a deadline answers the deadlines left.
                        json(200, """{"deadlines":[]}""")
                    } else {
                        refused(
                            428,
                            "OPERATOR_SIGNATURE_REQUIRED",
                            """{"subject":"s","method":"m","args":{},"nonce":"n1","cluster":"c","message":"${base64(statement)}"}""",
                        )
                    }
                })
            val client = cluster.client()
            val seed = ByteArray(32) { it.toByte() }
            client.operator = SeedSigner(Hex.encode(seed))
            client.ops.releaseDeadlineLift(version = "1.0.0")
            val proof = sent(cluster, 1)["operatorProof"]!!.jsonObject
            assertEquals("n1", proof["nonce"]!!.jsonPrimitive.content)
            val signature = Hex.decode(proof["signature"]!!.jsonPrimitive.content, "signature", 64)
            assertTrue(Ed25519.verify(Ed25519.publicKey(seed), signature, statement))
            assertEquals(
                "DELETE",
                cluster.nodes[0]
                    .requests[1]
                    .first["method"]!!
                    .jsonPrimitive.content,
            )
        }

    @Test
    fun `a generated route takes an id as one path segment and refuses a dot segment before sending`() =
        runBlocking {
            val org =
                """{"orgId":"a/b?c","name":"n","members":{},"teams":{},"defaultCapabilities":0,"capabilities":{},"approvedReleases":[],""" +
                    """"outboundApprovals":{},"metadata":null,"memberMetadata":{},"upgradeRuns":{}}"""
            val cluster = Cluster({ _, _ -> json(200, org) })
            val client = cluster.client()
            assertEquals("a/b?c", client.orgs.get(org = "a/b?c").orgId)
            assertEquals("/orgs/a%2Fb%3Fc", path(cluster.nodes[0].requests[0].first))
            for (bad in listOf("", ".", "..")) {
                try {
                    client.orgs.get(org = bad)
                    fail("expected IllegalArgumentException for '$bad'")
                } catch (_: IllegalArgumentException) {
                }
            }
            assertEquals(1, cluster.nodes[0].requests.size)
        }

    @Test
    fun `the facade sends a meaningful null only when asked, and leaves other nulls out`() =
        runBlocking {
            val cluster = Cluster({ _, _ -> json(200, "{}") })
            val client = cluster.client()
            client.orgs.teamUpdate(org = "o1", team = "t1", parent = Explicit(null))
            assertEquals(JsonNull, sent(cluster, 0)["parent"])
            assertTrue(!sent(cluster, 0).containsKey("name"))
            client.orgs.teamUpdate(org = "o1", team = "t1", name = "n")
            assertTrue(!sent(cluster, 1).containsKey("parent"))
            client.orgs.teamUpdate(org = "o1", team = "t1", parent = Explicit("t0"))
            assertEquals("t0", sent(cluster, 2)["parent"]!!.jsonPrimitive.content)
        }

    @Test
    fun `the facade sends a binary body with the spec's media type`() =
        runBlocking {
            val cluster = Cluster({ _, _ -> json(200, """{"blobId":"B58","id":"ab","size":3,"index":4,"hash":"h"}""") })
            val client = cluster.client()
            assertEquals("B58", client.apps.blobUpload(app = "app-1", body = byteArrayOf(1, 2, 3)).blobId)
            assertEquals("application/octet-stream", header(cluster.nodes[0].requests[0].first, "content-type"))
            assertEquals("h", client.packages.bundlePut(body = byteArrayOf(0, 0x61, 0x73, 0x6d)).hash)
            assertEquals("application/wasm", header(cluster.nodes[0].requests[1].first, "content-type"))
        }
}
