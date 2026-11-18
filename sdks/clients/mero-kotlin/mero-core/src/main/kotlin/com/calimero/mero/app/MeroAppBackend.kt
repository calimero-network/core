package com.calimero.mero.app

import kotlinx.coroutines.flow.Flow
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull

/**
 * Everything an app's screens need from a signed-in Calimero account. (Swift
 * `MeroKit.MeroAppBackend`.)
 *
 * The method names are the 0.x ones the native apps were written against; on Calimero 1.0 each
 * maps onto the cluster's own model:
 *
 * | here | Calimero 1.0 |
 * |---|---|
 * | namespace (`namespaceId`) | organization (`orgId`) |
 * | subgroup (`groupId` of a subgroup) | team, as `"{orgId}/{teamId}"` ([teamGroupId]) |
 * | a namespace's root group (`groupId` = `namespaceId`) | the organization itself (apps not on a team) |
 * | context (`contextId`) | app instance (`appId`) |
 * | application (`applicationId`) | package (`com.example.app`), optionally `package#service` |
 * | contract query / execute | `POST /apps/{app}/views` / `POST /apps/{app}/intents` |
 * | invitation | an org invite token (`inv_…`), as [OrgInvitation] |
 * | relay | none: the account talks to the cluster itself over the sealed channel |
 *
 * [CalimeroAppBackend] is the real one, built by mero-compose's `MeroAppSession` from the signed-in
 * [com.calimero.mero.CalimeroClient]. View models take this interface, so a test hands them
 * mero-testkit's `InMemoryAppBackend` instead.
 *
 * The names, blobs, leaving and members calls have defaults that throw
 * [UnsupportedOperationException] naming the call, so a backend of an app's own still compiles.
 */
@Suppress("TooManyFunctions")
interface MeroAppBackend {
    /** The account, 64 hex. */
    val account: String

    /** The contract in any app instance: views for reads, intents for writes. */
    val contract: ContextContract

    /**
     * Whether the account can reach the cluster. 0.x: "a relay serves this account". On 1.0 there
     * is no relay, so a signed-in backend is always `true`; kept so apps written against 0.x
     * compile unchanged.
     */
    val hasRelay: Boolean

    /** Whether [events] streams. Always `true` on 1.0 (`GET /apps/{app}/events`, sealed SSE). */
    val canSubscribe: Boolean

    /** The organizations this account is in (`GET /auth/me`), as namespaces. */
    suspend fun listNamespaces(): List<AppNamespace>

    /** The teams of an organization this account sees, and the app instances of this app on each. */
    suspend fun listSubgroups(namespaceId: String): List<AppSubgroup>

    /** The app instances of this app directly in [groupId]: an organization (apps on no team), or a team ([teamGroupId]). */
    suspend fun listContexts(groupId: String): List<String>

    /** Whether the account reaches the app instance, so its contract calls work (`GET /apps/{app}`). */
    suspend fun isMember(contextId: String): Boolean

    /**
     * Join an app instance. On 1.0 an open team admits every member of its organization by itself,
     * so this only checks the app is reachable; an invite-only team needs a lead to add the
     * account. Throws [com.calimero.mero.NotFound] when it is not reachable.
     */
    suspend fun joinContext(contextId: String)

    /** Create an organization named [name] (`POST /orgs`). */
    suspend fun foundNamespace(name: String): AppFoundedNamespace

    /** Create a team in [namespaceId], open (`join: everyone`) or invite-only. Returns its group id ([teamGroupId]). */
    suspend fun createSubgroup(
        namespaceId: String,
        name: String,
        isPublic: Boolean,
    ): String

    /**
     * Create an app instance of [applicationId] (a package, or `package#service`) in [groupId]
     * (an organization, or a team: the instance is then reachable by that team), initialised with
     * [initArgs]. Returns its app id.
     */
    suspend fun createContext(
        groupId: String,
        applicationId: String,
        name: String,
        initArgs: JsonElement,
    ): String

    /** Mint an invite to an organization (`POST /orgs/{org}/invites`, role `member`, one use, 7 days); a passkey step-up. */
    suspend fun createInvitation(namespaceId: String): OrgInvitation

    /** Redeem an invite (`POST /invites/{token}/redeem`): the account joins the organization at once. */
    suspend fun joinNamespace(
        namespaceId: String,
        invitation: OrgInvitation,
    ): AppJoinedNamespace

    /** Live events for one app instance (`GET /apps/{app}/events`, from now on). */
    fun events(contextId: String): Flow<AppContextEvent>

    /**
     * Create an app instance of one [service] of a multi-service package (battleships' lobby and
     * match). The default joins them as `package#service` for [createContext].
     */
    suspend fun createServiceContext(
        groupId: String,
        applicationId: String,
        service: String,
        name: String,
        initArgs: JsonElement,
    ): String = createContext(groupId, "$applicationId#$service", name, initArgs)

    // ---- names ---------------------------------------------------------------------------

    /** The app instances directly in [groupId], with their names: [listContexts] with what a screen shows. */
    suspend fun listContextEntries(groupId: String): List<AppContextEntry> = unsupported("listContextEntries")

    /** An organization's or a team's name and metadata, or null if it has none. */
    suspend fun groupMetadata(groupId: String): AppMetadata? = unsupported("groupMetadata")

    /** An app instance's name and metadata, or null if it has none. */
    suspend fun contextMetadata(
        groupId: String,
        contextId: String,
    ): AppMetadata? = unsupported("contextMetadata")

    /** Rename a team (`POST /orgs/{org}/teams/{team}`), or set an organization's display name in its metadata. */
    suspend fun setGroupName(
        groupId: String,
        name: String,
    ): Unit = unsupported("setGroupName")

    /** Set an app instance's display name in its metadata, keeping the rest. */
    suspend fun setContextName(
        groupId: String,
        contextId: String,
        name: String,
    ): Unit = unsupported("setContextName")

    // ---- blobs ---------------------------------------------------------------------------

    /** Store [data] as one of [contextId]'s blobs (`POST /apps/{app}/blobs`). */
    suspend fun uploadBlob(
        contextId: String,
        data: ByteArray,
    ): AppBlob = unsupported("uploadBlob")

    /** A blob's bytes, one of [contextId]'s (`GET /apps/{app}/blobs/{id}`). */
    suspend fun getBlob(
        blobId: String,
        contextId: String,
    ): ByteArray = unsupported("getBlob")

    // ---- leaving and deleting ------------------------------------------------------------

    /** Leave an organization (`DELETE /orgs/{org}/members/{me}`, a passkey step-up): every team and app in it with it. */
    suspend fun leaveNamespace(namespaceId: String): Unit = unsupported("leaveNamespace")

    /** Leave a team (`DELETE /orgs/{org}/teams/{team}/members/{me}`, a passkey step-up), and with it the apps on it. */
    suspend fun leaveSubgroup(groupId: String): Unit = unsupported("leaveSubgroup")

    /** Delete a team and its apps. The node refuses it unless the account may. */
    suspend fun deleteSubgroup(groupId: String): Unit = unsupported("deleteSubgroup")

    /** Delete an app instance (its owner, or an admin of its organization). */
    suspend fun removeContext(
        groupId: String,
        contextId: String,
    ): Unit = unsupported("removeContext")

    // ---- members -------------------------------------------------------------------------

    /** The members of an organization or a team, with their roles and names (member metadata). */
    suspend fun listMembers(groupId: String): List<AppMember> = unsupported("listMembers")

    /** Set a member's role. Admins (organization) and leads (team) only; a passkey step-up. */
    suspend fun setMemberRole(
        groupId: String,
        identity: String,
        role: GovernanceMemberRole,
    ): Unit = unsupported("setMemberRole")

    /** Remove a member. Admins (organization) and leads (team) only; a passkey step-up. */
    suspend fun removeMember(
        groupId: String,
        identity: String,
    ): Unit = unsupported("removeMember")

    /** Set the account's own display name in an organization (member metadata), keeping the rest. */
    suspend fun setMemberName(
        groupId: String,
        name: String,
    ): Unit = unsupported("setMemberName")
}

private fun unsupported(method: String): Nothing = throw UnsupportedOperationException("$method is not supported by this MeroAppBackend")

/** A context in a group, with its name. (Swift `AppContextEntry`.) */
data class AppContextEntry(
    /** The context id. */
    val id: String,
    /** Its name (context metadata), when it has one. */
    val name: String?,
)

/** A group's, member's or context's metadata record. (Swift `AppMetadata`.) */
data class AppMetadata(
    val name: String?,
    /** The record's other fields. Setting a name keeps them. */
    val data: Map<String, String> = emptyMap(),
)

/** A member of an organization or a team. (Swift `AppMember`.) */
data class AppMember(
    /** The member's account id. */
    val identity: String,
    /** `Admin`, `Member` or `ReadOnly` ([GovernanceMemberRole]); an organization's owner is `Admin`. */
    val role: String,
    /** The name the member set (member metadata), when it set one. */
    val name: String? = null,
) {
    val isAdmin: Boolean get() = role == GovernanceMemberRole.Admin.name
}

/**
 * A member's role, as the apps name it. On 1.0: an organization's `admin` / `member` / `guest`, a
 * team's `lead` / `member` / `readOnly`.
 */
enum class GovernanceMemberRole {
    Admin,
    Member,
    ReadOnly,
}

/** One of an app instance's blobs. (Swift `AppBlob`.) */
data class AppBlob(
    val blobId: String,
    val size: Long,
)

/** An organization this account is in. (Swift `AppNamespace`.) */
data class AppNamespace(
    /** The organization id. */
    val id: String,
    /** Its name. */
    val name: String?,
    /** The app's package: what a new app instance in it is created from. */
    val applicationId: String,
    val memberCount: Int = 0,
)

/** A team of an organization, and the app instances of this app on it. (Swift `AppSubgroup`.) */
data class AppSubgroup(
    /** `"{orgId}/{teamId}"` ([teamGroupId]). */
    val groupId: String,
    val name: String?,
    val contextIds: List<String>,
    /** Whether every member of the organization is in it (`join: everyone`). */
    val isPublic: Boolean = false,
)

/** What creating an organization came to. (Swift `AppFoundedNamespace`.) */
data class AppFoundedNamespace(
    val namespaceId: String,
    /** 0.x: whether the cloud agreed to host the namespace. Always `true` on 1.0: the cluster hosts it. */
    val haEnabled: Boolean = true,
    val haError: String? = null,
    val nameError: String? = null,
)

/** What redeeming an invite came to. (Swift `AppJoinedNamespace`.) */
data class AppJoinedNamespace(
    val namespaceId: String,
    /** 0.x: the join was published, not yet applied. On 1.0 the account is a member at once: `true`. */
    val published: Boolean = true,
    /** 0.x: the relay that admitted the account. On 1.0 the node the client uses. */
    val relayUrl: String = "",
)

/** A live event in an app instance: something changed there. (Swift `AppContextEvent`.) */
data class AppContextEvent(
    val contextId: String,
    /** The app's event kind (`#[app::event]`). */
    val kind: String,
    /** The event's data, parsed as JSON when it is. */
    val data: JsonElement = JsonNull,
    /** The event's id (its log position), when the node sent one. */
    val id: String = "",
)

/** The group id of [team] in [org]: what [AppSubgroup.groupId] and the team calls take. */
fun teamGroupId(
    org: String,
    team: String,
): String = "$org/$team"

/** A group id read back: the organization, and the team when it names one. */
data class GroupRef(
    val org: String,
    val team: String?,
) {
    companion object {
        fun parse(groupId: String): GroupRef {
            val cut = groupId.indexOf('/')
            return if (cut < 0) GroupRef(groupId, null) else GroupRef(groupId.substring(0, cut), groupId.substring(cut + 1))
        }
    }
}
