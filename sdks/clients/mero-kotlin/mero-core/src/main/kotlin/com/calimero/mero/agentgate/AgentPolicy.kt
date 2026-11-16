package com.calimero.mero.agentgate

import com.calimero.mero.tee.Dcap
import com.calimero.mero.tee.DcapCollateral
import com.calimero.mero.tee.DcapVerify
import com.calimero.mero.tee.PRODUCTION_PROFILE
import com.calimero.mero.tee.QuoteVerifier
import com.calimero.mero.tee.QuoteVerifierOptions
import com.calimero.mero.tee.SignerIdentity
import com.calimero.mero.tee.Sigstore
import com.calimero.mero.tee.TCB_STATUSES
import com.calimero.mero.tee.TeeTrustException
import com.calimero.mero.tee.createQuoteVerifier
import com.calimero.mero.tee.jsonString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.intOrNull

/*
 * Which agent images to trust: a mero-kms release's signed agent allowlist,
 * `kms-agent-attestation-policy.<profile>.json`, and a quote verifier built from it — port of
 * mero-js `src/agent-gate/policy.ts`.
 *
 * The KMS release workflow measures the release's agent image and signs its allowlist with
 * cosign keyless, beside the rest of the release. The allowlist can therefore come from
 * anywhere (a release asset, mdma's mirror, a file in the app): [verifySignedAgentPolicy]
 * checks the signature against the Sigstore trust root embedded in this SDK and refuses
 * anything else.
 */

/** The workflow that signs every mero-kms release, agent allowlists included. */
val KMS_RELEASE_SIGNER: SignerIdentity =
    SignerIdentity(
        identity = "https://github.com/calimero-network/mero-tee/.github/workflows/release-kms.yaml@refs/heads/master",
        issuer = "https://token.actions.githubusercontent.com",
    )

/** An agent allowlist, as the KMS release publishes it. Every list must be non-empty. */
data class AgentPolicy(
    val schemaVersion: Int? = null,
    val role: String? = null,
    val profile: String? = null,
    /** The mero-kms release it belongs to. */
    val version: String? = null,
    val allowedTcbStatuses: List<String>,
    val allowedMrtd: List<String>,
    val allowedRtmr0: List<String>,
    val allowedRtmr1: List<String>,
    val allowedRtmr2: List<String>,
    val allowedRtmr3: List<String>,
) {
    companion object {
        /** Read the policy's JSON (`allowed_mrtd`, …); a missing list reads as empty. */
        fun fromJson(o: JsonObject): AgentPolicy {
            fun list(key: String) = (o[key] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }.orEmpty()

            fun text(key: String) = (o[key] as? JsonPrimitive)?.takeIf { it.isString }?.content
            return AgentPolicy(
                schemaVersion = (o["schema_version"] as? JsonPrimitive)?.intOrNull,
                role = text("role"),
                profile = text("profile"),
                version = text("version"),
                allowedTcbStatuses = list("allowed_tcb_statuses"),
                allowedMrtd = list("allowed_mrtd"),
                allowedRtmr0 = list("allowed_rtmr0"),
                allowedRtmr1 = list("allowed_rtmr1"),
                allowedRtmr2 = list("allowed_rtmr2"),
                allowedRtmr3 = list("allowed_rtmr3"),
            )
        }
    }
}

private val policyJson = Json { ignoreUnknownKeys = true }

/**
 * Verify an agent allowlist against its cosign [bundle], as `cosign verify-blob` would, and
 * return it parsed. Throws [TeeTrustException] unless it was signed by the KMS release workflow
 * ([identity], [issuer]), it is an agent allowlist (`role: "agent"`) for [profile], every list
 * in it is non-empty, and [expectedVersion], when given, is its `version`.
 */
@Suppress("ThrowsCount")
fun verifySignedAgentPolicy(
    policy: ByteArray,
    bundle: Any,
    profile: String = PRODUCTION_PROFILE,
    expectedVersion: String? = null,
    identity: String? = null,
    issuer: String? = null,
): AgentPolicy {
    Sigstore.verifyCosignBundle(policy, bundle, SignerIdentity(identity ?: KMS_RELEASE_SIGNER.identity, issuer ?: KMS_RELEASE_SIGNER.issuer))
    val parsed =
        try {
            policyJson.parseToJsonElement(String(policy, Charsets.UTF_8)) as? JsonObject
        } catch (_: Exception) {
            throw TeeTrustException("The signed agent policy is not JSON")
        }
    val role = parsed?.get("role")
    if (parsed == null || (role as? JsonPrimitive)?.takeIf { it.isString }?.content != "agent") {
        throw TeeTrustException("The signed file is a ${role?.toString() ?: "undefined"} policy, not an agent one")
    }
    val read = AgentPolicy.fromJson(parsed)
    if (read.profile != profile) {
        throw TeeTrustException("The signed agent policy is for the ${read.profile?.let(::jsonString) ?: "undefined"} profile, not $profile")
    }
    if (expectedVersion != null && read.version != expectedVersion) {
        throw TeeTrustException("The signed agent policy is release ${read.version?.let(::jsonString) ?: "undefined"}, not $expectedVersion")
    }
    checkLists(read)
    return read
}

/** [verifySignedAgentPolicy] of the file as text (its UTF-8 bytes are what was signed). */
fun verifySignedAgentPolicy(
    policy: String,
    bundle: Any,
    profile: String = PRODUCTION_PROFILE,
    expectedVersion: String? = null,
    identity: String? = null,
    issuer: String? = null,
): AgentPolicy = verifySignedAgentPolicy(policy.encodeToByteArray(), bundle, profile, expectedVersion, identity, issuer)

private fun checkLists(policy: AgentPolicy) {
    val lists =
        listOf(
            "allowed_tcb_statuses" to policy.allowedTcbStatuses,
            "allowed_mrtd" to policy.allowedMrtd,
            "allowed_rtmr0" to policy.allowedRtmr0,
            "allowed_rtmr1" to policy.allowedRtmr1,
            "allowed_rtmr2" to policy.allowedRtmr2,
            "allowed_rtmr3" to policy.allowedRtmr3,
        )
    // As the gate's own verifier: an empty list accepts nothing.
    lists.firstOrNull { it.second.isEmpty() }?.let { throw TeeTrustException("The agent policy's ${it.first} is empty, so no agent would be accepted") }
}

private fun tcbStatus(value: String): String =
    TCB_STATUSES.firstOrNull { it.equals(value.trim(), ignoreCase = true) }
        ?: throw TeeTrustException("The agent policy allows an unknown TCB status ${jsonString(value)}")

private val HEX_PREFIX = Regex("^0x", RegexOption.IGNORE_CASE)

private fun register(value: String): String = value.trim().replace(HEX_PREFIX, "")

/**
 * A verifier for [attestAgent] that accepts a quote only from an agent image in [policy]: its
 * signature and collateral verify, its TCB status is one the policy allows (never `Revoked`),
 * it is not a debug TD, and each of MRTD and RTMR0-3 is in the policy's list. Mock quotes never
 * pass. The gate sends no collateral, so [fetchCollateral] gets it — for example
 * [com.calimero.mero.tee.PccsCollateral.fetch].
 */
fun agentQuoteVerifier(
    policy: AgentPolicy,
    fetchCollateral: suspend (ByteArray) -> DcapCollateral,
    dcapVerify: DcapVerify = Dcap,
    now: () -> Long = System::currentTimeMillis,
): QuoteVerifier {
    checkLists(policy)
    return createQuoteVerifier(
        QuoteVerifierOptions(
            dcapVerify = dcapVerify,
            fetchCollateral = fetchCollateral,
            now = now,
            allowedTcbStatuses = policy.allowedTcbStatuses.map(::tcbStatus),
            allowedMrtd = policy.allowedMrtd.map(::register),
            allowedRtmr0 = policy.allowedRtmr0.map(::register),
            allowedRtmr1 = policy.allowedRtmr1.map(::register),
            allowedRtmr2 = policy.allowedRtmr2.map(::register),
            allowedRtmr3 = policy.allowedRtmr3.map(::register),
        ),
    )
}
