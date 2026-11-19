package com.calimero.mero.agentgate

import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.concat
import com.calimero.mero.crypto.randomBytes
import com.calimero.mero.crypto.sha256
import com.calimero.mero.http.MeroException
import com.calimero.mero.http.PlainJsonHttp
import com.calimero.mero.http.trimBase
import com.calimero.mero.tee.TransportQuote
import com.calimero.mero.tee.VerifyTransportQuote
import com.calimero.mero.tee.X25519KeyPair
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
import okhttp3.OkHttpClient
import org.bouncycastle.util.encoders.Base64

/*
 * A client for mero-agent-gate, the gate in a private agent TD (mero-tee,
 * `mero-agent-gate/`): attest an agent, claim it with your key, give it its secrets sealed to
 * the TD, and release it — port of mero-js `src/agent-gate/gate.ts`, wire-identical with the
 * gate's `protocol.rs` / `server.rs` (`POST /attest`, `/provision`, `/release`). It runs where
 * the owner is, so whoever carries the requests sees only quotes and ciphertext.
 *
 * One agent TD hosts several owners' agents, each named by an agent id. For an agent the
 * gate's quote binds, in its report data:
 *
 *   nonce ‖ SHA-256("mero-agent-gate/attest/v3" ‖ provisioning_pub ‖ len(id) ‖ id ‖ signing_pub ‖ owner_pub)
 *
 * (signing and owner keys 32 zero bytes while unclaimed). [attestAgent] checks that and the
 * quote itself before returning anything; every function that sends refuses an agent whose
 * quote names another owner.
 *
 *   val policy = verifySignedAgentPolicy(policyJson, bundleJson)
 *   val verifier = agentQuoteVerifier(policy, fetchCollateral = { q -> PccsCollateral.fetch(q) })
 *   val owner = generateOwnerKey()   // keep owner.secret: the only key that can change the agent
 *   val agent = attestAgent(gateUrl, "a-1f2e", verifier)
 *   provisionAgent(gateUrl, agent, owner.secret, mapOf("NEAR_AI_API_KEY" to key), verifier)
 */

/** Domain of the report-data binding over an agent's keys (the gate's `ATTEST_DOMAIN`). */
const val AGENT_ATTEST_DOMAIN: String = "mero-agent-gate/attest/v3"

/** HPKE `info` prefix; the recipient's public key and the agent id follow it (`PROVISION_INFO_DOMAIN`). */
const val AGENT_PROVISION_INFO_DOMAIN: String = "mero-agent-gate/provision/v2"

private val AGENT_ID = Regex("^[a-z0-9][a-z0-9-]{0,62}$")
private val SECRET_NAME = Regex("^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$")
private const val MAX_SECRETS = 64
private const val MAX_SECRET_LEN = 64 * 1024
private const val MAX_DETAILS = 200
private val gateJson = Json { ignoreUnknownKeys = true }

/** A gate's refusal, with its `error` code (`not_the_owner`, `no_free_slot`, …) when it gave one. */
class AgentGateException(
    message: String,
    /** The gate's HTTP status; null for a refusal made here. */
    val status: Int? = null,
    val code: String? = null,
) : MeroException(message)

/** An agent's keys, once its quote passed [attestAgent]'s checks. */
data class VerifiedAgent(
    val agentId: String,
    /** Base64; secrets are sealed to it. Valid until the gate restarts. */
    val provisioningPublicKey: String,
    /** Base64 Ed25519 key the agent signs with; null while unclaimed. */
    val signingPublicKey: String? = null,
    /** Base64 X25519 key of the owner that claimed it; null while unclaimed. */
    val ownerPublicKey: String? = null,
)

/** An owner key, made on the device: it never needs to leave the owner. */
class OwnerKey(
    /** 32 raw X25519 bytes. The only key that can provision or release the agent: keep it. */
    val secret: ByteArray,
    /** Base64 of the public half: what the gate's quote names as owner. */
    val publicKey: String,
)

/** What [provisionAgent] did, confirmed by a fresh quote. */
data class ProvisionedAgent(
    val agentId: String,
    val provisioningPublicKey: String,
    val signingPublicKey: String,
    val ownerPublicKey: String,
    /** The names of the secrets written, sorted, as the gate reported them. */
    val written: List<String>,
    /** Whether this claimed the agent for your key. */
    val claimed: Boolean,
)

/** Throws unless [agentId] is `[a-z0-9][a-z0-9-]{0,62}`, the gate's agent ids. */
fun validateAgentId(agentId: String) {
    if (!AGENT_ID.matches(agentId)) {
        throw AgentGateException("Agent id ${com.calimero.mero.tee.jsonString(agentId)} must match [a-z0-9][a-z0-9-]{0,62}")
    }
}

private fun lengthPrefixed(agentId: String): ByteArray {
    val id = agentId.encodeToByteArray()
    return concat(byteArrayOf(id.size.toByte()), id)
}

private fun fromBase64(
    value: String,
    label: String,
    length: Int? = null,
): ByteArray {
    val bytes =
        try {
            Base64.decode(value.trim())
        } catch (_: Exception) {
            throw AgentGateException("$label is not base64")
        }
    if (length != null && bytes.size != length) throw AgentGateException("$label is not $length bytes")
    return bytes
}

/** The 32 bytes after the nonce in a gate's quote for [agentId] (empty: the TD alone). Pinned to the gate's `the_binding_is_pinned`. */
fun agentKeyBinding(
    provisioningPublic: ByteArray,
    agentId: String,
    signingPublic: ByteArray? = null,
    ownerPublic: ByteArray? = null,
): ByteArray =
    sha256(
        concat(
            AGENT_ATTEST_DOMAIN.encodeToByteArray(),
            provisioningPublic,
            lengthPrefixed(agentId),
            signingPublic ?: ByteArray(32),
            ownerPublic ?: ByteArray(32),
        ),
    )

/** HPKE `info` for a message to [agentId] on the gate whose provisioning key is [recipientPublic]. */
fun provisionInfo(
    recipientPublic: ByteArray,
    agentId: String,
): ByteArray = concat(AGENT_PROVISION_INFO_DOMAIN.encodeToByteArray(), recipientPublic, lengthPrefixed(agentId))

/** A fresh owner key, made here. */
fun generateOwnerKey(): OwnerKey {
    val secret = randomBytes(32)
    return OwnerKey(secret, ownerPublicKey(secret))
}

/** The base64 public half of an owner's X25519 [secret]. */
fun ownerPublicKey(secret: ByteArray): String {
    if (secret.size != 32) throw AgentGateException("An owner key is 32 bytes")
    return Base64.toBase64String(X25519KeyPair.of(secret).publicKey)
}

private suspend fun postJson(
    httpClient: OkHttpClient,
    url: String,
    body: JsonObject,
): JsonObject? {
    val response = PlainJsonHttp(httpClient, role = null).execute("POST", url, body.toString())
    val parsed = runCatching { response.body.takeIf { it.isNotEmpty() }?.let { gateJson.parseToJsonElement(it) as? JsonObject } }.getOrNull()
    if (!response.isSuccessful) {
        val error = (parsed?.get("error") as? JsonPrimitive)?.takeIf { it.isString }?.content
        val details = (parsed?.get("details") as? JsonPrimitive)?.content ?: response.body.take(MAX_DETAILS)
        throw AgentGateException("The gate answered ${response.status}${error?.let { " $it" }.orEmpty()}: $details", response.status, error)
    }
    return parsed
}

private fun JsonObject?.text(key: String): String? = (this?.get(key) as? JsonPrimitive)?.takeIf { it.isString }?.content

/**
 * Ask the gate at [gateUrl] for a quote for [agentId] (empty: the TD alone) over a fresh nonce
 * and verify it with [verifier] ([agentQuoteVerifier]): a genuine quote from a measured agent
 * image, for this nonce, committing to the agent id asked about and to the keys the gate names.
 * Returns those keys only once every check has passed. [gateUrl] may be a relay of the gate
 * (mdma's): only verified quotes are trusted.
 *
 * Unlike mero-js, a verifier that returns false (rather than throwing) is a refusal here too.
 */
@Suppress("ThrowsCount")
suspend fun attestAgent(
    gateUrl: String,
    agentId: String = "",
    verifier: VerifyTransportQuote,
    httpClient: OkHttpClient = PlainJsonHttp.defaultClient(),
): VerifiedAgent {
    if (agentId.isNotEmpty()) validateAgentId(agentId)
    val nonce = randomBytes(32)
    val body =
        postJson(
            httpClient,
            "${gateUrl.trimBase()}/attest",
            buildJsonObject {
                put("nonceB64", Base64.toBase64String(nonce))
                if (agentId.isNotEmpty()) put("agentId", agentId)
            },
        )
    val answered = body.text("agentId")
    if (answered.orEmpty() != agentId) {
        val named =
            answered?.let {
                com.calimero.mero.tee
                    .jsonString(it)
            } ?: "undefined"
        throw AgentGateException("Asked about agent ${com.calimero.mero.tee.jsonString(agentId)}, the gate answered for $named")
    }
    val provisioning = fromBase64(body.text("provisioningPublicKeyB64").orEmpty(), "provisioningPublicKeyB64", 32)
    val signing = body.text("signingPublicKeyB64")?.takeIf { it.isNotEmpty() }?.let { fromBase64(it, "signingPublicKeyB64", 32) }
    val owner = body.text("ownerPublicKeyB64")?.takeIf { it.isNotEmpty() }?.let { fromBase64(it, "ownerPublicKeyB64", 32) }
    if ((signing == null) != (owner == null)) throw AgentGateException("A claimed agent has both a signing key and an owner; the gate named one")
    // The id asked about, not the one answered: a quote for another agent fails here.
    val binding = agentKeyBinding(provisioning, agentId, signing, owner)
    val quoteB64 = body.text("quoteB64").orEmpty()
    if (!verifier.verify(TransportQuote(quoteB64, Hex.encode(nonce), Hex.encode(binding)))) {
        throw AgentGateException("The gate's quote did not verify")
    }
    return VerifiedAgent(
        agentId = agentId,
        provisioningPublicKey = Base64.toBase64String(provisioning),
        signingPublicKey = signing?.let(Base64::toBase64String),
        ownerPublicKey = owner?.let(Base64::toBase64String),
    )
}

/** The owner key [ownerSecret] may act on [agent]: it is unclaimed, or claimed by that key. Returns its public half. */
fun checkOwner(
    agent: VerifiedAgent,
    ownerSecret: ByteArray,
): String {
    val mine = ownerPublicKey(ownerSecret)
    if (agent.ownerPublicKey != null && agent.ownerPublicKey != mine) {
        throw AgentGateException(
            "This agent is claimed by another key (${agent.ownerPublicKey}); it is not yours. Send it nothing and do not authorize its signing key",
            code = "not_yours",
        )
    }
    return mine
}

@Suppress("ThrowsCount")
private fun validateSecrets(secrets: Map<String, String>) {
    if (secrets.isEmpty()) throw AgentGateException("There are no secrets to provision")
    if (secrets.size > MAX_SECRETS) throw AgentGateException("At most $MAX_SECRETS secrets at once")
    for ((name, value) in secrets) {
        if (!SECRET_NAME.matches(name)) {
            throw AgentGateException("Secret name ${com.calimero.mero.tee.jsonString(name)} must match [A-Za-z0-9][A-Za-z0-9_.-]{0,63}")
        }
        if (value.encodeToByteArray().size > MAX_SECRET_LEN) throw AgentGateException("Secret $name is over $MAX_SECRET_LEN bytes")
    }
}

private suspend fun sendSealed(
    httpClient: OkHttpClient,
    gateUrl: String,
    path: String,
    agent: VerifiedAgent,
    ownerSecret: ByteArray,
    message: JsonElement,
): JsonObject? {
    if (agent.agentId.isEmpty()) throw AgentGateException("This quote names no agent; attest with the agent id")
    val mine = checkOwner(agent, ownerSecret)
    val recipient = fromBase64(agent.provisioningPublicKey, "provisioningPublicKey", 32)
    val sealed = hpkeSealAuth(recipient, ownerSecret, provisionInfo(recipient, agent.agentId), message.toString().encodeToByteArray())
    return postJson(
        httpClient,
        "${gateUrl.trimBase()}$path",
        buildJsonObject {
            put("agentId", agent.agentId)
            put("provisioningPublicKeyB64", agent.provisioningPublicKey)
            put("senderPublicKeyB64", mine)
            put("encappedKeyB64", Base64.toBase64String(sealed.enc))
            put("ciphertextB64", Base64.toBase64String(sealed.ciphertext))
        },
    )
}

/**
 * Seal [secrets] (name to value; a name replaces what was there, others stay) to [agent] as its
 * owner and send them (`POST /provision`). The first bundle for an unclaimed agent claims it for
 * [ownerSecret]'s key. The gate's answer is not taken as proof: a fresh quote, checked by
 * [verifier] (the one [agent] was attested with), must name your key as owner, and the agent's
 * signing key must not have changed.
 */
@Suppress("LongParameterList", "ThrowsCount")
suspend fun provisionAgent(
    gateUrl: String,
    agent: VerifiedAgent,
    ownerSecret: ByteArray,
    secrets: Map<String, String>,
    verifier: VerifyTransportQuote,
    httpClient: OkHttpClient = PlainJsonHttp.defaultClient(),
): ProvisionedAgent {
    validateSecrets(secrets)
    val message = buildJsonObject { putJsonObject("secrets") { secrets.forEach { (k, v) -> put(k, v) } } }
    val answer = sendSealed(httpClient, gateUrl, "/provision", agent, ownerSecret, message)
    val after = attestAgent(gateUrl, agent.agentId, verifier, httpClient)
    val mine = ownerPublicKey(ownerSecret)
    val signing = after.signingPublicKey
    if (after.ownerPublicKey != mine || signing == null) {
        throw AgentGateException("The gate accepted the secrets, but its quote does not name your key as owner")
    }
    if (agent.signingPublicKey != null && agent.signingPublicKey != signing) throw AgentGateException("The agent's signing key changed while provisioning")
    return ProvisionedAgent(
        agentId = after.agentId,
        provisioningPublicKey = after.provisioningPublicKey,
        signingPublicKey = signing,
        ownerPublicKey = mine,
        written = (answer?.get("written") as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }.orEmpty(),
        claimed = (answer?.get("claimed") as? JsonPrimitive)?.booleanOrNull == true,
    )
}

/**
 * Release [agent] as its owner (`POST /release`): the gate stops it and erases its claim, keys,
 * secrets and state. Checked by a fresh quote that names no owner.
 */
suspend fun releaseAgent(
    gateUrl: String,
    agent: VerifiedAgent,
    ownerSecret: ByteArray,
    verifier: VerifyTransportQuote,
    httpClient: OkHttpClient = PlainJsonHttp.defaultClient(),
) {
    if (agent.ownerPublicKey == null) throw AgentGateException("This agent is not claimed; there is nothing to release")
    sendSealed(httpClient, gateUrl, "/release", agent, ownerSecret, buildJsonObject { put("release", true) })
    val after = attestAgent(gateUrl, agent.agentId, verifier, httpClient)
    if (after.ownerPublicKey != null) throw AgentGateException("The gate says it released the agent, but its quote still names an owner")
}
