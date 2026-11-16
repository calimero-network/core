package com.calimero.mero

import com.calimero.mero.api.models.ConsentPageResult200Response
import com.calimero.mero.api.models.ConsentPageResult200ResponseOutcome
import com.calimero.mero.tee.compareReleaseVersions
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import java.net.URLEncoder
import com.calimero.mero.api.models.OrgRole as ApiOrgRole
import com.calimero.mero.api.models.TeamRole as ApiTeamRole

// Approvals: the scope reports, release deadlines, publishers, and the node's consent page
// (mero-js `cloud-v2/approvals.ts`). Approvals happen only on the node's consent page:
// `POST /consent/requests` -> open the returned `url` -> `GET /consent/{id}/result`.

/** Who holds releases, app upgrades and outbound approvals on the cluster: `owner` or `operator`. */
@Serializable
data class Governance(
    val releases: String = "owner",
    val appUpgrades: String = "owner",
    val outbound: String = "owner",
    /** Who may install apps: any account ([DeploymentsRule.Open], or null) or granted deployers only. */
    val deployments: DeploymentsRule? = null,
    /** Who grants deployers: the operator ([DeployersRule.Operator], or null) or the cluster's owner. */
    val deployers: DeployersRule? = null,
)

/** Whose report: the account's own apps outside any organization (`kind = "account"`), or an organization's. */
@Serializable
data class ReportScope(
    val kind: String,
    val org: String? = null,
) {
    companion object {
        val Account: ReportScope = ReportScope("account")

        fun org(org: String): ReportScope = ReportScope("org", org)
    }
}

@Serializable
data class ReportedRelease(
    val `package`: String,
    val version: String,
    val service: String? = null,
)

@Serializable
data class ReportedBundle(
    val bundle: String,
    val `package`: String? = null,
    val version: String? = null,
)

@Serializable
data class LastUpgrade(
    val from: ReportedBundle,
    val to: ReportedBundle,
    val at: Long = 0,
    val ok: Boolean = false,
    /** `owner`, `member` or `operator`. */
    val actorKind: String = "",
    val actor: String? = null,
    val error: JsonElement = JsonNull,
    val run: String? = null,
)

@Serializable
data class ReportedReplica(
    val node: Long,
    val release: String? = null,
    val approved: Boolean = false,
)

@Serializable
data class Publisher(
    val signer: String,
)

/** One app in a scope's report. */
@Serializable
data class ReportedApp(
    val app: String,
    val group: Long = 0,
    val scope: ReportScope = ReportScope.Account,
    val bundle: String = "",
    val `package`: ReportedRelease? = null,
    val publisher: Publisher? = null,
    val upgradesHeldBy: String = "owner",
    val lastUpgrade: LastUpgrade? = null,
    val approvedReleases: List<String>? = null,
    val replicas: List<ReportedReplica> = emptyList(),
    val name: String? = null,
    val team: String? = null,
    val availableVersions: List<String>? = null,
)

@Serializable
data class ClusterRelease(
    val release: String,
    val nodes: Long = 0,
    val approved: Boolean = false,
)

/** One page of a scope's apps report (`GET /orgs/{org}/apps`, `GET /auth/apps`). */
@Serializable
data class AppsReport(
    val scope: ReportScope = ReportScope.Account,
    val governance: Governance = Governance(),
    val approvedReleases: List<String>? = null,
    val clusterReleases: List<ClusterRelease> = emptyList(),
    val apps: List<ReportedApp> = emptyList(),
    val next: String? = null,
)

@Serializable
data class OpsAppsReport(
    val fresh: Boolean = false,
    val governance: Governance = Governance(),
    val apps: List<ReportedApp> = emptyList(),
    val next: String? = null,
)

@Serializable
data class OpsOrgReport(
    val org: String,
    val approvedReleases: List<String>? = null,
)

@Serializable
data class OpsOrgsReport(
    val fresh: Boolean = false,
    val governance: Governance = Governance(),
    val orgs: List<OpsOrgReport> = emptyList(),
    val next: String? = null,
)

/** A page of a report: after the cursor a previous page gave, at most [limit] (1 to [MAX_REPORT_PAGE]). */
data class PageOptions(
    val after: String? = null,
    val limit: Int? = null,
)

const val MAX_REPORT_PAGE: Int = 100

/** The query string for [page] (`""`, or `?after=…&limit=…`). */
fun pageQuery(page: PageOptions = PageOptions()): String {
    val parts = mutableListOf<String>()
    page.after?.let { parts += "after=${URLEncoder.encode(it, "UTF-8")}" }
    page.limit?.let {
        require(it in 1..MAX_REPORT_PAGE) { "limit is 1 to $MAX_REPORT_PAGE" }
        parts += "limit=$it"
    }
    return if (parts.isEmpty()) "" else parts.joinToString("&", prefix = "?")
}

/** A release's deadline: from [after] (RFC 3339; [afterSecs] Unix seconds) a scope that has not approved it starts no new work. */
@Serializable
data class ReleaseDeadline(
    val version: String,
    val after: String = "",
    val afterSecs: Long = 0,
)

/** `GET /releases`: the cluster's governance and every release deadline. */
@Serializable
data class ReleaseRules(
    val governance: Governance = Governance(),
    val deadlines: List<ReleaseDeadline> = emptyList(),
)

/** Order two release versions (semver); anything that is not one, by its text. */
fun compareVersions(
    a: String,
    b: String,
): Int =
    try {
        compareReleaseVersions(a, b)
    } catch (_: Exception) {
        a.compareTo(b).coerceIn(-1, 1)
    }

/** The passed deadline that stops a scope approving [approved], or null when none does. */
fun stalledBy(
    approved: List<String>?,
    deadlines: List<ReleaseDeadline>,
    now: Long = System.currentTimeMillis() / 1000,
): ReleaseDeadline? {
    if (approved.isNullOrEmpty()) return null
    val newest = approved.reduce { a, b -> if (compareVersions(a, b) >= 0) a else b }
    var stall: ReleaseDeadline? = null
    for (d in deadlines) {
        if (d.afterSecs <= now && compareVersions(newest, d.version) < 0) {
            if (stall == null || compareVersions(d.version, stall.version) > 0) stall = d
        }
    }
    return stall
}

/** `GET /publishers/{signerId}`: what a key signs to bind itself next. */
@Serializable
data class PublisherBindingInfo(
    val signerId: String,
    val cluster: String = "",
    val generation: Long = 0,
    val bound: Boolean = false,
)

/** The message a package signing key signs to bind itself to [org] as its publisher. */
fun publisherBindingMessage(
    cluster: String,
    org: String,
    signerId: String,
    generation: Long,
): String = "calimero-publisher-binding/v1\n$cluster\n$org\n$signerId\n$generation"

fun publisherBindingBytes(
    cluster: String,
    org: String,
    signerId: String,
    generation: Long,
): ByteArray = publisherBindingMessage(cluster, org, signerId, generation).encodeToByteArray()

/** A key's proof for `bindPublisher`. */
@Serializable
data class PublisherProof(
    val signerId: String,
    val publicKey: String,
    val signature: String,
)

// ---------------------------------------------------------------- consent page

/** Whose approval a release or outbound request is for: the account's own (null org) or an organization's. */
data class ConsentScope(
    val org: String? = null,
) {
    internal fun toJson(): JsonElement =
        if (org == null) {
            kotlinx.serialization.json.JsonPrimitive("account")
        } else {
            buildJsonObject { put("org", org) }
        }

    companion object {
        val Account: ConsentScope = ConsentScope()
    }
}

/**
 * What the app asks its user to approve (`POST /consent/requests`): the spec's consent request body,
 * which the generated client takes as this type (`client.consent.pageCreate(request)`).
 */
@Serializable(with = ConsentRequestSerializer::class)
sealed interface ConsentRequest {
    /** Where the page sends the person back: the package's frontend origin, an app link, or one its manifest names. */
    val returnTo: String

    /** Approve (or, with [withdraw], withdraw) a mero-tee release for [scope]. */
    data class Release(
        val scope: ConsentScope,
        val version: String,
        override val returnTo: String,
        val withdraw: Boolean = false,
    ) : ConsentRequest {
        override val kind: String get() = "release"

        override fun subject(): JsonObject =
            buildJsonObject {
                put("version", version)
                if (withdraw) put("withdraw", true)
            }
    }

    /** Upgrade [app] to [toVersion] of its package. */
    data class Upgrade(
        val app: String,
        val toVersion: String,
        override val returnTo: String,
    ) : ConsentRequest {
        override val kind: String get() = "upgrade"

        override fun subject(): JsonObject =
            buildJsonObject {
                put("app", app)
                put("toVersion", toVersion)
            }
    }

    /** Approve outbound URL prefixes for [scope]; [pkg] a package-bound session's own when null, [prefixes] every one the latest release declares when null. */
    data class Outbound(
        val scope: ConsentScope,
        override val returnTo: String,
        val pkg: String? = null,
        val prefixes: List<String>? = null,
    ) : ConsentRequest {
        override val kind: String get() = "outbound"

        override fun subject(): JsonObject =
            buildJsonObject {
                pkg?.let { put("package", it) }
                prefixes?.let { put("prefixes", strings(it)) }
            }
    }

    /** Create an organization named [name], owned by the account; the outcome is [ConsentOutcome.OrgCreated]. */
    data class CreateOrg(
        val name: String,
        override val returnTo: String,
    ) : ConsentRequest {
        override val kind: String get() = "createOrg"

        override fun subject(): JsonObject =
            buildJsonObject { put("name", name) }
    }

    /**
     * Mint an invitation to [org] as [role] (guest, member or admin; never owner), on [teams], valid
     * [ttlSecs] seconds and for [uses] redemptions (the node's defaults when null), for every app of
     * the organization with [allApps]. The outcome is [ConsentOutcome.InviteCreated], which holds the
     * token: hand it on, never log it.
     */
    data class CreateInvite(
        val org: String,
        val role: ApiOrgRole,
        override val returnTo: String,
        val teams: List<String>? = null,
        val ttlSecs: Long? = null,
        val uses: Long? = null,
        val allApps: Boolean? = null,
    ) : ConsentRequest {
        override val kind: String get() = "createInvite"

        override fun subject(): JsonObject =
            buildJsonObject {
                put("org", org)
                put("role", role.value)
                teams?.let { put("teams", strings(it)) }
                ttlSecs?.let { put("ttlSecs", it) }
                uses?.let { put("uses", it) }
                allApps?.let { put("allApps", it) }
            }

        init {
            require(role != ApiOrgRole.OWNER) { "an invitation is for a guest, a member or an admin" }
            require(ttlSecs == null || ttlSecs >= 0) { "ttlSecs is not negative" }
            require(uses == null || uses >= 0) { "uses is not negative" }
        }
    }

    /**
     * Redeem the invitation [token]; the outcome is [ConsentOutcome.InviteAccepted]. [toString]
     * leaves the token out, so a logged request does not leak it.
     */
    class AcceptInvite(
        val token: String,
        override val returnTo: String,
    ) : ConsentRequest {
        override val kind: String get() = "acceptInvite"

        override fun subject(): JsonObject =
            buildJsonObject { put("token", token) }

        override fun equals(other: Any?): Boolean = other is AcceptInvite && other.token == token && other.returnTo == returnTo

        override fun hashCode(): Int = 31 * token.hashCode() + returnTo.hashCode()

        override fun toString(): String = "AcceptInvite(token=<redacted>, returnTo=$returnTo)"
    }

    /** Leave [org] (the account's own membership). */
    data class LeaveOrg(
        val org: String,
        override val returnTo: String,
    ) : ConsentRequest {
        override val kind: String get() = "leaveOrg"

        override fun subject(): JsonObject =
            buildJsonObject { put("org", org) }
    }

    /** Remove [account] from [org]. */
    data class RemoveMember(
        val org: String,
        val account: String,
        override val returnTo: String,
    ) : ConsentRequest {
        override val kind: String get() = "removeMember"

        override fun subject(): JsonObject =
            buildJsonObject {
                put("org", org)
                put("account", account)
            }
    }

    /** Make [account] a [role] of [org]; `owner` hands ownership over (the page flags it). */
    data class SetMemberRole(
        val org: String,
        val account: String,
        val role: ApiOrgRole,
        override val returnTo: String,
    ) : ConsentRequest {
        override val kind: String get() = "setMemberRole"

        override fun subject(): JsonObject =
            buildJsonObject {
                put("org", org)
                put("account", account)
                put("role", role.value)
            }
    }

    /** Add [account] to [teams] of [org], or change its role or capabilities there. */
    data class SetMemberTeams(
        val org: String,
        val account: String,
        val teams: List<ConsentTeamMembership>,
        override val returnTo: String,
    ) : ConsentRequest {
        override val kind: String get() = "setMemberTeams"

        override fun subject(): JsonObject =
            buildJsonObject {
                put("org", org)
                put("account", account)
                put("teams", JsonArray(teams.map { it.toJson() }))
            }
    }

    /** Take [account] out of [teams] of [org]. */
    data class RemoveMemberTeams(
        val org: String,
        val account: String,
        val teams: List<String>,
        override val returnTo: String,
    ) : ConsentRequest {
        override val kind: String get() = "removeMemberTeams"

        override fun subject(): JsonObject =
            buildJsonObject {
                put("org", org)
                put("account", account)
                put("teams", strings(teams))
            }
    }

    /** Revoke the invitation [inviteKey] of [org] (its key, not its token). */
    data class RevokeInvite(
        val org: String,
        val inviteKey: String,
        override val returnTo: String,
    ) : ConsentRequest {
        override val kind: String get() = "revokeInvite"

        override fun subject(): JsonObject =
            buildJsonObject {
                put("org", org)
                put("inviteKey", inviteKey)
            }
    }

    /** Rename [org] to [name]; the outcome is [ConsentOutcome.OrgRenamed]. */
    data class RenameOrg(
        val org: String,
        val name: String,
        override val returnTo: String,
    ) : ConsentRequest {
        override val kind: String get() = "renameOrg"

        override fun subject(): JsonObject =
            buildJsonObject {
                put("org", org)
                put("name", name)
            }
    }

    /** Delete [org] and what it holds; the outcome is [ConsentOutcome.OrgDeleted]. */
    data class DeleteOrg(
        val org: String,
        override val returnTo: String,
    ) : ConsentRequest {
        override val kind: String get() = "deleteOrg"

        override fun subject(): JsonObject =
            buildJsonObject { put("org", org) }
    }

    /**
     * Set [capabilities] ([OrgCapabilities] bits): [account]'s in [org] (on [team] when given), or
     * without [account] the default of [team] or of the organization.
     */
    data class SetCapabilities(
        val org: String,
        val capabilities: Long,
        override val returnTo: String,
        val account: String? = null,
        val team: String? = null,
    ) : ConsentRequest {
        override val kind: String get() = "setCapabilities"

        override fun subject(): JsonObject =
            buildJsonObject {
                put("org", org)
                account?.let { put("account", it) }
                team?.let { put("team", it) }
                put("capabilities", capabilities)
            }

        init {
            require(capabilities >= 0) { "capabilities is not negative" }
        }
    }

    /** Give [account] every app of [org] ([allApps]), or only those of its teams. */
    data class SetAllApps(
        val org: String,
        val account: String,
        val allApps: Boolean,
        override val returnTo: String,
    ) : ConsentRequest {
        override val kind: String get() = "setAllApps"

        override fun subject(): JsonObject =
            buildJsonObject {
                put("org", org)
                put("account", account)
                put("allApps", allApps)
            }
    }

    /** The request's `kind` on the wire. */
    val kind: String

    /** The request's `subject` on the wire. */
    fun subject(): JsonObject

    /** The wire body: `kind`, the `scope` of a release or outbound request, `subject` and `returnTo`. */
    fun toJson(): JsonObject =
        buildJsonObject {
            put("kind", kind)
            when (this@ConsentRequest) {
                is Release -> put("scope", scope.toJson())
                is Outbound -> put("scope", scope.toJson())
                else -> Unit
            }
            put("subject", subject())
            put("returnTo", returnTo)
        }
}

/** One team of [ConsentRequest.SetMemberTeams]: [team], with the member's [role] and [capabilities] there (the node's defaults when null). */
data class ConsentTeamMembership(
    val team: String,
    val role: ApiTeamRole? = null,
    val capabilities: Long? = null,
) {
    init {
        require(capabilities == null || capabilities >= 0) { "capabilities is not negative" }
    }

    internal fun toJson(): JsonObject =
        buildJsonObject {
            put("team", team)
            role?.let { put("role", it.value) }
            capabilities?.let { put("capabilities", it) }
        }
}

/** The node's answer to a consent request: send the person to [url]. */
@Serializable
data class ConsentStarted(
    val requestId: String,
    /** The consent page, on the node's own origin. */
    val url: String,
    /** Unix seconds; a request lives 10 minutes. */
    val expiresAt: Long = 0,
    /**
     * The request's confirmation code (`ABC-123`): the consent page shows the same one, so show it
     * to the person before they open the page, and they can check they approve what the app asked.
     * `""` from a node that predates it.
     */
    val code: String = "",
)

/** How [CalimeroClient.openConsentPage] sends the person to the page (mero-js `OpenConsentOptions`). */
class OpenConsentOptions(
    /** Opens the page's `url` (a Custom Tab: mero-compose's `ConsentLauncher`). */
    val navigate: (String) -> Unit,
    /**
     * Runs after the node answers and before navigating, with the answer: show the person
     * [ConsentStarted.code] here so they can check the page shows the same. Answer `false` and the
     * page is not opened (the request stays on the node until it expires).
     */
    val confirm: (suspend (ConsentStarted) -> Boolean)? = null,
)

private const val CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"

/**
 * The confirmation code the node's consent page shows for [requestId]: the first 30 bits of
 * SHA-256 of its UTF-8 bytes, big-endian, as six Crockford base32 characters written `XXX-XXX`
 * (core's consent page). Compare it with [ConsentStarted.code] to check the node's answer.
 */
fun confirmationCode(requestId: String): String {
    val h =
        com.calimero.mero.crypto
            .sha256(requestId.encodeToByteArray())
    val word = ((h[0].toLong() and 0xff) shl 24) or ((h[1].toLong() and 0xff) shl 16) or ((h[2].toLong() and 0xff) shl 8) or (h[3].toLong() and 0xff)
    val code = StringBuilder()
    for (i in 0 until 6) {
        if (i == 3) code.append('-')
        code.append(CROCKFORD[((word ushr (27 - 5 * i)) and 31).toInt()])
    }
    return code.toString()
}

/**
 * What an approved consent request did, by its kind (`GET /consent/{id}/result`'s `outcome`). A
 * release, upgrade or outbound approval carries nothing beyond the approval itself.
 */
sealed interface ConsentOutcome {
    /** The request's `kind` on the wire. */
    val kind: String

    data object Release : ConsentOutcome {
        override val kind: String = "release"
    }

    data object Upgrade : ConsentOutcome {
        override val kind: String = "upgrade"
    }

    data object Outbound : ConsentOutcome {
        override val kind: String = "outbound"
    }

    /** [ConsentRequest.CreateOrg]: the organization it created. */
    data class OrgCreated(
        val orgId: String,
    ) : ConsentOutcome {
        override val kind: String = "createOrg"
    }

    /**
     * [ConsentRequest.CreateInvite]: the invitation's [token], a bearer secret. Hand it to whoever
     * is invited (an invitation link); never log it. [toString] leaves it out.
     */
    class InviteCreated(
        val token: String,
    ) : ConsentOutcome {
        override val kind: String = "createInvite"

        override fun equals(other: Any?): Boolean = other is InviteCreated && other.token == token

        override fun hashCode(): Int = token.hashCode()

        override fun toString(): String = "InviteCreated(token=<redacted>)"
    }

    /** [ConsentRequest.AcceptInvite]: the organization the account joined. */
    data class InviteAccepted(
        val org: String,
    ) : ConsentOutcome {
        override val kind: String = "acceptInvite"
    }

    data object LeftOrg : ConsentOutcome {
        override val kind: String = "leaveOrg"
    }

    data object MemberRemoved : ConsentOutcome {
        override val kind: String = "removeMember"
    }

    data object MemberRoleSet : ConsentOutcome {
        override val kind: String = "setMemberRole"
    }

    data object MemberTeamsSet : ConsentOutcome {
        override val kind: String = "setMemberTeams"
    }

    data object MemberTeamsRemoved : ConsentOutcome {
        override val kind: String = "removeMemberTeams"
    }

    data object InviteRevoked : ConsentOutcome {
        override val kind: String = "revokeInvite"
    }

    data object CapabilitiesSet : ConsentOutcome {
        override val kind: String = "setCapabilities"
    }

    data object AllAppsSet : ConsentOutcome {
        override val kind: String = "setAllApps"
    }

    /** [ConsentRequest.RenameOrg]: the organization and its new name. */
    data class OrgRenamed(
        val orgId: String,
        val name: String,
    ) : ConsentOutcome {
        override val kind: String = "renameOrg"
    }

    /** [ConsentRequest.DeleteOrg]: the apps it deleted with the organization. */
    data class OrgDeleted(
        val apps: List<String>,
    ) : ConsentOutcome {
        override val kind: String = "deleteOrg"
    }

    /** A kind this SDK does not know (a newer node), or a known kind whose `outcome` lacks its field. */
    data class Other(
        override val kind: String,
        /** The `outcome` object as the node sent it, or null. Keys and values as sent: may hold a secret. */
        val raw: JsonObject?,
    ) : ConsentOutcome {
        override fun toString(): String = "Other(kind=$kind, keys=${raw?.keys})"
    }

    companion object {
        /** The outcome of an approved request of [kind] from the node's `outcome` object. */
        fun of(
            kind: String,
            outcome: JsonObject?,
        ): ConsentOutcome {
            fun field(key: String): String? = (outcome?.get(key) as? kotlinx.serialization.json.JsonPrimitive)?.takeIf { it.isString }?.content
            return when (kind) {
                "release" -> Release
                "upgrade" -> Upgrade
                "outbound" -> Outbound
                "createOrg" -> field("orgId")?.let(::OrgCreated)
                "createInvite" -> field("token")?.let(::InviteCreated)
                "acceptInvite" -> field("org")?.let(::InviteAccepted)
                "leaveOrg" -> LeftOrg
                "removeMember" -> MemberRemoved
                "setMemberRole" -> MemberRoleSet
                "setMemberTeams" -> MemberTeamsSet
                "removeMemberTeams" -> MemberTeamsRemoved
                "revokeInvite" -> InviteRevoked
                "setCapabilities" -> CapabilitiesSet
                "setAllApps" -> AllAppsSet
                "renameOrg" -> {
                    val orgId = field("orgId")
                    val name = field("name")
                    if (orgId != null && name != null) OrgRenamed(orgId, name) else null
                }
                "deleteOrg" -> strings(outcome?.get("apps"))?.let(::OrgDeleted)
                else -> null
            } ?: Other(kind, outcome)
        }
    }
}

/** `GET /consent/{id}/result`. [status] is `pending`, `approved` or `declined`. */
@Serializable(with = ConsentResultSerializer::class)
data class ConsentResult(
    val requestId: String,
    val kind: String = "",
    val status: String = "pending",
    /** Unix seconds, once decided. */
    val decidedAt: Long? = null,
    /** What the approval did ([ConsentOutcome]); null unless [isApproved]. */
    val outcome: ConsentOutcome? = null,
) {
    val isPending: Boolean get() = status == "pending"
    val isApproved: Boolean get() = status == "approved"
    val isDeclined: Boolean get() = status == "declined"

    companion object {
        /** The facade's answer (`client.consent.pageResult(request = id)`) as a [ConsentResult], its generated `outcome` typed by kind. */
        fun from(answer: ConsentPageResult200Response): ConsentResult =
            ConsentResult(
                answer.requestId,
                answer.kind.value,
                answer.status.value,
                answer.decidedAt,
                if (answer.status == ConsentPageResult200Response.Status.APPROVED) ConsentOutcome.of(answer.kind.value, answer.outcome?.toJson()) else null,
            )

        /** The generated `outcome`'s fields that are present, as the node sent them. */
        private fun ConsentPageResult200ResponseOutcome.toJson(): JsonObject =
            buildJsonObject {
                orgId?.let { put("orgId", it) }
                token?.let { put("token", it) }
                org?.let { put("org", it) }
                name?.let { put("name", it) }
                apps?.let { put("apps", strings(it)) }
            }

        /** A result from the node's JSON answer: [outcome] typed by `kind` when approved. */
        fun fromJson(o: JsonObject): ConsentResult {
            fun str(key: String) = (o[key] as? kotlinx.serialization.json.JsonPrimitive)?.takeIf { it !is JsonNull }?.content
            val kind = str("kind").orEmpty()
            val status = str("status") ?: "pending"
            return ConsentResult(
                requestId = str("requestId").orEmpty(),
                kind = kind,
                status = status,
                decidedAt = str("decidedAt")?.toLongOrNull(),
                outcome = if (status == "approved") ConsentOutcome.of(kind, o["outcome"] as? JsonObject) else null,
            )
        }
    }
}

/** [values] as a JSON array of strings. */
private fun strings(values: List<String>): JsonArray = JsonArray(values.map { JsonPrimitive(it) })

/** Every element of [element] when it is an array of strings, else null. */
private fun strings(element: JsonElement?): List<String>? {
    val array = element as? JsonArray ?: return null
    val out = array.map { (it as? JsonPrimitive)?.takeIf { p -> p.isString }?.content }
    return if (out.any { it == null }) null else out.filterNotNull()
}

/** Reads [ConsentResult.fromJson]; writes the wire shape back (`outcome` with its fields). */
internal object ConsentResultSerializer : kotlinx.serialization.KSerializer<ConsentResult> {
    override val descriptor: kotlinx.serialization.descriptors.SerialDescriptor = JsonObject.serializer().descriptor

    override fun serialize(
        encoder: kotlinx.serialization.encoding.Encoder,
        value: ConsentResult,
    ) {
        val json =
            buildJsonObject {
                put("requestId", value.requestId)
                put("kind", value.kind)
                put("status", value.status)
                if (value.decidedAt == null) put("decidedAt", JsonNull) else put("decidedAt", value.decidedAt)
                when (val o = value.outcome) {
                    is ConsentOutcome.OrgCreated -> put("outcome", buildJsonObject { put("orgId", o.orgId) })
                    is ConsentOutcome.InviteCreated -> put("outcome", buildJsonObject { put("token", o.token) })
                    is ConsentOutcome.InviteAccepted -> put("outcome", buildJsonObject { put("org", o.org) })
                    is ConsentOutcome.OrgRenamed ->
                        put(
                            "outcome",
                            buildJsonObject {
                                put("orgId", o.orgId)
                                put("name", o.name)
                            },
                        )
                    is ConsentOutcome.OrgDeleted -> put("outcome", buildJsonObject { put("apps", JsonArray(o.apps.map { JsonPrimitive(it) })) })
                    is ConsentOutcome.Other -> o.raw?.let { put("outcome", it) }
                    else -> Unit
                }
            }
        (encoder as kotlinx.serialization.json.JsonEncoder).encodeJsonElement(json)
    }

    override fun deserialize(decoder: kotlinx.serialization.encoding.Decoder): ConsentResult {
        val element = (decoder as kotlinx.serialization.json.JsonDecoder).decodeJsonElement()
        return ConsentResult.fromJson(element as? JsonObject ?: throw kotlinx.serialization.SerializationException("a consent result is an object"))
    }
}

/** What the consent page put on `returnTo` when it sent the person back. */
data class ConsentReturn(
    val requestId: String,
    /** `approved` or `declined`. */
    val result: String,
)

/**
 * What the consent page put on `returnTo` when it sent the person back, or null when [href]
 * carries none. A hint only (anyone can type a URL): confirm with
 * [CalimeroClient.consentResult] before acting on it.
 */
fun consentReturn(href: String): ConsentReturn? {
    val url = href.toHttpUrlOrNull() ?: href.replaceBefore("://", "https").toHttpUrlOrNull() ?: return null
    val requestId = url.queryParameter("request_id")?.takeIf { it.isNotEmpty() } ?: return null
    val result = url.queryParameter("result")?.takeIf { it == "approved" || it == "declined" } ?: return null
    return ConsentReturn(requestId, result)
}

/**
 * A native app's consent `returnTo` on Calimero Cloud: the package's served page
 * (`{origin}/{version}/?native-consent=1`), which passes the answer on to the app
 * (`callback?request_id=…&result=…`). The node takes only the package's own https origin (or one its
 * signed manifest names) as `returnTo`, never an app scheme. On a development node, use the app's
 * callback as it is. (swift-sdk `ServedPageHandoff.consentReturnTo`.)
 */
fun consentReturnTo(
    origin: String,
    version: String,
): String {
    require(version.isNotEmpty() && '/' !in version && '?' !in version && '#' !in version) { "a package version" }
    return "${origin.trimEnd('/')}/$version/?native-consent=1"
}

/** The public verifier for mero-tee releases (`attestation-verifier`). */
const val RELEASE_VERIFIER: String = "https://mero-tee.vercel.app"

/** A mero-tee release's notes on GitHub. */
fun releaseNotesUrl(version: String): String =
    "https://github.com/calimero-network/mero-tee/releases/tag/mero-tee-v${URLEncoder.encode(version, "UTF-8")}"

/** The public verifier's page for a mero-tee release. */
fun verifyReleaseUrl(version: String): String = "$RELEASE_VERIFIER/mero-tee?release_tag=${URLEncoder.encode("mero-tee-v$version", "UTF-8")}"

/** A mero-tee release the cluster's nodes run that the scope has not approved. */
data class PendingRelease(
    val version: String,
    /** How many of the cluster's nodes run it. */
    val nodes: Long,
    /** Whether the scope's side holds release approvals (else the operator does). */
    val actionable: Boolean,
    val releaseNotesUrl: String,
    val verifyUrl: String,
    val deadline: ReleaseDeadline?,
)

/** An app with newer releases of its package. */
data class PendingUpgrade(
    val app: String,
    val name: String?,
    val `package`: String,
    val from: String,
    /** The newest it may move to. */
    val to: String,
    /** Every newer release it may move to, oldest first. */
    val available: List<String>,
    val actionable: Boolean,
)

/** What [CalimeroClient.pendingApprovals] answers. */
data class PendingApprovals(
    val scope: ReportScope,
    val governance: Governance,
    val approvedReleases: List<String>?,
    val releases: List<PendingRelease>,
    val upgrades: List<PendingUpgrade>,
    /** A passed deadline has stopped new work for the scope: approve its version (or newer) to lift it. */
    val stalledBy: ReleaseDeadline?,
    /** Whether anything here is the user's to approve. */
    val actionable: Boolean,
)

/** The pending list from a scope's report pages (every page's apps in `report.apps`) and the cluster's release rules. */
fun pendingApprovalsFrom(
    report: AppsReport,
    rules: ReleaseRules,
    now: Long = System.currentTimeMillis() / 1000,
): PendingApprovals {
    val releasesActionable = report.governance.releases == "owner"
    val upgradesActionable = report.governance.appUpgrades == "owner"
    val releases =
        report.clusterReleases
            .filter { !it.approved }
            .sortedWith { a, b -> compareVersions(b.release, a.release) }
            .map { r ->
                PendingRelease(
                    r.release,
                    r.nodes,
                    releasesActionable,
                    releaseNotesUrl(r.release),
                    verifyReleaseUrl(r.release),
                    rules.deadlines.firstOrNull { it.version == r.release },
                )
            }
    val upgrades =
        report.apps.mapNotNull { a ->
            val available = a.availableVersions.orEmpty()
            val pkg = a.`package` ?: return@mapNotNull null
            if (available.isEmpty()) return@mapNotNull null
            PendingUpgrade(a.app, a.name, pkg.`package`, pkg.version, available.last(), available, upgradesActionable)
        }
    return PendingApprovals(
        report.scope,
        report.governance,
        report.approvedReleases,
        releases,
        upgrades,
        stalledBy(report.approvedReleases, rules.deadlines, now),
        releases.any { it.actionable } || upgrades.any { it.actionable },
    )
}

/** The consent request that approves [version] for [scope], or with [withdraw] withdraws its approval. */
fun releaseConsent(
    scope: ReportScope,
    version: String,
    returnTo: String,
    withdraw: Boolean = false,
): ConsentRequest = ConsentRequest.Release(if (scope.kind == "org") ConsentScope(scope.org) else ConsentScope.Account, version, returnTo, withdraw)

/** The consent request that withdraws [version]'s approval for [scope]. */
fun withdrawConsent(
    scope: ReportScope,
    version: String,
    returnTo: String,
): ConsentRequest = releaseConsent(scope, version, returnTo, withdraw = true)

/** The consent request that upgrades [app] to [toVersion]. */
fun upgradeConsent(
    app: String,
    toVersion: String,
    returnTo: String,
): ConsentRequest = ConsentRequest.Upgrade(app, toVersion, returnTo)

/**
 * What a member may do in an organization beyond their role, as core's `caps` bits
 * (`setCapabilities`, `setTeamMember`'s capabilities). A team's lead and an organization's admin
 * hold them all. (mero-js `ORG_CAPABILITIES`.)
 */
object OrgCapabilities {
    /** Create an app in the organization (on its team, or org-wide). */
    const val CREATE_APP: Int = 1

    /** Mint invites (org-wide, or on each team the invite names). */
    const val INVITE_MEMBERS: Int = 1 shl 1

    /** Inherited into open teams; denying it keeps a member out of them. */
    const val JOIN_OPEN_TEAMS: Int = 1 shl 2

    /** Add and remove a team's members. */
    const val MANAGE_MEMBERS: Int = 1 shl 3

    /** Upgrade the organization's apps, start org upgrade runs, approve outbound hosts. */
    const val MANAGE_APPLICATION: Int = 1 shl 4

    /** Create a top-level team without being an admin. */
    const val CREATE_TEAM: Int = 1 shl 5

    /** Delete a team, and what it holds, without being an admin. */
    const val DELETE_TEAM: Int = 1 shl 6

    /** Change who may join a team. */
    const val MANAGE_VISIBILITY: Int = 1 shl 7

    /** Set the organization's and its teams' metadata. */
    const val MANAGE_METADATA: Int = 1 shl 8

    /** Approve (or withdraw) the mero-tee releases that hold the organization's apps; bind a publisher key. Bit 9 is unused. */
    const val APPROVE_RELEASES: Int = 1 shl 10

    /** What a member holds unless an admin says otherwise (mero-js `DEFAULT_ORG_CAPABILITIES`). */
    const val DEFAULT: Int = JOIN_OPEN_TEAMS

    /** Every capability by its mero-js name. */
    val ALL: Map<String, Int> =
        mapOf(
            "CREATE_APP" to CREATE_APP,
            "INVITE_MEMBERS" to INVITE_MEMBERS,
            "JOIN_OPEN_TEAMS" to JOIN_OPEN_TEAMS,
            "MANAGE_MEMBERS" to MANAGE_MEMBERS,
            "MANAGE_APPLICATION" to MANAGE_APPLICATION,
            "CREATE_TEAM" to CREATE_TEAM,
            "DELETE_TEAM" to DELETE_TEAM,
            "MANAGE_VISIBILITY" to MANAGE_VISIBILITY,
            "MANAGE_METADATA" to MANAGE_METADATA,
            "APPROVE_RELEASES" to APPROVE_RELEASES,
        )

    /** The bitmask of [names] (mero-js `orgCapabilities`). */
    fun of(vararg names: String): Int = names.fold(0) { mask, name -> mask or (ALL[name] ?: throw IllegalArgumentException("unknown capability $name")) }
}

/** Writes a [ConsentRequest] as [ConsentRequest.toJson]; one is never read back. */
internal object ConsentRequestSerializer : kotlinx.serialization.KSerializer<ConsentRequest> {
    override val descriptor: kotlinx.serialization.descriptors.SerialDescriptor = JsonObject.serializer().descriptor

    override fun serialize(
        encoder: kotlinx.serialization.encoding.Encoder,
        value: ConsentRequest,
    ) {
        (encoder as kotlinx.serialization.json.JsonEncoder).encodeJsonElement(value.toJson())
    }

    override fun deserialize(decoder: kotlinx.serialization.encoding.Decoder): ConsentRequest =
        throw kotlinx.serialization.SerializationException("a consent request is only ever sent")
}
