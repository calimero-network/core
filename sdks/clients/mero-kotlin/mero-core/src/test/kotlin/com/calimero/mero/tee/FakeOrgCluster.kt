package com.calimero.mero.tee

import com.calimero.mero.ConsentRequest
import com.calimero.mero.ConsentResult
import com.calimero.mero.app.OrgConsentApprover
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

/**
 * The organization routes the app backend uses, kept in memory for one account ([ME]), behind a
 * stand-in node (Swift `FakeCloud`). With [boundPackage] the session is bound to that package (signed
 * in on its served page), and refused with `403` what core refuses it (calimero-network/core#4934):
 * organizations, invites (redeeming included), members, capabilities, renames, a team outside the
 * package's app team, and instances of another package; each refusal is recorded in [refused].
 */
internal class FakeOrgCluster {
    class Org(
        val name: String,
        var metadata: JsonElement = JsonNull,
        val members: MutableMap<String, String> = mutableMapOf(ME to "owner"),
    )

    class Team(
        val org: String,
        val name: String,
        val join: String,
        val parent: String? = null,
        val metadata: JsonElement = JsonNull,
        val members: MutableMap<String, Boolean> = mutableMapOf(ME to true),
    )

    class App(
        val name: String,
        val org: String?,
        val team: String?,
        val pkg: String,
    )

    val orgs = mutableMapOf<String, Org>()
    val teams = mutableMapOf<String, Team>()
    val apps = mutableMapOf<String, App>()

    /** Invite token -> the teams it names. */
    val inviteTeams = mutableMapOf<String, List<String>>()

    @Volatile var boundPackage: String? = null

    /** Every request refused to the bound session, as `METHOD path`. */
    val refused: MutableList<String> = java.util.Collections.synchronizedList(mutableListOf())

    private var next = 0

    private fun id(prefix: String) = "${prefix}_${++next}"

    /** The node: answers each sealed request from the state above. */
    val node: (JsonObject, String) -> Answer = { head, text -> synchronized(this) { answer(head, text) } }

    /** An organization as the consent page makes it for [pkg]'s frontend: owned by [ME], with the package's app team. */
    fun consentOrg(
        name: String,
        pkg: String,
    ): Pair<String, String> =
        synchronized(this) {
            val org = id("org")
            val team = id("team")
            orgs[org] = Org(name)
            teams[team] = Team(org, pkg, "everyone", metadata = buildJsonObject { put("app", buildJsonObject { put("package", pkg) }) })
            org to team
        }

    /** An invite to [org] naming [teams], as the consent page's `createInvite` makes it. */
    fun consentInvite(
        org: String,
        teams: List<String>,
    ): String =
        synchronized(this) {
            "inv_$org".also { inviteTeams[it] = teams }
        }

    /** Redeems [token] as the consent page's `acceptInvite` does: [ME] joins its org and the teams it names. */
    fun consentRedeem(token: String): String =
        synchronized(this) {
            val org = token.removePrefix("inv_")
            orgs[org]?.members?.put(ME, "member")
            inviteTeams[token].orEmpty().forEach { teams[it]?.members?.put(ME, false) }
            org
        }

    private fun json(
        status: Int,
        body: String,
    ) = Answer(status, listOf("content-type" to "application/json"), listOf(body))

    private fun error(
        status: Int,
        code: String,
    ) = json(status, """{"error":"$code","message":"${code.lowercase()} (fake)","details":{}}""")

    private fun appTeam(
        org: String,
        pkg: String,
    ): String? =
        teams.entries
            .filter { (_, t) -> t.org == org && t.parent == null && ((t.metadata as? JsonObject)?.get("app") as? JsonObject)?.get("package")?.jsonPrimitive?.content == pkg }
            .minOfOrNull { it.key }

    /** Why a session bound to [pkg] is refused [method] [segments], or null. */
    private fun refusal(
        method: String,
        segments: List<String>,
        body: JsonObject,
        pkg: String,
    ): String? {
        val ids = segments[0] == "orgs" || segments[0] == "invites"
        val route = "$method " + segments.mapIndexed { i, s -> if (ids && i % 2 == 1) "*" else s }.joinToString("/")
        val holderOnly =
            setOf(
                "POST orgs",
                "POST orgs/*/invites",
                "POST invites/*/redeem",
                "POST orgs/*/members",
                "DELETE orgs/*/members/*",
                "POST orgs/*/capabilities",
                "POST orgs/*/name",
                "POST orgs/*/all-apps",
                "POST orgs/*/members/*/teams",
                "DELETE orgs/*",
            )
        if (route in holderOnly) return "a session opened on an app's frontend manages no organization"
        if (route == "POST orgs/*/teams") {
            val own = appTeam(segments[1], pkg)
            var parent = (body["parent"] as? JsonPrimitive)?.takeIf { it.isString }?.content
            while (parent != null && parent != own) parent = teams[parent]?.parent
            if (own == null || parent == null) return "a session opened on an app's frontend makes teams under its app team only"
        }
        if (route == "POST apps" && ((body["package"] as? JsonPrimitive)?.content != pkg || body["bundle"] != null)) {
            return "a session opened on an app's frontend creates instances of that app only"
        }
        return null
    }

    @Suppress("CyclomaticComplexMethod", "LongMethod", "ReturnCount")
    private fun answer(
        head: JsonObject,
        text: String,
    ): Answer {
        val method = head["method"]!!.jsonPrimitive.content
        val path = head["path"]!!.jsonPrimitive.content.substringBefore('?')
        val body = runCatching { Json.parseToJsonElement(text).jsonObject }.getOrDefault(JsonObject(emptyMap()))
        val s = path.trim('/').split('/')
        boundPackage?.let { pkg ->
            refusal(method, s, body, pkg)?.let {
                refused += "$method $path"
                return error(403, "FORBIDDEN")
            }
        }

        fun str(key: String) = (body[key] as? JsonPrimitive)?.takeIf { it.isString }?.content
        return when {
            method == "GET" && path == "/auth/me" ->
                json(
                    200,
                    buildJsonObject {
                        put("accountId", ME)
                        put(
                            "orgs",
                            JsonArray(
                                orgs.filter { ME in it.value.members }.map { (id, o) ->
                                    buildJsonObject {
                                        put("orgId", id)
                                        put("name", o.name)
                                        put("role", o.members[ME]!!)
                                    }
                                },
                            ),
                        )
                    }.toString(),
                )
            method == "POST" && path == "/orgs" -> {
                val org = id("org")
                orgs[org] = Org(str("name").orEmpty())
                json(200, """{"orgId":"$org"}""")
            }
            method == "GET" && s.size == 2 && s[0] == "orgs" -> {
                val o = orgs[s[1]] ?: return error(404, "NOT_FOUND")
                json(
                    200,
                    buildJsonObject {
                        put("orgId", s[1])
                        put("name", o.name)
                        put("metadata", o.metadata)
                        put("members", JsonObject(o.members.mapValues { JsonPrimitive(it.value) }))
                        put(
                            "teams",
                            JsonObject(
                                teams.filter { it.value.org == s[1] }.mapValues { (tid, t) ->
                                    buildJsonObject {
                                        put("teamId", tid)
                                        put("name", t.name)
                                        put("join", t.join)
                                        put("parent", t.parent?.let(::JsonPrimitive) ?: JsonNull)
                                        put("metadata", t.metadata)
                                        put("members", JsonObject(t.members.mapValues { JsonPrimitive(it.value) }))
                                    }
                                },
                            ),
                        )
                    }.toString(),
                )
            }
            method == "POST" && s.size == 3 && s[0] == "orgs" && s[2] == "metadata" -> {
                val o = orgs[s[1]] ?: return error(404, "NOT_FOUND")
                o.metadata = body["metadata"] ?: JsonNull
                json(200, "{}")
            }
            method == "POST" && s.size == 3 && s[0] == "orgs" && s[2] == "teams" -> {
                val team = id("team")
                teams[team] = Team(s[1], str("name").orEmpty(), str("join") ?: "invite", parent = str("parent"))
                json(200, """{"teamId":"$team"}""")
            }
            method == "POST" && s.size == 3 && s[0] == "orgs" && s[2] == "invites" -> {
                val token = "inv_${s[1]}"
                inviteTeams[token] = (body["teams"] as? JsonArray)?.map { it.jsonPrimitive.content }.orEmpty()
                json(200, """{"token":"$token"}""")
            }
            method == "POST" && s.size == 3 && s[0] == "invites" && s[2] == "redeem" -> json(200, """{"org":"${consentRedeem(s[1])}"}""")
            method == "GET" && s.size == 3 && s[0] == "orgs" && s[2] == "apps" ->
                json(
                    200,
                    buildJsonObject {
                        put(
                            "apps",
                            JsonArray(
                                apps.filter { it.value.org == s[1] }.map { (id, a) ->
                                    buildJsonObject {
                                        put("app", id)
                                        put("name", a.name)
                                        put("team", a.team?.let(::JsonPrimitive) ?: JsonNull)
                                        put(
                                            "package",
                                            buildJsonObject {
                                                put("package", a.pkg)
                                                put("version", "1.0.0")
                                            },
                                        )
                                    }
                                },
                            ),
                        )
                        put("next", JsonNull)
                    }.toString(),
                )
            method == "POST" && path == "/apps" -> {
                val app = id("app")
                val access = body["access"] as? JsonObject
                apps[app] = App(str("name").orEmpty(), str("org"), (access?.get("team") as? JsonPrimitive)?.content, str("package").orEmpty())
                json(200, """{"appId":"$app"}""")
            }
            else -> error(404, "NOT_FOUND")
        }
    }

    companion object {
        const val ME = "acc-1"
    }
}

/**
 * The node's consent page over [cluster]: approves, and does what the node does (an organization
 * with the package's app team, an invite naming its teams, a join into them).
 */
internal class ClusterConsent(
    private val cluster: FakeOrgCluster,
) : OrgConsentApprover {
    val kinds = mutableListOf<String>()
    val appTeams = mutableMapOf<String, String>()

    override suspend fun decide(request: (returnTo: String) -> ConsentRequest): ConsentResult {
        val made = request("https://chat.test/1.0.0/?native-consent=1")
        val outcome =
            when (made) {
                is ConsentRequest.CreateOrg -> {
                    val (org, team) = cluster.consentOrg(made.name, cluster.boundPackage.orEmpty())
                    appTeams[org] = team
                    buildJsonObject { put("orgId", org) }
                }
                is ConsentRequest.CreateInvite -> buildJsonObject { put("token", cluster.consentInvite(made.org, made.teams.orEmpty())) }
                is ConsentRequest.AcceptInvite -> buildJsonObject { put("org", cluster.consentRedeem(made.token)) }
                else -> null
            }
        kinds += made.kind
        return ConsentResult.fromJson(
            buildJsonObject {
                put("requestId", "r1")
                put("kind", made.kind)
                put("status", "approved")
                put("outcome", outcome ?: JsonNull)
            },
        )
    }
}
