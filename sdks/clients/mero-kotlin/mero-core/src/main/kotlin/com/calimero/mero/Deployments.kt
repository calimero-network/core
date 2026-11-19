package com.calimero.mero

import com.calimero.mero.http.PlainJsonHttp
import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.JsonDecoder
import kotlinx.serialization.json.JsonEncoder
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.OkHttpClient

// Blobs with content types, a component app's xcalls, deployments and deployers (core rc.7+;
// mero-js `cloud-v2/client.ts`, #323).

/** A blob's bytes with the content type the node serves them as. */
data class BlobContent(
    val data: ByteArray,
    /** What the uploader gave (component apps); `application/octet-stream` otherwise. */
    val contentType: String,
) {
    override fun equals(other: Any?): Boolean = other is BlobContent && data.contentEquals(other.data) && contentType == other.contentType

    override fun hashCode(): Int = 31 * data.contentHashCode() + contentType.hashCode()
}

/** One xcall that was never delivered (core's `DeadLetter`). */
@Serializable
data class DeadLetter(
    val seq: Long,
    /** The target app. */
    val app: String,
    val method: String,
    val reason: String = "",
    /** The log entry's timestamp it was dead-lettered at (ms since the epoch). */
    val atMs: Long = 0,
)

/** `GET /apps/{app}/xcalls`: a component app's outbox and its dead letters, newest last. */
@Serializable
data class AppXcalls(
    /** Messages waiting in the outbox. */
    val pending: Long = 0,
    val deadLetters: List<DeadLetter> = emptyList(),
)

/**
 * An account or an organization, as a deployer, a deployment's owner or the cluster's owner:
 * `{ "account": id }` or `{ "org": id }` on the wire.
 */
@Serializable(with = DeployerSerializer::class)
sealed interface Deployer {
    /** `account:{id}` or `org:{id}`: what `GET /deployers` lists and `DELETE /deployers/{key}` takes. */
    val key: String

    data class Account(
        val account: String,
    ) : Deployer {
        override val key: String get() = "account:$account"
    }

    data class Org(
        val org: String,
    ) : Deployer {
        override val key: String get() = "org:$org"
    }
}

/** A deployer's key (mero-js `deployerKey`). */
fun deployerKey(deployer: Deployer): String = deployer.key

internal object DeployerSerializer : KSerializer<Deployer> {
    override val descriptor: SerialDescriptor = JsonObject.serializer().descriptor

    override fun serialize(
        encoder: Encoder,
        value: Deployer,
    ) {
        val json =
            buildJsonObject {
                when (value) {
                    is Deployer.Account -> put("account", value.account)
                    is Deployer.Org -> put("org", value.org)
                }
            }
        (encoder as JsonEncoder).encodeJsonElement(json)
    }

    override fun deserialize(decoder: Decoder): Deployer {
        val o = (decoder as JsonDecoder).decodeJsonElement() as? JsonObject ?: error("a deployer is an object")
        o.string("account")?.let { return Deployer.Account(it) }
        o.string("org")?.let { return Deployer.Org(it) }
        error("a deployer names an account or an org")
    }
}

/** Who may install apps on a cluster (`Governance.deployments`). */
@Serializable
enum class DeploymentsRule {
    /** Any account. */
    @SerialName("open")
    Open,

    /** Granted deployers only ([CalimeroClient.grantDeployer]). */
    @SerialName("granted")
    Granted,
}

/** Who grants deployers (`Governance.deployers`). */
@Serializable
enum class DeployersRule {
    /** The operator, with its key. */
    @SerialName("operator")
    Operator,

    /** The cluster's owner, with a passkey. */
    @SerialName("owner")
    Owner,
}

/** One deployer grant, as `GET /deployers` lists it. */
@Serializable
data class DeployerGrant(
    /** `account:{id}` or `org:{id}`. */
    val key: String,
    val deployer: Deployer? = null,
    /** RFC 3339; null: it does not end. */
    val until: String? = null,
    val untilSecs: Long? = null,
    val grantedAt: String = "",
    val grantedBy: String = "",
)

/** `GET /deployers`: who may install apps on the cluster, and who grants it. */
@Serializable
data class DeployersInfo(
    /** Any account, or granted deployers only; null on a default cluster. */
    val deployments: DeploymentsRule? = null,
    /** Who grants deployers: the operator (its key) or the cluster's owner (with a passkey). */
    val deployers: DeployersRule? = null,
    val owner: Deployer? = null,
    val grants: List<DeployerGrant> = emptyList(),
)

/** What `grantDeployer` answers. */
@Serializable
data class DeployerGranted(
    val key: String,
    val deployer: Deployer,
    val grant: JsonObject = JsonObject(emptyMap()),
)

/** One app a deployment offers, as its managers read it. */
@Serializable
data class DeploymentOffer(
    /** The app's name in the deployment. */
    val app: String,
    val `package`: String,
    val version: String,
    val addedBy: String = "",
    /** RFC 3339. */
    val addedAt: String = "",
)

/** A deployment as its managers read it (`GET /deployments`). */
@Serializable
data class DeploymentInfo(
    val id: String,
    val name: String = "",
    val owner: Deployer? = null,
    val apps: List<DeploymentOffer> = emptyList(),
    val createdBy: String = "",
    /** RFC 3339. */
    val createdAt: String = "",
)

/** A public deployment's owner: its kind (`account` or `org`) and name (null if it is gone). */
@Serializable
data class PublicDeploymentOwner(
    val kind: String,
    val name: String? = null,
)

/** One app a deployment offers, as anyone reads it. */
@Serializable
data class PublicDeploymentApp(
    val app: String,
    val `package`: String,
    val version: String,
)

/** `GET /deployments/{id}`, public: what a frontend and the customer's consent show. */
@Serializable
data class PublicDeployment(
    val id: String,
    val name: String = "",
    val owner: PublicDeploymentOwner? = null,
    val apps: List<PublicDeploymentApp> = emptyList(),
)

/** What `offerApp` answers. */
@Serializable
data class DeploymentOffered(
    val deployment: String,
    val app: String,
    val offer: JsonObject = JsonObject(emptyMap()),
)

/** Which of a deployment's apps an organization runs, at which release. */
@Serializable
data class DeploymentOrgApp(
    val app: String,
    val version: String,
)

/** One organization with instances created from a deployment. */
@Serializable
data class DeploymentOrg(
    val orgId: String,
    val name: String? = null,
    val instances: Long = 0,
    val apps: List<DeploymentOrgApp> = emptyList(),
    /** RFC 3339: when its first instance from the deployment was created. */
    val firstCreatedAt: String = "",
)

/** `GET /deployments/{id}/orgs`: the organizations with instances created from a deployment. */
@Serializable
data class DeploymentOrgs(
    val deployment: String,
    val orgs: List<DeploymentOrg> = emptyList(),
    /** Instances outside any organization, counted only. */
    val outsideOrgs: Long = 0,
)

/** A deployment a frontend manifest lists. */
@Serializable
data class FrontendDeployment(
    val id: String,
    val name: String = "",
    val app: String = "",
    val version: String = "",
)

/**
 * A package frontend's `/.well-known/calimero.json`: the package, its releases, the deployments
 * offering it and the cluster's API URLs.
 */
@Serializable
data class FrontendManifest(
    val `package`: String,
    val versions: List<String> = emptyList(),
    val deployments: List<FrontendDeployment> = emptyList(),
    val api: List<String> = emptyList(),
)

/**
 * Read the `/.well-known/calimero.json` of the frontend at [origin]. Served in the clear by the
 * frontend host: for choosing a deployment and the API URL to connect to, never for trust (the
 * client attests the node it then talks to). A refusal throws its [CalimeroError].
 */
suspend fun frontendManifest(
    origin: String,
    httpClient: OkHttpClient = PlainJsonHttp.defaultClient(),
): FrontendManifest {
    val base = origin.toHttpUrlOrNull() ?: throw IllegalArgumentException("not a URL: $origin")
    val url = base.resolve("/.well-known/calimero.json").toString()
    val response = PlainJsonHttp(httpClient, role = null).execute("GET", url)
    if (!response.isSuccessful) {
        val body = CalimeroClient.parseOrNull(response.body) ?: buildJsonObject { put("message", response.body) }
        throw errorFrom(
            response.status, body,
            response.headers.entries
                .firstOrNull { it.key.equals("retry-after", true) }
                ?.value,
        )
    }
    return CalimeroClient.wire.decodeFromString(FrontendManifest.serializer(), response.body)
}

/**
 * The deployment a frontend page was opened for: the node serves `/d/{deployment}/` by redirecting
 * to `/{version}/…?deployment={id}`, so the page creates its instances from that deployment
 * (`createApp(name, AppSource.Package(pkg, deployment = id))`). Null when [href] was not opened
 * through a deployment (or is not a URL). On Android, [href] is the deep link the app was opened with.
 */
fun pageDeployment(href: String?): String? {
    val query = href?.let { runCatching { java.net.URI(it.trim()).rawQuery }.getOrNull() } ?: return null
    return query
        .split('&')
        .firstOrNull { it.substringBefore('=') == "deployment" }
        ?.substringAfter('=', "")
        ?.let { java.net.URLDecoder.decode(it, "UTF-8") }
        ?.ifEmpty { null }
}
