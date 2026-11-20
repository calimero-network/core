package com.calimero.mero.cluster

import com.calimero.mero.AppGrant
import com.calimero.mero.CalimeroClient
import com.calimero.mero.CalimeroError
import com.calimero.mero.ConfirmRequired
import com.calimero.mero.Explicit
import com.calimero.mero.LoginRequired
import com.calimero.mero.NotFound
import com.calimero.mero.SoftPasskey
import com.calimero.mero.api.apis.OrganizationsApi
import com.calimero.mero.api.models.OrgRole
import com.calimero.mero.api.models.TeamJoin
import com.calimero.mero.tee.TeeTrustException
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.BeforeClass
import org.junit.Test

/** mero-kotlin's client against a real local merod 1.0 cluster ([Cluster]): mero-js's `client.test.ts`. */
class ClusterTest {
    companion object {
        @BeforeClass
        @JvmStatic
        fun cluster() = Cluster.require()
    }

    private fun client(
        i: Int,
        allowMock: Boolean = true,
    ) = Cluster.client(i, allowMock)

    private suspend fun person(
        i: Int,
        name: String,
    ) = Cluster.person(i, name)

    private suspend fun deployKv(
        c: CalimeroClient,
        name: String,
        org: String? = null,
        grants: List<AppGrant>? = null,
    ) = Cluster.deployKv(c, name, org, grants)

    private fun kv(
        key: String,
        value: String? = null,
    ) = Cluster.kv(key, value)

    @Test
    fun refusesAMockQuoteUnlessAllowed() =
        runBlocking<Unit> {
            val e = refused<Throwable> { client(0, allowMock = false).auth.loginBegin() }
            assertTrue("$e", generateSequence(e) { it.cause }.any { it is TeeTrustException && "mock quote refused" in it.message.orEmpty() })
        }

    @Test
    fun signsUpDeploysKvAndCallsItSealed() =
        runBlocking<Unit> {
            val (alice, pk) = person(0, "alice")
            val me = alice.auth.me()
            assertEquals("session", me.via.value)

            val again = client(1)
            val s = again.signIn(pk)
            assertEquals(me.accountId, s.accountId)

            val app = deployKv(alice, "kv")
            val r = alice.apps.intents(app = app, method = "set", args = kv("a", "1"), idempotencyKey = "k1")
            assertTrue(r.index > 0)
            assertEquals(1, r.events.size)
            val replay = alice.apps.intents(app = app, method = "set", args = kv("a", "other"), idempotencyKey = "k1")
            assertEquals(r.index, replay.index)

            assertEquals(JsonPrimitive("1"), again.apps.views(app = app, method = "get", args = kv("a")).result)
            assertEquals(JsonPrimitive("1"), again.view(app, "get", kv("a"), stale = true, minIndex = r.index).result)

            alice.apps.intents(app = app, method = "set", args = kv("b", "2"))

            // Events replay from the start; each id is `<index>:<seq>`.
            val first = withTimeout(10_000) { alice.events(app, 0).first() }
            assertTrue(first.kind.isNotEmpty())
            assertTrue(first.id, Regex("""\d+:\d+""").matches(first.id))
            // Resuming after that id skips it.
            val next = withTimeout(10_000) { alice.events(app, after = first.id).first() }
            assertNotEquals(first.id, next.id)

            // A confirm method needs a fresh passkey assertion over the exact call.
            refused<ConfirmRequired> { alice.apps.intents(app = app, method = "clear", args = buildJsonObject {}) }
            alice.apps.intents(app = app, method = "clear", args = buildJsonObject {}, passkey = pk)
            assertEquals(
                0L,
                alice.apps
                    .views(app = app, method = "len", args = buildJsonObject {})
                    .result!!
                    .jsonPrimitive.long,
            )

            // Others cannot reach an owner-only app.
            val (bob) = person(2, "bob")
            assertEquals("NOT_FOUND", refused<CalimeroError> { bob.apps.intents(app = app, method = "set", args = kv("x", "y")) }.code)

            // A read-only API key reads, never writes.
            val key = alice.auth.apikeyCreate(readOnly = true, apps = listOf(app), passkey = pk).key
            val agent = client(2)
            agent.useApiKey(key)
            agent.apps.views(app = app, method = "len", args = buildJsonObject {})
            assertEquals("FORBIDDEN", refused<CalimeroError> { agent.apps.intents(app = app, method = "set", args = kv("x", "y")) }.code)

            // Refresh rotates the session.
            val fresh = alice.refresh()
            assertNotEquals(s.sessionToken, fresh.sessionToken)
        }

    @Test
    fun organizationsGateAppsAndRemovalCutsAccessOff() =
        runBlocking<Unit> {
            val (alice, alicePk) = person(0, "alice-org")
            val (bob) = person(1, "bob-org")
            val bobId = bob.auth.me().accountId
            val org = alice.orgs.create(name = "Acme").orgId
            val team = alice.orgs.teamCreate(org = org, name = "eng", join = TeamJoin.INVITE).teamId
            val token = alice.orgs.inviteCreate(org = org, role = OrgRole.MEMBER, teams = listOf(team), passkey = alicePk).token
            bob.orgs.inviteRedeem(token = token)

            val app = deployKv(alice, "team-kv", org = org)
            bob.apps.intents(app = app, method = "set", args = kv("from", "bob"))

            refused<ConfirmRequired> { alice.orgs.removeMember(org = org, account = bobId) }
            alice.orgs.removeMember(org = org, account = bobId, passkey = alicePk)
            delay(300)
            assertEquals("NOT_FOUND", refused<CalimeroError> { bob.apps.intents(app = app, method = "set", args = kv("x", "y")) }.code)
        }

    @Test
    fun storesAnAppsBlobsAndServesThemOnlyToThatApp() =
        runBlocking<Unit> {
            val (alice) = person(0, "alice-blobs")
            val app = deployKv(alice, "files")
            val other = deployKv(alice, "other")
            val data = "%PDF-1.7 ".repeat(500).toByteArray()
            val up = alice.apps.blobUpload(app = app, body = data)
            assertEquals(data.size.toLong(), up.propertySize)
            assertTrue(data.contentEquals(alice.blobContent(app, up.blobId).data))
            assertTrue(data.contentEquals(alice.blobContent(app, up.id).data))
            refused<NotFound> { alice.blobContent(other, up.blobId) }
        }

    @Test
    fun installsASignedPackageAndRunsIt() =
        runBlocking<Unit> {
            val mpk = Cluster.kvMpk
            assumeTrue("MERO_KV_MPK not set", mpk != null)
            val (alice) = person(0, "alice-packages")
            assertEquals("BAD_PACKAGE", refused<CalimeroError> { alice.packages.upload(body = byteArrayOf(1, 2, 3)) }.code)
            val installed = alice.packages.upload(body = mpk!!)
            val app = alice.apps.create(name = "pkg-kv", `package` = installed.`package`, version = installed.version).appId
            alice.apps.intents(app = app, method = "set", args = kv("a", "1"))
            assertEquals(JsonPrimitive("1"), alice.apps.views(app = app, method = "get", args = kv("a")).result)
            assertTrue(
                alice.apps
                    .list(`package` = installed.`package`)
                    .apps
                    .any { it.appId == app },
            )
            assertTrue(
                alice.packages
                    .list()
                    .packages
                    .any { it.`package` == installed.`package` },
            )
            alice.apps.delete(app = app)
            assertEquals(404, refused<CalimeroError> { alice.apps.get(app = app) }.status)
        }

    @Test
    fun coversPasskeysApiKeysAccessOrgsAndTeams() =
        runBlocking<Unit> {
            val (alice, pk) = person(0, "alice-api")
            val (bob) = person(1, "bob-api")
            val bobId = bob.auth.me().accountId

            val second = SoftPasskey("localhost", Cluster.ORIGIN)
            refused<CalimeroError> { alice.addPasskey(second, second) }
            val cred = alice.addPasskey(second, pk)
            val viaSecond = client(2)
            viaSecond.signIn(second)
            assertEquals(alice.auth.me().accountId, viaSecond.auth.me().accountId)
            refused<ConfirmRequired> { alice.auth.passkeyRemove(cred = cred) }
            alice.auth.passkeyRemove(cred = cred, passkey = pk)
            refused<CalimeroError> { client(2).signIn(second) }

            refused<ConfirmRequired> { alice.auth.apikeyCreate(label = "ci") }
            val keyId = alice.auth.apikeyCreate(label = "ci", passkey = pk).keyId
            assertTrue(
                alice.auth
                    .apikeyList()
                    .propertyKeys
                    .any { it.keyId == keyId },
            )
            alice.auth.apikeyRevoke(key = keyId, passkey = pk)
            assertTrue(
                alice.auth
                    .apikeyList()
                    .propertyKeys
                    .none { it.keyId == keyId },
            )

            // Owner-only, in an organization made with it: a named account reaches it only as a
            // member of the organization, so bob joins it, and joining alone still leaves him out.
            val app = deployKv(alice, "shared", grants = emptyList())
            val shared = alice.apps.get(app = app).org!!
            bob.orgs.inviteRedeem(token = alice.orgs.inviteCreate(org = shared, role = OrgRole.MEMBER, passkey = pk).token)
            assertEquals("NOT_FOUND", refused<CalimeroError> { bob.apps.intents(app = app, method = "set", args = kv("x", "y")) }.code)
            alice.apps.access(app = app, grants = listOf(AppGrant.Account(bobId)))
            delay(300)
            bob.apps.intents(app = app, method = "set", args = kv("x", "y"))

            val org = alice.orgs.create(name = "Initech").orgId
            alice.orgs.setMember(org = org, account = bobId, role = OrgRole.MEMBER, passkey = pk)
            val team = alice.orgs.teamCreate(org = org, name = "ops", join = TeamJoin.INVITE).teamId
            assertEquals(team, alice.orgs.teamGet(org = org, team = team).teamId)
            alice.orgs.teamDelete(org = org, team = team)
            assertEquals(404, refused<CalimeroError> { alice.orgs.teamGet(org = org, team = team) }.status)

            // A team whose sub-team holds an app goes only with the owner's passkey (412 names the app).
            val eng = alice.orgs.teamCreate(org = org, name = "eng", join = TeamJoin.INVITE).teamId
            val backend = alice.orgs.teamCreate(org = org, name = "backend", join = TeamJoin.INVITE, parent = eng).teamId
            val teamApp = deployKv(alice, "eng-kv", org = org, grants = listOf(AppGrant.Team(backend)))
            val asked = refused<ConfirmRequired> { alice.orgs.teamDelete(org = org, team = eng) }
            assertEquals(412, asked.status)
            assertTrue("${asked.details}", teamApp in asked.details.toString())
            alice.orgs.teamDelete(org = org, team = eng, passkey = pk)
            delay(300)
            assertEquals(404, refused<CalimeroError> { alice.apps.get(app = teamApp) }.status)

            val token = alice.orgs.inviteCreate(org = org, role = OrgRole.GUEST, teams = emptyList(), ttlSecs = 60, uses = 1, passkey = pk).token
            assertTrue(token.isNotEmpty())
            assertEquals(org, alice.orgs.get(org = org).orgId)
        }

    @Test
    fun setsAliasesDeletesAnOrgAndSignsOutEverywhere() =
        runBlocking<Unit> {
            val (alice, pk) = person(0, "alice-aliases")
            val app = deployKv(alice, "kv-alias")
            alice.auth.aliasSet(alias = "notes", app = app)
            assertEquals(mapOf("notes" to app), alice.auth.aliasList().aliases)
            alice.auth.aliasDelete(alias = "notes")
            assertEquals(emptyMap<String, String>(), alice.auth.aliasList().aliases)

            val org = alice.orgs.create(name = "doomed").orgId
            refused<ConfirmRequired> { alice.orgs.delete(org = org) }
            alice.orgs.delete(org = org, passkey = pk)
            assertEquals(404, refused<CalimeroError> { alice.orgs.get(org = org) }.status)

            val laptop = client(1)
            laptop.signIn(pk)
            alice.signOutEverywhere()
            assertEquals(401, refused<CalimeroError> { laptop.auth.me() }.status)
        }

    @Test
    fun listsSessionsAndEndsOne() =
        runBlocking<Unit> {
            val (laptop, pk) = person(0, "alice-sessions")
            val me = laptop.auth.me()
            val phone = client(1)
            phone.signIn(pk)
            val sessions = laptop.auth.sessionsList().sessions
            assertEquals(2, sessions.size)
            val other = sessions.first { !it.current }
            laptop.auth.sessionRevoke(session = other.session)
            refused<LoginRequired> { phone.auth.me() }
            refused<NotFound> { laptop.auth.sessionRevoke(session = other.session) }
            assertEquals(me.accountId, laptop.auth.me().accountId)
        }

    @Test
    fun showsAdminsTheirMembersLockoutRisk() =
        runBlocking<Unit> {
            val (alice, alicePk) = person(0, "alice-security")
            val (bob) = person(1, "bob-security")
            val org = alice.orgs.create(name = "Lockout Co").orgId
            bob.orgs.inviteRedeem(token = alice.orgs.inviteCreate(org = org, role = OrgRole.MEMBER, passkey = alicePk).token)
            val bobId = bob.auth.me().accountId
            val members = alice.orgs.members(org = org, detail = OrganizationsApi.DetailOrgMembers.SECURITY).members
            assertEquals(2, members.size)
            assertEquals(403, refused<CalimeroError> { bob.orgs.members(org = org, detail = OrganizationsApi.DetailOrgMembers.SECURITY) }.status)
            alice.orgs.removeMember(org = org, account = bobId, passkey = alicePk)
            assertTrue(
                alice.orgs
                    .members(org = org, detail = OrganizationsApi.DetailOrgMembers.SECURITY)
                    .members
                    .none { it.account == bobId },
            )
        }

    @Test
    fun coversPresenceTransferAndBlobs() =
        runBlocking<Unit> {
            val (alice, pk) = person(0, "alice-rest")
            val (bob) = person(1, "bob-rest")
            val bobId = bob.auth.me().accountId

            val org = alice.orgs.create(name = "Rest Co").orgId
            alice.orgs.setMember(org = org, account = bobId, role = OrgRole.MEMBER, passkey = pk)
            val app = deployKv(alice, "rest-kv", org = org)
            alice.apps.intents(app = app, method = "set", args = kv("a", "1"))

            val up = alice.apps.blobUpload(app = app, body = "hello blob".toByteArray())
            assertTrue(
                alice.apps
                    .blobList(app = app)
                    .blobs
                    .any { it.id == up.id },
            )
            alice.apps.blobDelete(app = app, id = up.blobId)
            assertEquals(404, refused<CalimeroError> { alice.apps.blobInfo(app = app, id = up.blobId) }.status)

            alice.apps.presencePublish(app = app, seq = 1, state = Explicit("010203"))
            assertTrue(
                alice.apps
                    .presenceGet(app = app)
                    .present
                    .any { it.state == "010203" },
            )
            val first = withTimeout(10_000) { alice.presenceStream(app).first() }
            assertEquals("snapshot", first.kind)
            alice.apps.presencePublish(app = app, seq = 2, state = Explicit(null))

            alice.apps.transfer(app = app, to = bobId, passkey = pk)
            assertEquals(bobId, bob.apps.get(app = app).owner)
        }
}
