package com.calimero.mero

import kotlinx.serialization.KSerializer
import kotlinx.serialization.Serializable
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonDecoder
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonEncoder
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

// What a [CalimeroClient] sends and answers (mero-js `cloud-v2/client.ts` types).

/** A signed-in session: a `v2.…` session token (15 minutes) and its `rt_…` refresh token. */
@Serializable
data class CalimeroSession(
    val accountId: String,
    val sessionToken: String,
    /** When the session token expires, as the node said (Unix seconds). */
    val expiresAt: Long = 0,
    val refreshToken: String = "",
)

/** One event a call emitted. */
@Serializable
data class CallEvent(
    val kind: String,
    val data: JsonElement = JsonNull,
)

/** What a call (`/apps/{app}/intents`) answers: its log [index], the method's [result] and its events. */
@Serializable
data class CallResult(
    val index: Long = 0,
    val result: JsonElement = JsonNull,
    val events: List<CallEvent> = emptyList(),
)

/** What a view (`/apps/{app}/views`) answers. */
@Serializable
data class ViewResult(
    val index: Long = 0,
    val result: JsonElement = JsonNull,
)

/** One server-sent event: an app's committed event, or a presence change. [data] is parsed as JSON when it is. */
data class CloudEvent(
    val id: String,
    val kind: String,
    val data: JsonElement,
)

/** What installing a package release answers. */
@Serializable
data class PackageInstalled(
    val `package`: String,
    val version: String,
    val signerId: String = "",
    /** Service name (`""` for a single-service bundle) -> the bundle hash it runs as. */
    val services: Map<String, String> = emptyMap(),
)

/** One installed release of a package. */
@Serializable
data class PackageVersion(
    val version: String,
    val mpk: String = "",
    val size: Long = 0,
    /** `registry` or `upload`. */
    val source: String = "",
    val services: Map<String, String> = emptyMap(),
    val metadata: JsonElement = JsonNull,
    val outbound: List<String> = emptyList(),
    val installedBy: String = "",
    val installedAt: Long = 0,
)

/** An installed package and its releases. */
@Serializable
data class PackageInfo(
    val `package`: String,
    val signerId: String = "",
    val owner: String = "",
    val latest: String? = null,
    val versions: List<PackageVersion> = emptyList(),
)

/** One app instance as `listApps` answers it. */
@Serializable
data class AppInfo(
    val appId: String,
    val name: String = "",
    val owner: String = "",
    val org: String? = null,
    val bundle: String = "",
    val `package`: String? = null,
    val version: String? = null,
    /** The deployment it was created from, if any. */
    val deployment: String? = null,
)

/** An organization-wide upgrade run, as `upgradeOrg`, `upgradeRun` and `retryUpgradeRun` answer it. */
@Serializable
data class UpgradeRun(
    val runId: String,
    val `package`: String = "",
    val version: String = "",
    val team: String? = null,
    val cascade: Boolean = false,
    /** The account that started it, or `operator`. */
    val by: String = "",
    val startedAt: Long = 0,
    val apps: List<String> = emptyList(),
    val done: List<String> = emptyList(),
    /** App id -> why its upgrade failed. */
    val failed: Map<String, JsonElement> = emptyMap(),
    val finished: Boolean = false,
)

/**
 * One of an app's blobs: [blobId] base58 (what apps print), [id] hex. A component app's sealed file
 * ([CalimeroClient.uploadFile]) has [sealed] set and [sha256], the plaintext's SHA-256 (hex): shown
 * only to the app's owner and its organization's members.
 */
@Serializable
data class BlobInfo(
    val blobId: String,
    val id: String = "",
    val size: Long = 0,
    val sealed: Boolean = false,
    val sha256: String? = null,
)

/** One device present in an app (`publishPresence`); [state] hex, null once retracted. */
@Serializable
data class PresenceEntry(
    val account: String,
    val device: String = "",
    val state: String? = null,
)

/** What an organization-wide upgrade targets. */
@Serializable
data class OrgUpgradeTarget(
    val `package`: String,
    val version: String,
    /** Only this team's apps (and, with [cascade], those of the teams below it). */
    val team: String? = null,
    val cascade: Boolean? = null,
    val migrate: String? = null,
)

/** The release-deadline stop on an organization's new work. */
@Serializable
data class NewWorkStopped(
    val release: String,
    val after: String,
)

/**
 * `GET /orgs/{org}`: the organization as the caller sees it. [raw] is the whole answer (members,
 * teams, metadata, capabilities, ...); the typed fields are the ones every caller reads.
 */
data class OrgInfo(
    val raw: JsonObject,
    val orgId: String,
    val name: String,
    /** Account -> role (`guest`, `member`, `admin`, `owner`). */
    val members: Map<String, String>,
    /** The teams this caller sees, by id. */
    val teams: Map<String, TeamInfo>,
    val metadata: JsonElement,
    /** Account -> the member's metadata (a display name). */
    val memberMetadata: Map<String, JsonElement>,
    /** Accounts holding `APPROVE_RELEASES` there (owner and admins included). */
    val releaseApprovers: List<String>,
    /** The signer ids (`did:key`) bound to it as publisher. */
    val publishes: List<String>,
    /** Null unless a deadline stopped new work: approve its `release` (or newer) to lift it. */
    val newWorkStopped: NewWorkStopped?,
    /** The recovery key and any recovery under way; null while the organization has none. */
    val recovery: OrgRecovery? = null,
) {
    /**
     * [packageName]'s app team here: the top-level team whose `metadata.app.package` is
     * [packageName] (made with the organization on the consent page, or adopted by an admin); the
     * smallest id when several are. Null without one, or for a closed team the caller does not see.
     */
    fun appTeam(packageName: String): String? {
        if (packageName.isEmpty()) return null
        return teams.values
            .filter { team ->
                val app = (team.metadata as? JsonObject)?.get("app") as? JsonObject
                team.parent == null && (app?.get("package") as? JsonPrimitive)?.takeIf { it.isString }?.content == packageName
            }.minOfOrNull { it.teamId }
    }
}

/** A team of an organization (`GET /orgs/{org}/teams/{team}`, or one of [OrgInfo.teams]). */
@Serializable
data class TeamInfo(
    val teamId: String,
    val name: String = "",
    val parent: String? = null,
    /** `everyone` (open) or `invite`. */
    val join: String = "invite",
    /** Account -> whether they lead the team. */
    val members: Map<String, Boolean> = emptyMap(),
    val readOnly: List<String> = emptyList(),
    val metadata: JsonElement = JsonNull,
)

/** An organization the account is in, as `me()` lists it. */
@Serializable
data class OrgMembership(
    val orgId: String,
    val name: String = "",
    val role: String = "",
)

/** One of the account's passkeys, as `me()` lists it. */
@Serializable
data class PasskeyInfo(
    val id: String,
    val signCount: Long = 0,
    val label: String? = null,
)

/** `GET /auth/me`: the signed-in account. [raw] is the whole answer. */
data class AccountInfo(
    val raw: JsonObject,
    val accountId: String,
    val name: String,
    val passkeys: List<PasskeyInfo>,
    val orgs: List<OrgMembership>,
    /** `session` or `apiKey`. */
    val via: String,
    /** The passkey (device) this session was opened with. */
    val passkeyId: String?,
    /**
     * The package the session is bound to: opened on its frontend or on an origin its manager
     * registered ([CalimeroClient.packageOrigins]). Null for an unbound session.
     */
    val app: String? = null,
    /** The registered web origin the session was opened on, when the node recorded one. */
    val origin: String? = null,
)

/** An API key the account made (never the key itself). */
@Serializable
data class ApiKeyInfo(
    val keyId: String,
    val label: String = "",
    val apps: List<String>? = null,
    val methods: List<String>? = null,
    val readOnly: Boolean = false,
    val allowConfirm: Boolean = false,
    val expiresAt: Long = 0,
    /** Set for an upgrade key: the one app it upgrades, its package and that package's pinned signer. */
    val upgrades: UpgradeKeyScope? = null,
)

/**
 * An upgrade key's scope ([ApiKeyOptions.upgrades]): the one [app] it upgrades, to signed releases
 * of [package] (default: the package the app runs). [signer] (the package's pinned signer,
 * `did:key`) is the node's answer in [ApiKeyInfo]; never sent.
 */
@Serializable
data class UpgradeKeyScope(
    val app: String,
    val `package`: String? = null,
    val signer: String? = null,
)

/** What `createApiKey` takes. */
@Serializable
data class ApiKeyOptions(
    val label: String? = null,
    val apps: List<String>? = null,
    val methods: List<String>? = null,
    val readOnly: Boolean? = null,
    val allowConfirm: Boolean? = null,
    val ttlSecs: Long? = null,
    /**
     * Makes an upgrade key, for CI: it upgrades that one app to signed releases of its package with
     * [CalimeroClient.upgradeApp] and no passkey, and every other request with it is refused
     * `403 UPGRADE_KEY_ONLY`. It takes no other scope field ([label] and [ttlSecs] aside).
     */
    val upgrades: UpgradeKeyScope? = null,
)

/** A new API key: [key] is shown once. */
@Serializable
data class ApiKeyCreated(
    val keyId: String,
    val key: String,
)

/** What `createApp` creates from: a bundle hash (a raw upload, development only), or a package release. */
sealed interface AppSource {
    data class Bundle(
        val hash: String,
    ) : AppSource

    /**
     * A package release. With [deployment], the instance runs the release that deployment offers
     * for [package] ([version], if given, must be it) and records the deployment; no install, no
     * deployer grant. A package or version it does not offer answers `400 NOT_OFFERED`, several
     * offered releases with no [version] `400 VERSION_REQUIRED`, an unknown [service]
     * `400 NO_SUCH_SERVICE`. A page opened through `/d/{deployment}/` finds it with [pageDeployment].
     */
    data class Package(
        val `package`: String,
        val version: String? = null,
        val service: String? = null,
        val deployment: String? = null,
    ) : AppSource
}

/** Where `upgradeApp` moves an app: another release of its package, another package, or a bundle hash. */
@Serializable
data class AppUpgradeTarget(
    val version: String? = null,
    val `package`: String? = null,
    val service: String? = null,
    val bundle: String? = null,
    val migrate: String? = null,
)

/**
 * Who may reach an app (`setAccess`, `createApp`): `{ "kind": "owner" | "org" | "public" }`,
 * `{ "kind": "accounts", "accounts": [...] }` or `{ "kind": "team", "team": id }` on the wire, the
 * spec's `Access` (the generated client's type for it).
 */
@Serializable(with = AppAccessSerializer::class)
sealed interface AppAccess {
    /** Only the owner. */
    data object Owner : AppAccess

    /** Members of the app's organization. */
    data object Org : AppAccess

    /** Anyone, signed in or not. */
    data object Public : AppAccess

    /** The owner and these accounts. */
    data class Accounts(
        val accounts: List<String>,
    ) : AppAccess

    /** Members of one team of the app's organization. */
    data class Team(
        val team: String,
    ) : AppAccess
}

internal object AppAccessSerializer : KSerializer<AppAccess> {
    override val descriptor: SerialDescriptor = JsonObject.serializer().descriptor

    override fun serialize(
        encoder: Encoder,
        value: AppAccess,
    ) {
        (encoder as JsonEncoder).encodeJsonElement(CalimeroClient.accessJson(value))
    }

    override fun deserialize(decoder: Decoder): AppAccess {
        val o = (decoder as JsonDecoder).decodeJsonElement() as? JsonObject ?: error("an access is an object")
        return when (val kind = o.string("kind")) {
            "owner" -> AppAccess.Owner
            "org" -> AppAccess.Org
            "public" -> AppAccess.Public
            "accounts" -> AppAccess.Accounts((o["accounts"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }.orEmpty())
            "team" -> AppAccess.Team(o.string("team").orEmpty())
            else -> error("unknown access kind $kind")
        }
    }
}

/**
 * Which outbound URL prefixes an approval covers (the spec's `OutboundSelection`): [Manifest], every
 * prefix the package's release declares (the string `"manifest"`), or exactly [Prefixes] (a bare
 * JSON array).
 */
@Serializable(with = OutboundSelectionSerializer::class)
sealed interface OutboundSelection {
    /** Every prefix the release's manifest declares. */
    data object Manifest : OutboundSelection

    /** Exactly these prefixes. */
    data class Prefixes(
        val prefixes: List<String>,
    ) : OutboundSelection
}

internal object OutboundSelectionSerializer : KSerializer<OutboundSelection> {
    override val descriptor: SerialDescriptor = JsonElement.serializer().descriptor

    override fun serialize(
        encoder: Encoder,
        value: OutboundSelection,
    ) {
        val json =
            when (value) {
                OutboundSelection.Manifest -> JsonPrimitive("manifest")
                is OutboundSelection.Prefixes -> JsonArray(value.prefixes.map(::JsonPrimitive))
            }
        (encoder as JsonEncoder).encodeJsonElement(json)
    }

    override fun deserialize(decoder: Decoder): OutboundSelection =
        when (val json = (decoder as JsonDecoder).decodeJsonElement()) {
            is JsonArray -> OutboundSelection.Prefixes(json.mapNotNull { (it as? JsonPrimitive)?.content })
            else -> OutboundSelection.Manifest
        }
}

/** An organization role. */
enum class OrgRole(
    val wire: String,
) {
    Guest("guest"),
    Member("member"),
    Admin("admin"),
    Owner("owner"),
}

/** A member's row in a team. */
enum class TeamRole(
    val wire: String,
) {
    Lead("lead"),
    Member("member"),
    ReadOnly("readOnly"),
}

/** Who may join a team: everyone who reaches its parent (open), or invited members only. */
enum class TeamJoin(
    val wire: String,
) {
    Everyone("everyone"),
    Invite("invite"),
}

/**
 * How a call or a view is sent again while the node is unavailable: with the same key, after a
 * backoff that starts at [baseDelayMs] and doubles up to [maxDelayMs], no jitter, at most
 * [retries] times. The defaults are the Rust client's: 6 retries, 200 ms doubling, at most 2 s.
 */
data class CloudRetryOptions(
    val retries: Int = 6,
    val baseDelayMs: Long = 200,
    val maxDelayMs: Long = 2_000,
)

/** The backoff before retry [retry] (0 for the first): `baseDelayMs` doubling, at most `maxDelayMs`. */
fun retryDelayMs(
    retry: Int,
    options: CloudRetryOptions,
): Long = minOf(options.baseDelayMs * (1L shl minOf(retry, 30)), options.maxDelayMs)
