package com.calimero.mero.cluster

import com.calimero.mero.BadOrigin
import com.calimero.mero.CalimeroClient
import com.calimero.mero.CalimeroError
import com.calimero.mero.ConfirmRequired
import com.calimero.mero.ConsentOutcome
import com.calimero.mero.ConsentRequest
import com.calimero.mero.ConsentStarted
import com.calimero.mero.LoginRequired
import com.calimero.mero.NotFound
import com.calimero.mero.OriginNotRegistered
import com.calimero.mero.PasskeyUnknownHere
import com.calimero.mero.RecoveryNotReady
import com.calimero.mero.SignInOptions
import com.calimero.mero.SignUpOptions
import com.calimero.mero.SoftPasskey
import com.calimero.mero.WrongRecoveryCode
import com.calimero.mero.api.models.OrgRole
import com.calimero.mero.app.CalimeroAppBackend
import com.calimero.mero.app.GovernanceMemberRole
import com.calimero.mero.app.OrgInvitation
import com.calimero.mero.app.PasskeyRequired
import com.calimero.mero.cluster.Cluster.client
import com.calimero.mero.cluster.Cluster.deployKv
import com.calimero.mero.cluster.Cluster.kv
import com.calimero.mero.cluster.Cluster.person
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.BeforeClass
import org.junit.Test
import java.util.Base64

/**
 * The rest of mero-kotlin against a real cluster ([Cluster]): the consent page, the app layer
 * ([CalimeroAppBackend]), approvals, packages and their origins, upgrade runs, moving apps.
 */
class ClusterAppTest {
    companion object {
        private val PKG = "com.mero-kotlin.kv${Cluster.run}"
        private val http = OkHttpClient()

        @BeforeClass
        @JvmStatic
        fun cluster() = Cluster.require()
    }

    /**
     * What the node's consent page does on *Approve* (`POST /consent/{id}/approve`, in the clear):
     * the route's `412` over the frozen request, then the passkey's assertion as `confirm`.
     */
    private suspend fun approveOnPage(
        started: ConsentStarted,
        pk: SoftPasskey,
    ): JsonObject {
        val url = started.url.substringBefore('?').trimEnd('/') + "/approve"

        fun post(body: JsonObject): Pair<Int, JsonObject> {
            val request =
                Request
                    .Builder()
                    .url(url)
                    .post(body.toString().toRequestBody("application/json".toMediaType()))
                    .build()
            return http.newCall(request).execute().use { r -> r.code to Json.parseToJsonElement(r.body!!.string()).jsonObject }
        }
        val (status, asked) = post(JsonObject(emptyMap()))
        assertEquals("$asked", 412, status)
        val challenge = asked["details"]!!.jsonObject
        val assertion =
            pk.get(
                Base64.getUrlDecoder().decode(challenge["challenge"]!!.jsonPrimitive.content),
                challenge["rpId"]?.jsonPrimitive?.content.orEmpty(),
                (challenge["allowCredentials"] as? JsonArray)?.map { it.jsonPrimitive.content },
            )
        val (done, answer) =
            post(
                buildJsonObject {
                    put(
                        "confirm",
                        buildJsonObject {
                            put("challengeId", challenge["challengeId"]!!.jsonPrimitive.content)
                            put(
                                "assertion",
                                buildJsonObject {
                                    put("credentialId", assertion.credentialId)
                                    put("authenticatorData", assertion.authenticatorData)
                                    put("clientDataJson", assertion.clientDataJson)
                                    put("signature", assertion.signature)
                                },
                            )
                        },
                    )
                },
            )
        assertEquals("$answer", 200, done)
        return answer
    }

    @Test
    fun createsAnOrgOnTheConsentPage() =
        runBlocking<Unit> {
            val (alice, pk) = person(0, "alice-consent")
            val started = alice.requestConsent(ConsentRequest.CreateOrg("Consented Co", "${Cluster.ORIGIN}/back"))
            assertTrue(started.code, Regex("[A-Z0-9]{3}-[A-Z0-9]{3}").matches(started.code))
            assertEquals("pending", alice.consentResult(started.requestId).status)
            approveOnPage(started, pk)
            val result = alice.consentResult(started.requestId)
            assertTrue("$result", result.isApproved)
            val org = (result.outcome as ConsentOutcome.OrgCreated).orgId
            assertEquals("Consented Co", alice.orgs.get(org = org).name)
            assertEquals(result, alice.waitForConsent(started.requestId))
        }

    @Test
    fun runsAnAppsWorkspacesThroughTheAppBackend() =
        runBlocking<Unit> {
            val (alice, alicePk) = person(0, "alice-backend")
            val (bob, bobPk) = person(1, "bob-backend")
            val aliceId = alice.auth.me().accountId
            val bobId = bob.auth.me().accountId
            alice.packages.upload(body = signedMpk(PKG, "1.0.0", variant(Cluster.kvRaw, "backend${Cluster.run}"), Publisher()))

            val asAlice = CalimeroAppBackend(alice, PKG, aliceId) { alicePk }
            val org = asAlice.foundNamespace("Workspace").namespaceId
            assertTrue(asAlice.listNamespaces().any { it.id == org })
            val general = asAlice.createSubgroup(org, "general", isPublic = true)
            assertTrue(asAlice.listSubgroups(org).any { it.groupId == general })
            val channel = asAlice.createContext(general, PKG, "chat", buildJsonObject {})
            assertEquals(listOf(channel), asAlice.listContexts(general))
            asAlice.contract.execute(channel, "set", kv("a", "1"))
            assertEquals(JsonPrimitive("1"), asAlice.contract.query(channel, "get", kv("a")))

            // Bob joins by invitation, reaches the open team's channel, and is named.
            val invitation = asAlice.createInvitation(org)
            val asBob = CalimeroAppBackend(bob, PKG, bobId) { bobPk }
            assertEquals(org, asBob.joinNamespace(org, OrgInvitation(org, invitation.token)).namespaceId)
            assertTrue(asBob.listNamespaces().any { it.id == org })
            delay(300)
            assertTrue(asBob.isMember(channel))
            asBob.setMemberName(org, "Bob")
            assertEquals("Bob", asAlice.listMembers(org).first { it.identity == bobId }.name)

            // Names, events (resumed by id), blobs.
            asAlice.setGroupName(general, "General")
            assertEquals("General", asAlice.groupMetadata(general)?.name)
            asAlice.setContextName(general, channel, "Chat")
            assertEquals("Chat", asAlice.contextMetadata(general, channel)?.name)
            asAlice.contract.execute(channel, "set", kv("b", "2"))
            val seen = withTimeout(10_000) { asBob.events(channel).take(2).toList() }
            assertEquals(2, seen.size)
            val resumed = withTimeout(10_000) { bob.events(channel, after = seen[0].id!!).first() }
            assertEquals(seen[1].id, resumed.id)
            val blob = asBob.uploadBlob(channel, "hi".toByteArray())
            assertTrue("hi".toByteArray().contentEquals(asAlice.getBlob(blob.blobId, channel)))

            // Roles and removal are passkey step-ups; without one the backend sends nothing.
            refused<PasskeyRequired> { CalimeroAppBackend(alice, PKG, aliceId).setMemberRole(org, bobId, GovernanceMemberRole.ReadOnly) }
            asAlice.setMemberRole(org, bobId, GovernanceMemberRole.ReadOnly)
            assertEquals("ReadOnly", asAlice.listMembers(org).first { it.identity == bobId }.role)
            asAlice.removeMember(org, bobId)
            delay(300)
            assertTrue(asBob.listNamespaces().none { it.id == org })
            refused<NotFound> { bob.view(channel, "get", kv("a")) }

            // A team holding an app goes with the owner's passkey; the backend gives it.
            asAlice.deleteSubgroup(general)
            delay(300)
            assertTrue(asAlice.listSubgroups(org).none { it.groupId == general })
        }

    @Test
    fun reportsPendingApprovalsAndTheAccountsActivity() =
        runBlocking<Unit> {
            val (alice, pk) = person(0, "alice-approvals")
            val me = alice.auth.me()
            val org = alice.orgs.create(name = "Approvals Co").orgId
            val app = deployKv(alice, "approvals-kv", org = org)
            val report = alice.appsReportAll(org)
            assertTrue(report.apps.any { it.app == app })
            val pending = alice.pendingApprovals(org)
            assertNotNull(pending.governance)
            val mine = alice.auth.accountApps().apps
            assertNotNull(mine)

            val activity = alice.auth.activityList().activity
            assertEquals("signIn", activity.first().kind.value)
            alice.auth.apikeyCreate(label = "ci", passkey = pk)
            val after = alice.auth.activityList().activity
            assertEquals("__create_api_key", after.first().method)
            assertEquals(me.passkeyId, after.first().passkey)
        }

    @Test
    fun upgradesAnOrgsAppsInOneRunAndMovesAnApp() =
        runBlocking<Unit> {
            val (alice, pk) = person(0, "alice-upgrade")
            val (bob) = person(1, "bob-upgrade")
            val bobId = bob.auth.me().accountId
            val key = Publisher()
            val pkg = "com.mero-kotlin.upgrade${Cluster.run}"
            alice.packages.upload(body = signedMpk(pkg, "1.0.0", variant(Cluster.kvRaw, "u100${Cluster.run}"), key))
            alice.packages.upload(body = signedMpk(pkg, "1.0.1", variant(Cluster.kvRaw, "u101${Cluster.run}"), key))
            // Another key cannot release the same package.
            assertEquals(403, refused<CalimeroError> { alice.packages.upload(body = signedMpk(pkg, "1.1.0", variant(Cluster.kvRaw, "u110${Cluster.run}"), Publisher())) }.status)
            assertEquals("NO_REGISTRY", refused<CalimeroError> { alice.packages.install(`package` = pkg, version = "1.0.1") }.code)

            val org = alice.orgs.create(name = "Upgrade Co").orgId
            alice.orgs.setMember(org = org, account = bobId, role = OrgRole.MEMBER, passkey = pk)
            val app = alice.apps.create(name = "upgrade-kv", `package` = pkg, version = "1.0.0", org = org).appId
            alice.apps.intents(app = app, method = "set", args = kv("a", "1"))

            refused<ConfirmRequired> { alice.orgs.upgrade(org = org, `package` = pkg, version = "1.0.1") }
            var run = alice.orgs.upgrade(org = org, `package` = pkg, version = "1.0.1", passkey = pk)
            var tries = 0
            while (run.finished != true && tries++ < 50) {
                delay(200)
                run = alice.orgs.upgradeStatus(org = org, run = run.runId)
            }
            assertEquals(true, run.finished)
            assertEquals(listOf(app), run.done)
            assertEquals(
                "1.0.1",
                alice.apps
                    .list(`package` = pkg)
                    .apps
                    .first { it.appId == app }
                    .version,
            )
            assertEquals(JsonPrimitive("1"), alice.apps.views(app = app, method = "get", args = kv("a")).result)
            assertEquals(
                listOf("1.0.0", "1.0.1"),
                alice.packages
                    .get(`package` = pkg)
                    .versions
                    .map { it.version },
            )

            // Moving it, with its people, into another organization.
            val dst = alice.orgs.create(name = "Upgrade Dst").orgId
            refused<ConfirmRequired> { alice.apps.move(app = app, toOrg = dst) }
            val moved = alice.apps.move(app = app, toOrg = dst, bringMembers = true, passkey = pk)
            assertEquals(dst, moved.org)
            assertEquals(listOf(bobId), moved.added)
            assertEquals(JsonPrimitive("1"), bob.apps.views(app = app, method = "get", args = kv("a")).result)

            // Every app added later goes to a member the admin names.
            alice.orgs.allApps(org = dst, account = bobId, allApps = true, passkey = pk)
            assertEquals(listOf(bobId), alice.orgs.get(org = dst).allApps)
        }

    @Test
    fun registersAPackagesOriginAndSignsInFromIt() =
        runBlocking<Unit> {
            val (alice, pk) = person(0, "alice-origins")
            val pkg = "com.mero-kotlin.origins${Cluster.run}"
            alice.packages.upload(body = signedMpk(pkg, "1.0.0", variant(Cluster.kvRaw, "origins${Cluster.run}"), Publisher()))
            assertEquals(emptyList<Any>(), alice.packages.originsList(`package` = pkg).origins)
            val site = "https://chat${Cluster.run}.mero-kotlin.test"
            refused<ConfirmRequired> { alice.packages.originsAdd(`package` = pkg, origin = site) }
            val added = alice.packages.originsAdd(`package` = pkg, origin = site, passkey = pk)
            assertEquals("chat${Cluster.run}.mero-kotlin.test", added.rpId)
            refused<BadOrigin> { alice.packages.originsAdd(`package` = pkg, origin = "not an origin", passkey = pk) }

            val carol = client(1)
            val carolPk = SoftPasskey("chat${Cluster.run}.mero-kotlin.test", site)
            carol.signUp(carolPk, "carol", SignUpOptions(origin = site))
            assertEquals(site, carol.auth.me().origin)
            client(2).signIn(carolPk, SignInOptions(origin = site))
            val stranger =
                refused<OriginNotRegistered> {
                    client(0).signIn(SoftPasskey("stranger.mero-kotlin.test", "https://stranger.mero-kotlin.test"), SignInOptions(origin = "https://stranger.mero-kotlin.test"))
                }
            assertEquals("https://stranger.mero-kotlin.test", stranger.origin)

            alice.packages.originsRemove(`package` = pkg, origin = site, passkey = pk)
            delay(300)
            refused<LoginRequired> { carol.auth.me() }
            refused<NotFound> { alice.packages.originsRemove(`package` = pkg, origin = site, passkey = pk) }
        }

    @Test
    fun recoversAnAccountWithItsRecoveryCode() =
        runBlocking<Unit> {
            val (alice, pk) = person(0, "alice-recovery")
            val me = alice.auth.me()
            refused<ConfirmRequired> { alice.auth.recoveryCreate() }
            val code = alice.auth.recoveryCreate(passkey = pk).code
            assertEquals(true, alice.auth.me().recoveryCode)

            refused<WrongRecoveryCode> { client(1).startAccountRecovery("not-a-code", SoftPasskey("localhost", Cluster.ORIGIN)) }
            val lost = client(1)
            val fresh = SoftPasskey("localhost", Cluster.ORIGIN)
            val started = lost.startAccountRecovery(code, fresh)
            assertEquals(me.accountId, started.accountId)
            assertTrue(started.readyAt > started.startedAt)
            // 72 hours to go: not yet.
            refused<RecoveryNotReady> { lost.finishAccountRecovery(fresh, started.accountId) }
            // A passkey the account still has cancels it; the code is spent.
            alice.auth.accountRecoveryCancel(passkey = pk)
            refused<WrongRecoveryCode> { client(2).startAccountRecovery(code, SoftPasskey("localhost", Cluster.ORIGIN)) }
        }

    @Test
    fun namesAPasskeyThisClusterDoesNotKnow() =
        runBlocking<Unit> {
            val (alice, pk) = person(0, "alice-unknown")
            val second = SoftPasskey("localhost", Cluster.ORIGIN)
            val cred = alice.addPasskey(second, pk)
            alice.auth.passkeyRemove(cred = cred, passkey = pk)
            val e = refused<CalimeroError> { client(1).signIn(second) }
            assertTrue("$e", e is PasskeyUnknownHere)
        }

    @Test
    fun keepsServingAfterARefreshOnAnotherNode() =
        runBlocking<Unit> {
            val (alice, pk) = person(1, "alice-nodes")
            val app = deployKv(alice, "kv-nodes")
            alice.apps.intents(app = app, method = "set", args = kv("k", "v"))
            val other: CalimeroClient = client(2)
            other.signIn(pk)
            other.refresh()
            assertEquals(JsonPrimitive("v"), other.apps.views(app = app, method = "get", args = kv("k")).result)
        }
}
