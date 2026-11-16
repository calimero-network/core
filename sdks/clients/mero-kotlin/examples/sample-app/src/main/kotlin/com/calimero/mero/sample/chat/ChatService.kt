package com.calimero.mero.sample.chat

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.calimero.mero.AppError
import com.calimero.mero.CalimeroError
import com.calimero.mero.app.AppContextEvent
import com.calimero.mero.app.MeroAppBackend
import com.calimero.mero.app.OrgInvitation
import com.calimero.mero.sample.app.SampleLog
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.put

private val chatJson =
    Json {
        ignoreUnknownKeys = true
        encodeDefaults = true
    }

// ---- Wire models (com.calimero.chat contract, snake_case) ------------------

@Serializable
data class ChatMessage(
    val id: String,
    val text: String,
    @SerialName("sender_username") val senderUsername: String = "",
    val sender: String = "",
    val timestamp: Long = 0,
    val deleted: Boolean? = null,
)

/** One entry of `get_profiles`: the name a member registered with `set_profile`. */
@Serializable
data class ChatProfile(
    val identity: String,
    val username: String = "",
)

@Serializable
data class ChatMessagePage(
    @SerialName("total_count") val totalCount: Int = 0,
    val messages: List<ChatMessage> = emptyList(),
    @SerialName("start_position") val startPosition: Int = 0,
)

// ---- View models -----------------------------------------------------------

/** A space: an organization the account is in. */
data class ChatSpace(
    val id: String,
    val name: String,
    /** The package a new channel is created from. */
    val applicationId: String = ChatService.PACKAGE_NAME,
)

/** A channel: an instance of mero-chat in the space, on a team or org-wide. */
data class ChatChannel(
    val contextId: String,
    val groupId: String?,
    val name: String,
    val kind: String = "Channel",
)

/** Where [ChatService.spaces] stands. Mirrors the Swift sample's `SpacesLoad`. */
enum class SpacesLoad {
    /** Not listed yet. */
    PENDING,

    /** Listed: [ChatService.spaces] is the answer, empty or not. */
    LOADED,

    /** The listing failed. */
    FAILED,
}

// ---- ChatService -----------------------------------------------------------

/**
 * A native mero-chat frontend on Calimero 1.0: spaces (organizations), channels (app instances of
 * `com.calimero.chat`, one per team or org-wide), messages (views and calls on the contract),
 * invite and join (an org invite token in a link) — all through the app-facing
 * [MeroAppBackend], so it runs unchanged on mero-testkit's `InMemoryAppBackend` (the debug demo).
 * Mirrors the Swift sample's `ChatService`.
 */
@Suppress("TooManyFunctions")
class ChatService(
    private val backend: MeroAppBackend,
    displayName: String? = null,
    /** Bumped when the session changes; the Chat tab re-lists spaces then. */
    val revision: StateFlow<Int> = MutableStateFlow(0),
) {
    /** The package channels run. Never null on 1.0: the cluster installs it from the registry when a channel is created. */
    val appId: String? = PACKAGE_NAME

    var spaces by mutableStateOf<List<ChatSpace>>(emptyList())
        private set
    var spacesLoad by mutableStateOf(SpacesLoad.PENDING)
        private set
    private var spacesGeneration = 0
    var channels by mutableStateOf<List<ChatChannel>>(emptyList())
        private set
    var messages by mutableStateOf<List<ChatMessage>>(emptyList())
        private set
    var status by mutableStateOf("")
        private set
    var statusIsError by mutableStateOf(false)
        private set
    var busy by mutableStateOf(false)
        private set

    /** Why the last send failed, shown above the composer. */
    var sendError by mutableStateOf<String?>(null)
        private set

    /** Where the Chat tab is: kept here so switching tabs and back lands in the same place. */
    var openSpace by mutableStateOf<ChatSpace?>(null)
    var openChannel by mutableStateOf<ChatChannel?>(null)

    private var autoJoinClaimed = false

    /** True the first time only: the launch-time invite (the e2e hook) is redeemed once. */
    fun claimAutoJoin(): Boolean = !autoJoinClaimed.also { autoJoinClaimed = true }

    /** The display name registered with `set_profile`. */
    val username: String = displayName?.takeIf { it.isNotEmpty() } ?: "Account ${backend.account.take(ACCOUNT_PREFIX)}"

    /** Live events for a channel's app instance. */
    fun eventStream(channel: ChatChannel): Flow<AppContextEvent> = backend.events(channel.contextId)

    /** Nothing to install on 1.0: list the spaces. */
    suspend fun setup() = loadSpaces()

    suspend fun detectInstalled() = loadSpaces()

    // ---- spaces ------------------------------------------------------------

    /** List the organizations the account is in. A failed listing is [SpacesLoad.FAILED], never an empty one. */
    suspend fun loadSpaces() {
        val generation = ++spacesGeneration
        try {
            val listed = backend.listNamespaces().map { ChatSpace(it.id, it.name ?: "Space", it.applicationId) }
            if (generation != spacesGeneration) return
            spaces = listed
            spacesLoad = SpacesLoad.LOADED
            if (statusIsError) say("")
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (generation != spacesGeneration) return
            spacesLoad = SpacesLoad.FAILED
            say("Couldn't load spaces: ${short(e)}", error = true)
        }
    }

    /** Any signed-in account may create an organization. */
    val canCreateSpaces: Boolean get() = true

    /** Offer "New space" only once the listing answered, so a failed one can't lead to a duplicate. */
    val offerNewSpace: Boolean get() = spacesLoad == SpacesLoad.LOADED

    /** Create an organization with an open `general` team and a `general` channel on it. */
    suspend fun createSpace(name: String) {
        run("Creating space $name") {
            val org = backend.foundNamespace(name).namespaceId
            val team = backend.createSubgroup(org, "general", isPublic = true)
            val channel = backend.createContext(team, PACKAGE_NAME, "general", initArgs("general"))
            runCatching { write(channel, "set_profile", profileArgs()) }
            say("Created $name")
            loadSpaces()
        }
    }

    // ---- channels ----------------------------------------------------------

    /** The space's channels: mero-chat instances on each team it sees, and org-wide ones. */
    suspend fun loadChannels(space: ChatSpace) {
        try {
            val onTeams =
                backend.listSubgroups(space.id).flatMap { team ->
                    backend.listContextEntries(team.groupId).map { ChatChannel(it.id, team.groupId, it.name ?: team.name ?: "channel") }
                }
            val orgWide = backend.listContextEntries(space.id).map { ChatChannel(it.id, space.id, it.name ?: "channel") }
            channels = (onTeams + orgWide).distinctBy { it.contextId }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            say("Couldn't load channels: ${short(e)}", error = true)
        }
    }

    /** Create a channel open to everyone in the space. */
    suspend fun createChannel(
        space: ChatSpace,
        name: String,
    ) {
        run("Creating #$name") {
            val channel = backend.createContext(space.id, PACKAGE_NAME, name, initArgs(name))
            runCatching { write(channel, "set_profile", profileArgs()) }
            say("Created #$name")
            loadChannels(space)
        }
    }

    /** Re-list the space's channels (there is nothing to sync on 1.0: the cluster holds one copy). */
    suspend fun resync(space: ChatSpace) = loadChannels(space)

    // ---- messages ----------------------------------------------------------

    suspend fun loadMessages(channel: ChatChannel) {
        try {
            val args =
                buildJsonObject {
                    put("parent_message", JsonNull)
                    put("limit", MESSAGE_PAGE)
                    put("offset", 0)
                    put("search_term", JsonNull)
                }
            val page = read<ChatMessagePage>(channel.contextId, "get_messages", args)
            val names =
                runCatching { read<List<ChatProfile>>(channel.contextId, "get_profiles") }
                    .getOrNull()
                    .orEmpty()
                    .associate { it.identity to it.username }
            messages =
                page.messages
                    .filter { it.deleted != true }
                    .map { m -> if (m.senderUsername.isEmpty()) m.copy(senderUsername = names[m.sender].orEmpty()) else m }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            say("Couldn't load messages: ${short(e)}", error = true)
        }
    }

    suspend fun sendMessage(
        channel: ChatChannel,
        text: String,
    ) {
        if (text.isEmpty()) return
        try {
            val args =
                buildJsonObject {
                    put("message", text)
                    put("mentions", buildJsonArray { })
                    put("mentions_usernames", buildJsonArray { })
                    put("parent_message", JsonNull)
                    put("timestamp", System.currentTimeMillis() / MILLIS_PER_SECOND)
                    put("files", JsonNull)
                    put("images", JsonNull)
                }
            write(channel.contextId, "send_message", args)
            sendError = null
            loadMessages(channel)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            sendError = short(e)
            SampleLog.log(SampleLog.Level.ERR, "chat: send failed: ${short(e)}")
        }
    }

    // ---- invite and join ---------------------------------------------------

    /** Every member may try; the node refuses one who lacks `INVITE_MEMBERS`. */
    val canInvite: Boolean get() = true

    /** Mint an invite link to [space], or null (the reason is in [status]). */
    suspend fun makeInvite(space: ChatSpace): String? =
        try {
            val invitation = backend.createInvitation(space.id).copy(name = space.name)
            say("Invite ready for ${space.name}")
            invitation.link(PACKAGE_NAME)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            say("Couldn't create an invite: ${short(e)}", error = true)
            null
        }

    /** Redeem an invite link or code: the account joins the organization at once. */
    suspend fun joinSpace(inviteCode: String): Boolean {
        val invitation = OrgInvitation.parse(inviteCode, PACKAGE_NAME)
        if (invitation == null) {
            say("That isn't a Mero Chat invite.", error = true)
            return false
        }
        var joined = false
        run("Joining ${invitation.name ?: "space"}") {
            backend.joinNamespace(invitation.namespaceId, invitation)
            joined = true
            loadSpaces()
            spaces.firstOrNull { it.id == invitation.namespaceId }?.let { space ->
                loadChannels(space)
                channels.forEach { runCatching { write(it.contextId, "set_profile", profileArgs()) } }
            }
            say("Joined ${invitation.name ?: "the space"}")
        }
        return joined
    }

    // ---- helpers -----------------------------------------------------------

    private fun initArgs(name: String): JsonObject =
        buildJsonObject {
            put("name", name)
            put("context_type", "Channel")
            put("description", "")
            put("created_at", System.currentTimeMillis() / MILLIS_PER_SECOND)
            put("creator_username", username)
        }

    private fun profileArgs(): JsonObject =
        buildJsonObject {
            put("username", username)
            put("avatar", JsonNull)
        }

    /** A read: a view on the app instance. */
    private suspend inline fun <reified T> read(
        contextId: String,
        method: String,
        args: JsonObject = JsonObject(emptyMap()),
    ): T {
        val out: JsonElement = backend.contract.query(contextId, method, args) ?: JsonNull
        return chatJson.decodeFromJsonElement(out)
    }

    /** A write: a call on the app instance. */
    private suspend fun write(
        contextId: String,
        method: String,
        args: JsonObject,
    ) {
        backend.contract.execute(contextId, method, args)
    }

    private fun say(
        message: String,
        error: Boolean = false,
    ) {
        status = message
        statusIsError = error
        if (message.isNotEmpty()) SampleLog.log(if (error) SampleLog.Level.ERR else SampleLog.Level.INFO, "chat: $message")
    }

    private suspend fun run(
        message: String,
        body: suspend () -> Unit,
    ) {
        busy = true
        say("$message…")
        try {
            body()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            say("$message failed: ${short(e)}", error = true)
        } finally {
            busy = false
        }
    }

    private fun short(error: Exception): String =
        when (error) {
            is AppError -> error.appCode.ifEmpty { error.nodeMessage }
            is CalimeroError -> error.nodeMessage.ifEmpty { error.code }
            else -> error.message ?: error.toString()
        }

    companion object {
        /** mero-chat (`apps/mero-chat`), as the app registry publishes it. Invite links name it too. */
        const val PACKAGE_NAME = "com.calimero.chat"
        private const val MESSAGE_PAGE = 50
        private const val MILLIS_PER_SECOND = 1000
        private const val ACCOUNT_PREFIX = 6
    }
}
