package com.calimero.mero.app

import com.calimero.mero.AppAccess
import com.calimero.mero.AppSource
import com.calimero.mero.CalimeroClient
import com.calimero.mero.CalimeroError
import com.calimero.mero.ConsentOutcome
import com.calimero.mero.ConsentRequest
import com.calimero.mero.ConsentResult
import com.calimero.mero.NotFound
import com.calimero.mero.OrgInfo
import com.calimero.mero.PasskeyProvider
import com.calimero.mero.api.models.OrgRole
import com.calimero.mero.api.models.TeamJoin
import com.calimero.mero.api.models.TeamRole
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put

/**
 * The real [MeroAppBackend]: an app's screens over a signed-in [CalimeroClient], for the app
 * published as [packageName]. (Swift `CalimeroAppBackend`; replaces 0.x's `DelegatedAppBackend`.)
 *
 * Namespaces are organizations, subgroups are teams (`"{orgId}/{teamId}"`), contexts are app
 * instances of [packageName]; see [MeroAppBackend] for the whole mapping. Names live where 1.0
 * keeps them: a team's and an organization's `name`, an app instance's `name`, overridden by a
 * `name` in its metadata once renamed; a member's display name is `name` in their member metadata.
 *
 * Member changes, leaving and invitations are passkey step-ups on 1.0 (`412 CONFIRM_REQUIRED`):
 * they are approved with the passkey [passkey] answers when one is needed (mero-compose's
 * `MeroAppSession.passkey`, a `PlatformPasskey` over the app's Activity). While it answers null they
 * throw [PasskeyRequired] and send nothing; a dismissed passkey sheet throws
 * [com.calimero.mero.ConfirmCancelled].
 *
 * With [consent] (a session bound to the package, which the node refuses organization routes),
 * [foundNamespace], [createInvitation] and [joinNamespace] are approved on the node's consent page
 * instead (`createOrg`, `createInvite`, `acceptInvite`), with the same answers. Declining throws
 * [OrgConsentNotApproved], and nothing is changed.
 *
 * The package's app team ([OrgInfo.appTeam], which an organization made on the consent page starts
 * with) is the organization's own place for the app: open instances reach it, invites name it, and
 * teams are made under it (a package-bound session makes teams there only).
 */
@Suppress("TooManyFunctions")
class CalimeroAppBackend(
    val client: CalimeroClient,
    /** The app's package (`com.calimero.chat`): what app instances are created from and listed by. */
    val packageName: String,
    /** The signed-in account's id. */
    override val account: String,
    /** The passkey that approves step-ups, asked for each time one is needed; null: none to hand. */
    val passkey: () -> PasskeyProvider?,
    /**
     * Where organization actions are approved for a session bound to the package
     * ([servedPageConsent]); null: the node is called directly.
     */
    val consent: OrgConsentApprover?,
) : MeroAppBackend {
    constructor(
        client: CalimeroClient,
        packageName: String,
        account: String,
        passkey: () -> PasskeyProvider? = { null },
    ) : this(client, packageName, account, passkey, null)

    override val contract: ContextContract = CloudContextContract(client)
    override val hasRelay: Boolean = true
    override val canSubscribe: Boolean = true

    override suspend fun listNamespaces(): List<AppNamespace> =
        client.me().orgs.map { org ->
            val members = runCatching { client.getOrg(org.orgId).members.size }.getOrDefault(0)
            AppNamespace(org.orgId, org.name.ifEmpty { null }, packageName, members)
        }

    override suspend fun listSubgroups(namespaceId: String): List<AppSubgroup> {
        val org = client.getOrg(namespaceId)
        val apps = appsByTeam(namespaceId)
        // The package's app team is the org's own place for the app (listContextEntries lists its instances with the org's).
        val own = org.appTeam(packageName)
        return org.teams.values.filter { it.teamId != own }.map { team ->
            AppSubgroup(teamGroupId(namespaceId, team.teamId), team.name.ifEmpty { null }, apps[team.teamId].orEmpty().map { it.app }, team.join == TeamJoin.EVERYONE.value)
        }
    }

    override suspend fun listContexts(groupId: String): List<String> = listContextEntries(groupId).map { it.id }

    override suspend fun listContextEntries(groupId: String): List<AppContextEntry> {
        val ref = GroupRef.parse(groupId)
        val byTeam = appsByTeam(ref.org)
        if (ref.team != null) return byTeam[ref.team].orEmpty().map { AppContextEntry(it.app, it.name) }
        // The org's own: its apps that no team holds, and those of the package's app team.
        val own = client.getOrg(ref.org).appTeam(packageName)
        return (byTeam[null].orEmpty() + own?.let { byTeam[it] }.orEmpty()).map { AppContextEntry(it.app, it.name) }
    }

    override suspend fun isMember(contextId: String): Boolean =
        try {
            client.getApp(contextId)
            true
        } catch (_: NotFound) {
            false
        }

    override suspend fun joinContext(contextId: String) {
        client.getApp(contextId)
    }

    override suspend fun foundNamespace(name: String): AppFoundedNamespace {
        val consent = consent ?: return AppFoundedNamespace(client.orgs.create(name = name).orgId)
        val result = approved(consent.decide { ConsentRequest.CreateOrg(name, it) })
        val org = (result.outcome as? ConsentOutcome.OrgCreated)?.orgId ?: throw noOutcome(result)
        // A session bound to the package may be refused the metadata write; the org is then
        // listed by the app instances it runs.
        val named =
            try {
                setMetadata(org, name)
                true
            } catch (e: CalimeroError) {
                false
            }
        return AppFoundedNamespace(org, nameError = if (named) null else "the workspace name could not be saved")
    }

    private suspend fun setMetadata(
        org: String,
        name: String,
    ) {
        val metadata =
            buildJsonObject {
                put("name", name)
                if (packageName.isNotEmpty()) put("package", packageName)
            }
        client.orgs.metadata(org = org, metadata = metadata)
    }

    override suspend fun createSubgroup(
        namespaceId: String,
        name: String,
        isPublic: Boolean,
    ): String {
        // Under the package's app team when the org has one: a session bound to the package makes teams there only.
        val parent = client.getOrg(namespaceId).appTeam(packageName)
        val join = if (isPublic) TeamJoin.EVERYONE else TeamJoin.INVITE
        return teamGroupId(namespaceId, client.orgs.teamCreate(org = namespaceId, name = name, join = join, parent = parent).teamId)
    }

    override suspend fun createContext(
        groupId: String,
        applicationId: String,
        name: String,
        initArgs: JsonElement,
    ): String {
        val ref = GroupRef.parse(groupId)
        val pkg = applicationId.substringBefore('#').ifEmpty { packageName }
        val service = applicationId.substringAfter('#', "").ifEmpty { null }
        // The org's own instance reaches the package's app team when the org has one, else the whole org.
        val team = ref.team ?: client.getOrg(ref.org).appTeam(packageName)
        return client.createApp(
            name,
            AppSource.Package(pkg, service = service),
            org = ref.org,
            access = team?.let { AppAccess.Team(it) } ?: AppAccess.Org,
            initArgs = initArgs,
        )
    }

    override suspend fun createInvitation(namespaceId: String): OrgInvitation {
        val org =
            try {
                client.getOrg(namespaceId)
            } catch (_: CalimeroError) {
                null
            }
        // Naming the package's app team when the org has one: an invitee outside it has no way into the app.
        val teams = listOfNotNull(org?.appTeam(packageName))
        val consent = consent
        val token =
            if (consent != null) {
                val result = approved(consent.decide { ConsentRequest.CreateInvite(namespaceId, OrgRole.MEMBER, it, teams = teams) })
                (result.outcome as? ConsentOutcome.InviteCreated)?.token ?: throw noOutcome(result)
            } else {
                client.orgs.inviteCreate(org = namespaceId, role = OrgRole.MEMBER, teams = teams, passkey = approver("createInvitation")).token
            }
        return OrgInvitation(namespaceId, token, name = org?.name)
    }

    override suspend fun joinNamespace(
        namespaceId: String,
        invitation: OrgInvitation,
    ): AppJoinedNamespace {
        val consent = consent
        val joined =
            if (consent != null) {
                val result = approved(consent.decide { ConsentRequest.AcceptInvite(invitation.token, it) })
                (result.outcome as? ConsentOutcome.InviteAccepted)?.org ?: throw noOutcome(result)
            } else {
                client.orgs.inviteRedeem(token = invitation.token).org
            }
        val org = joined.ifEmpty { invitation.namespaceId }
        return AppJoinedNamespace(org, published = true, relayUrl = client.nodeUrl)
    }

    override fun events(contextId: String): Flow<AppContextEvent> = client.events(contextId).map { AppContextEvent(contextId, it.kind, it.data, it.id) }

    // ---- names ---------------------------------------------------------------------------

    override suspend fun groupMetadata(groupId: String): AppMetadata? {
        val ref = GroupRef.parse(groupId)
        val org = client.getOrg(ref.org)
        return if (ref.team == null) {
            metadataOf(org.name, org.metadata)
        } else {
            org.teams[ref.team]?.let { metadataOf(it.name, it.metadata) }
        }
    }

    override suspend fun contextMetadata(
        groupId: String,
        contextId: String,
    ): AppMetadata? {
        val app = client.getApp(contextId)
        return metadataOf((app["name"] as? JsonPrimitive)?.contentOrNull.orEmpty(), app["metadata"] ?: JsonNull)
    }

    override suspend fun setGroupName(
        groupId: String,
        name: String,
    ) {
        val ref = GroupRef.parse(groupId)
        if (ref.team != null) {
            client.orgs.teamUpdate(org = ref.org, team = ref.team, name = name)
        } else {
            client.orgs.metadata(org = ref.org, metadata = withName(client.getOrg(ref.org).metadata, name))
        }
    }

    override suspend fun setContextName(
        groupId: String,
        contextId: String,
        name: String,
    ) {
        client.apps.metadata(app = contextId, metadata = withName(client.getApp(contextId)["metadata"] ?: JsonNull, name))
    }

    // ---- blobs ---------------------------------------------------------------------------

    override suspend fun uploadBlob(
        contextId: String,
        data: ByteArray,
    ): AppBlob = client.uploadBlob(contextId, data).let { AppBlob(it.blobId, it.size) }

    override suspend fun getBlob(
        blobId: String,
        contextId: String,
    ): ByteArray = client.apps.blobDownload(app = contextId, id = blobId)

    // ---- leaving and deleting ------------------------------------------------------------

    override suspend fun leaveNamespace(namespaceId: String) {
        client.orgs.removeMember(org = namespaceId, account = account, passkey = approver("leaveNamespace"))
    }

    override suspend fun leaveSubgroup(groupId: String) {
        val ref = GroupRef.parse(groupId)
        val team = ref.team ?: return leaveNamespace(ref.org)
        client.orgs.teamRemoveMember(org = ref.org, team = team, account = account, passkey = approver("leaveSubgroup"))
    }

    override suspend fun deleteSubgroup(groupId: String) {
        val ref = GroupRef.parse(groupId)
        client.orgs.teamDelete(org = ref.org, team = ref.team ?: throw IllegalArgumentException("$groupId is an organization, not a team"))
    }

    override suspend fun removeContext(
        groupId: String,
        contextId: String,
    ) {
        client.apps.delete(app = contextId)
    }

    // ---- members -------------------------------------------------------------------------

    override suspend fun listMembers(groupId: String): List<AppMember> {
        val ref = GroupRef.parse(groupId)
        val org = client.getOrg(ref.org)
        val names = org.memberMetadata.mapValues { (_, v) -> ((v as? JsonObject)?.get("name") as? JsonPrimitive)?.contentOrNull }
        if (ref.team == null) {
            return org.members.map { (account, role) ->
                AppMember(
                    account,
                    if (role == "admin" || role == "owner") {
                        "Admin"
                    } else if (role == "guest") {
                        "ReadOnly"
                    } else {
                        "Member"
                    },
                    names[account],
                )
            }
        }
        val team = org.teams[ref.team] ?: return emptyList()
        val accounts = if (team.join == TeamJoin.EVERYONE.value) org.members.keys + team.members.keys else team.members.keys
        return accounts.map { account ->
            val role =
                when {
                    team.members[account] == true -> "Admin"
                    account in team.readOnly -> "ReadOnly"
                    else -> "Member"
                }
            AppMember(account, role, names[account])
        }
    }

    override suspend fun setMemberRole(
        groupId: String,
        identity: String,
        role: GovernanceMemberRole,
    ) {
        val ref = GroupRef.parse(groupId)
        if (ref.team == null) {
            val orgRole =
                when (role) {
                    GovernanceMemberRole.Admin -> OrgRole.ADMIN
                    GovernanceMemberRole.Member -> OrgRole.MEMBER
                    GovernanceMemberRole.ReadOnly -> OrgRole.GUEST
                }
            client.orgs.setMember(org = ref.org, account = identity, role = orgRole, passkey = approver("setMemberRole"))
        } else {
            val teamRole =
                when (role) {
                    GovernanceMemberRole.Admin -> TeamRole.LEAD
                    GovernanceMemberRole.Member -> TeamRole.MEMBER
                    GovernanceMemberRole.ReadOnly -> TeamRole.READ_ONLY
                }
            client.orgs.teamSetMember(org = ref.org, team = ref.team, account = identity, role = teamRole, passkey = approver("setMemberRole"))
        }
    }

    override suspend fun removeMember(
        groupId: String,
        identity: String,
    ) {
        val ref = GroupRef.parse(groupId)
        val passkey = approver("removeMember")
        if (ref.team == null) {
            client.orgs.removeMember(org = ref.org, account = identity, passkey = passkey)
        } else {
            client.orgs.teamRemoveMember(org = ref.org, team = ref.team, account = identity, passkey = passkey)
        }
    }

    override suspend fun setMemberName(
        groupId: String,
        name: String,
    ) {
        val org = GroupRef.parse(groupId).org
        val current = client.getOrg(org).memberMetadata[account] ?: JsonNull
        client.orgs.memberMetadata(org = org, account = account, metadata = withName(current, name))
    }

    // ---- helpers -------------------------------------------------------------------------

    /** [result], when approved; [OrgConsentNotApproved] otherwise. */
    private fun approved(result: ConsentResult): ConsentResult {
        if (!result.isApproved) throw OrgConsentNotApproved(result.kind, result.status)
        return result
    }

    private fun noOutcome(result: ConsentResult): Exception = IllegalStateException("an approved ${result.kind} answered no outcome")

    /** The passkey that approves [method]'s step-up, or [PasskeyRequired]. */
    private fun approver(method: String): PasskeyProvider = passkey() ?: throw PasskeyRequired(method)

    /** This package's apps in [org], by team (null: on no team). */
    private suspend fun appsByTeam(org: String): Map<String?, List<com.calimero.mero.ReportedApp>> =
        client
            .appsReportAll(org)
            .apps
            .filter { it.`package`?.`package` == packageName }
            .groupBy { it.team }

    private fun metadataOf(
        name: String,
        metadata: JsonElement,
    ): AppMetadata? {
        val fields = (metadata as? JsonObject).orEmpty()
        val shown = (fields["name"] as? JsonPrimitive)?.contentOrNull ?: name.ifEmpty { null }
        val data = fields.filterKeys { it != "name" }.mapValues { (_, v) -> (v as? JsonPrimitive)?.contentOrNull ?: v.toString() }
        if (shown == null && data.isEmpty()) return null
        return AppMetadata(shown, data)
    }

    private fun withName(
        metadata: JsonElement,
        name: String,
    ): JsonObject = JsonObject((metadata as? JsonObject).orEmpty() + ("name" to JsonPrimitive(name)))

    /** The organization [org] as [OrgInfo], for an app's own screens. */
    suspend fun organization(org: String): OrgInfo = client.getOrg(org)
}
