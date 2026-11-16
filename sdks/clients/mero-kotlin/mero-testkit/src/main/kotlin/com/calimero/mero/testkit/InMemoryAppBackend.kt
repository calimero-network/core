package com.calimero.mero.testkit

import com.calimero.mero.app.AppBlob
import com.calimero.mero.app.AppContextEntry
import com.calimero.mero.app.AppContextEvent
import com.calimero.mero.app.AppFoundedNamespace
import com.calimero.mero.app.AppJoinedNamespace
import com.calimero.mero.app.AppMember
import com.calimero.mero.app.AppMetadata
import com.calimero.mero.app.AppNamespace
import com.calimero.mero.app.AppSubgroup
import com.calimero.mero.app.ContextContract
import com.calimero.mero.app.GovernanceMemberRole
import com.calimero.mero.app.MeroAppBackend
import com.calimero.mero.app.OrgInvitation
import com.calimero.mero.app.SignedOutContextContract
import com.calimero.mero.app.SignedOutException
import kotlinx.coroutines.channels.SendChannel
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull

// In-memory stand-ins for an app's signed-in session, so an app's view models are unit-testable
// with no cluster and no passkey. (Swift MeroKitTestSupport `InMemoryAppBackend` /
// `RecordingContextContract`.)

/** One contract call a [RecordingContextContract] saw. */
data class RecordedContractCall(
    val path: Path,
    val contextId: String,
    val method: String,
    val args: JsonElement,
) {
    enum class Path { QUERY, EXECUTE }
}

/**
 * A [ContextContract] that records every call and answers from [handler] (`null` by default).
 * Script the app's contract in the handler, by method.
 */
class RecordingContextContract(
    @Volatile var handler: (RecordedContractCall) -> JsonElement? = { JsonNull },
) : ContextContract {
    private val recorded = mutableListOf<RecordedContractCall>()

    /** Every call so far, in order. */
    val calls: List<RecordedContractCall> get() = synchronized(recorded) { recorded.toList() }

    /** The calls to [method]. */
    fun calls(method: String): List<RecordedContractCall> = calls.filter { it.method == method }

    fun reset() = synchronized(recorded) { recorded.clear() }

    override suspend fun query(
        contextId: String,
        method: String,
        args: JsonElement,
    ): JsonElement? = record(RecordedContractCall(RecordedContractCall.Path.QUERY, contextId, method, args))

    override suspend fun execute(
        contextId: String,
        method: String,
        args: JsonElement,
    ): JsonElement? = record(RecordedContractCall(RecordedContractCall.Path.EXECUTE, contextId, method, args))

    private fun record(call: RecordedContractCall): JsonElement? {
        synchronized(recorded) { recorded += call }
        return handler(call)
    }
}

/**
 * A [MeroAppBackend] that keeps its namespaces, subgroups, contexts and memberships in memory,
 * and whose contract is a [RecordingContextContract].
 *
 * It behaves like a Calimero 1.0 cluster as far as a view model can tell: founding adds a namespace, creating a
 * subgroup or a context adds it (the creator is a member of a context it creates), joining a
 * context makes the account a member, and redeeming an invitation adds the namespace. Names, members and blobs behave the same way: the founder or creator
 * of a group is its `Admin`, a joiner a `Member`; leaving or deleting takes the contexts with it; a
 * blob reads back only through the context it was stored in. Seed state with [namespaces],
 * [subgroups], [members], [contextNames] and [groupMembers]; make every
 * admin-side call fail with [failure]; push live events with [emit] (to streams being collected:
 * [subscriberCount] says how many). [log] lists what was asked, in order.
 */
@Suppress("TooManyFunctions")
class InMemoryAppBackend(
    override val account: String = "ab".repeat(32),
    val recording: RecordingContextContract = RecordingContextContract(),
) : MeroAppBackend {
    private val lock = Any()
    private var next = 0
    private val streams = mutableListOf<Pair<String, SendChannel<AppContextEvent>>>()
    private val asked = mutableListOf<String>()
    private val subgroupsCreatedOpen = mutableSetOf<String>()
    private val contextInitArgs = mutableMapOf<String, JsonElement>()
    private val groupData = mutableMapOf<String, Map<String, String>>()
    private val storedBlobs = mutableMapOf<String, Pair<String, ByteArray>>()

    @Volatile override var hasRelay: Boolean = true

    @Volatile override var canSubscribe: Boolean = true

    /** The application a founded or joined namespace runs. */
    @Volatile var applicationId: String = "ap".repeat(32)

    /** Thrown by every call but [events] and the contract while set. */
    @Volatile var failure: Throwable? = null

    /** What [foundNamespace] reports about hosting. */
    @Volatile var haEnabled: Boolean = true

    /** The node [joinNamespace] reports. */
    @Volatile var joinRelayUrl: String = "https://relay.test"

    /** The namespaces the account is in. */
    var namespaces: List<AppNamespace> = emptyList()
        get() = synchronized(lock) { field }
        set(value) = synchronized(lock) { field = value }

    /** Subgroups by namespace id. */
    var subgroups: Map<String, List<AppSubgroup>> = emptyMap()
        get() = synchronized(lock) { field }
        set(value) = synchronized(lock) { field = value }

    /** Contexts directly in a group that is not a listed subgroup (a namespace's root), by group id. */
    var rootContexts: Map<String, List<String>> = emptyMap()
        get() = synchronized(lock) { field }
        set(value) = synchronized(lock) { field = value }

    /** The contexts the account is a member of. */
    var members: Set<String> = emptySet()
        get() = synchronized(lock) { field }
        set(value) = synchronized(lock) { field = value }

    /** Context names (context metadata), by context id. */
    var contextNames: Map<String, String> = emptyMap()
        get() = synchronized(lock) { field }
        set(value) = synchronized(lock) { field = value }

    /** The members of each namespace and subgroup, by group id. */
    var groupMembers: Map<String, List<AppMember>> = emptyMap()
        get() = synchronized(lock) { field }
        set(value) = synchronized(lock) { field = value }

    /** The stored blobs' bytes, by blob id. */
    val blobs: Map<String, ByteArray> get() = synchronized(lock) { storedBlobs.mapValues { it.value.second } }

    /** The subgroups created open. */
    val publicSubgroups: Set<String> get() = synchronized(lock) { subgroupsCreatedOpen.toSet() }

    /** The `initArgs` each created context was given, by context id. */
    val initArgs: Map<String, JsonElement> get() = synchronized(lock) { contextInitArgs.toMap() }

    /** What was asked, in order: `"foundNamespace Team"`, `"joinContext <id>"`, ... */
    val log: List<String> get() = synchronized(lock) { asked.toList() }

    /** How many [events] streams are being collected. */
    val subscriberCount: Int get() = synchronized(lock) { streams.size }

    /** [recording], or [SignedOutContextContract] while [hasRelay] is false. */
    override val contract: ContextContract get() = if (hasRelay) recording else SignedOutContextContract

    // ---- MeroAppBackend ------------------------------------------------------------------

    override suspend fun listNamespaces(): List<AppNamespace> = step("listNamespaces") { namespaces }

    override suspend fun listSubgroups(namespaceId: String): List<AppSubgroup> = step("listSubgroups $namespaceId") { subgroups[namespaceId].orEmpty() }

    override suspend fun listContexts(groupId: String): List<String> = step("listContexts $groupId") { contextsIn(groupId) }

    override suspend fun isMember(contextId: String): Boolean = step("isMember $contextId") { contextId in members }

    override suspend fun joinContext(contextId: String) = step("joinContext $contextId") { members = members + contextId }

    override suspend fun foundNamespace(name: String): AppFoundedNamespace =
        step("foundNamespace $name") {
            if (!hasRelay) throw SignedOutException("foundNamespace")
            val id = mint("aa")
            namespaces = namespaces + AppNamespace(id, name, applicationId, memberCount = 1)
            groupMembers = groupMembers + (id to listOf(AppMember(account, GovernanceMemberRole.Admin.name)))
            AppFoundedNamespace(id, haEnabled, if (haEnabled) null else "not hosted (fake)")
        }

    override suspend fun createSubgroup(
        namespaceId: String,
        name: String,
        isPublic: Boolean,
    ): String =
        step("createSubgroup $namespaceId $name ${if (isPublic) "open" else "restricted"}") {
            val id = mint("bb")
            subgroups = subgroups + (namespaceId to subgroups[namespaceId].orEmpty() + AppSubgroup(id, name, emptyList(), isPublic))
            if (isPublic) subgroupsCreatedOpen += id
            groupMembers = groupMembers + (id to listOf(AppMember(account, GovernanceMemberRole.Admin.name)))
            id
        }

    override suspend fun createContext(
        groupId: String,
        applicationId: String,
        name: String,
        initArgs: JsonElement,
    ): String =
        step("createContext $groupId $applicationId $name") {
            val id = mint("cc")
            var placed = false
            subgroups =
                subgroups.mapValues { (_, list) ->
                    list.map { if (it.groupId == groupId) it.copy(contextIds = it.contextIds + id).also { placed = true } else it }
                }
            if (!placed) rootContexts = rootContexts + (groupId to rootContexts[groupId].orEmpty() + id)
            members = members + id
            contextInitArgs[id] = initArgs
            contextNames = contextNames + (id to name)
            id
        }

    override suspend fun createInvitation(namespaceId: String): OrgInvitation = step("createInvitation $namespaceId") { invitation(namespaceId) }

    override suspend fun joinNamespace(
        namespaceId: String,
        invitation: OrgInvitation,
    ): AppJoinedNamespace =
        step("joinNamespace $namespaceId") {
            if (namespaces.none { it.id == namespaceId }) namespaces = namespaces + AppNamespace(namespaceId, null, applicationId)
            if (groupMembers[namespaceId].orEmpty().none { it.identity == account }) {
                groupMembers = groupMembers + (namespaceId to groupMembers[namespaceId].orEmpty() + AppMember(account, GovernanceMemberRole.Member.name))
            }
            hasRelay = true
            AppJoinedNamespace(namespaceId, published = true, relayUrl = joinRelayUrl)
        }

    // ---- names ---------------------------------------------------------------------------

    override suspend fun listContextEntries(groupId: String): List<AppContextEntry> =
        step("listContextEntries $groupId") { contextsIn(groupId).map { AppContextEntry(it, contextNames[it]) } }

    /** Null for a group this backend does not know. */
    override suspend fun groupMetadata(groupId: String): AppMetadata? =
        step("groupMetadata $groupId") {
            val namespace = namespaces.firstOrNull { it.id == groupId }
            val subgroup = subgroups.values.flatten().firstOrNull { it.groupId == groupId }
            when {
                namespace != null -> AppMetadata(namespace.name, groupData[groupId].orEmpty())
                subgroup != null -> AppMetadata(subgroup.name, groupData[groupId].orEmpty())
                else -> null
            }
        }

    override suspend fun contextMetadata(
        groupId: String,
        contextId: String,
    ): AppMetadata? = step("contextMetadata $groupId $contextId") { contextNames[contextId]?.let { AppMetadata(it) } }

    /** Renames the namespace or subgroup in the listings; throws [IllegalArgumentException] for an unknown group. */
    override suspend fun setGroupName(
        groupId: String,
        name: String,
    ) = step("setGroupName $groupId $name") {
        val inNamespaces = namespaces.any { it.id == groupId }
        val inSubgroups = subgroups.values.flatten().any { it.groupId == groupId }
        require(inNamespaces || inSubgroups) { "no group $groupId" }
        namespaces = namespaces.map { if (it.id == groupId) it.copy(name = name) else it }
        subgroups = subgroups.mapValues { (_, list) -> list.map { if (it.groupId == groupId) it.copy(name = name) else it } }
    }

    override suspend fun setContextName(
        groupId: String,
        contextId: String,
        name: String,
    ) = step("setContextName $groupId $contextId $name") { contextNames = contextNames + (contextId to name) }

    // ---- blobs ---------------------------------------------------------------------------

    override suspend fun uploadBlob(
        contextId: String,
        data: ByteArray,
    ): AppBlob =
        step("uploadBlob $contextId ${data.size}") {
            if (!hasRelay) throw SignedOutException("uploadBlob")
            val id = mint("dd")
            storedBlobs[id] = contextId to data.copyOf()
            AppBlob(id, data.size.toLong())
        }

    /** Throws [IllegalArgumentException] unless the blob was stored through [contextId], as the node refuses it. */
    override suspend fun getBlob(
        blobId: String,
        contextId: String,
    ): ByteArray =
        step("getBlob $blobId $contextId") {
            val blob = storedBlobs[blobId]
            require(blob != null && blob.first == contextId) { "no blob $blobId in context $contextId" }
            blob.second.copyOf()
        }

    // ---- leaving and deleting ------------------------------------------------------------

    /** Drops the namespace, its subgroups and the account's contexts in them. */
    override suspend fun leaveNamespace(namespaceId: String) =
        step("leaveNamespace $namespaceId") {
            namespaces = namespaces.filter { it.id != namespaceId }
            subgroups[namespaceId].orEmpty().forEach { subgroup ->
                members = members - subgroup.contextIds.toSet()
                dropMember(account, subgroup.groupId)
            }
            members = members - rootContexts[namespaceId].orEmpty().toSet()
            subgroups = subgroups - namespaceId
            dropMember(account, namespaceId)
        }

    /** The subgroup stays listed (it may be open); the account is no longer in it or its contexts. */
    override suspend fun leaveSubgroup(groupId: String) =
        step("leaveSubgroup $groupId") {
            members = members - contextsIn(groupId).toSet()
            dropMember(account, groupId)
        }

    override suspend fun deleteSubgroup(groupId: String) =
        step("deleteSubgroup $groupId") {
            members = members - contextsIn(groupId).toSet()
            subgroups = subgroups.mapValues { (_, list) -> list.filter { it.groupId != groupId } }
            groupMembers = groupMembers - groupId
            subgroupsCreatedOpen -= groupId
        }

    override suspend fun removeContext(
        groupId: String,
        contextId: String,
    ) = step("removeContext $groupId $contextId") {
        subgroups =
            subgroups.mapValues { (_, list) ->
                list.map { if (it.groupId == groupId) it.copy(contextIds = it.contextIds - contextId) else it }
            }
        rootContexts[groupId]?.let { rootContexts = rootContexts + (groupId to it - contextId) }
        members = members - contextId
    }

    // ---- members -------------------------------------------------------------------------

    override suspend fun listMembers(groupId: String): List<AppMember> = step("listMembers $groupId") { groupMembers[groupId].orEmpty() }

    /** Throws [IllegalArgumentException] for someone who is not a member. */
    override suspend fun setMemberRole(
        groupId: String,
        identity: String,
        role: GovernanceMemberRole,
    ) = step("setMemberRole $groupId $identity ${role.name}") {
        val list = groupMembers[groupId].orEmpty()
        require(list.any { it.identity == identity }) { "$identity is not a member of $groupId" }
        groupMembers = groupMembers + (groupId to list.map { if (it.identity == identity) it.copy(role = role.name) else it })
    }

    override suspend fun removeMember(
        groupId: String,
        identity: String,
    ) = step("removeMember $groupId $identity") { dropMember(identity, groupId) }

    override suspend fun setMemberName(
        groupId: String,
        name: String,
    ) = step("setMemberName $groupId $name") {
        val list = groupMembers[groupId].orEmpty()
        val updated =
            if (list.any { it.identity == account }) {
                list.map { if (it.identity == account) it.copy(name = name) else it }
            } else {
                list + AppMember(account, GovernanceMemberRole.Member.name, name)
            }
        groupMembers = groupMembers + (groupId to updated)
    }

    override fun events(contextId: String): Flow<AppContextEvent> {
        if (!canSubscribe) return emptyFlow()
        return callbackFlow {
            val entry = contextId to channel
            synchronized(lock) { streams += entry }
            awaitClose { synchronized(lock) { streams -= entry } }
        }
    }

    // ---- driving it ----------------------------------------------------------------------

    /** Deliver [event] to every [events] stream being collected for its context. */
    fun emit(event: AppContextEvent) {
        val targets = synchronized(lock) { streams.filter { it.first == event.contextId }.map { it.second } }
        targets.forEach { it.trySend(event) }
    }

    /** End every [events] stream being collected, with [error] when given. */
    fun finishEvents(error: Throwable? = null) {
        val targets = synchronized(lock) { streams.map { it.second } }
        targets.forEach { it.close(error) }
    }

    // ---- plumbing --------------------------------------------------------------------------

    private fun contextsIn(groupId: String): List<String> =
        subgroups.values
            .flatten()
            .firstOrNull { it.groupId == groupId }
            ?.contextIds ?: rootContexts[groupId].orEmpty()

    private fun dropMember(
        identity: String,
        groupId: String,
    ) {
        groupMembers[groupId]?.let { list -> groupMembers = groupMembers + (groupId to list.filter { it.identity != identity }) }
    }

    private fun mint(prefix: String): String {
        next += 1
        val digits = next.toString(HEX_RADIX)
        return prefix + "0".repeat(ID_LENGTH - prefix.length - digits.length) + digits
    }

    private fun <T> step(
        entry: String,
        body: () -> T,
    ): T =
        synchronized(lock) {
            asked += entry
            failure?.let { throw it }
            body()
        }

    companion object {
        private const val HEX_RADIX = 16
        private const val ID_LENGTH = 64

        /** An invite to [namespaceId] shaped like a real one (its token is made up). */
        fun invitation(namespaceId: String): OrgInvitation = OrgInvitation(namespaceId, "inv_fake-$namespaceId")
    }
}
