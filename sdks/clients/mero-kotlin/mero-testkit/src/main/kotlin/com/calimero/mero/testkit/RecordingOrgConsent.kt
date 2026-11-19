package com.calimero.mero.testkit

import com.calimero.mero.ConsentRequest
import com.calimero.mero.ConsentResult
import com.calimero.mero.app.OrgConsentApprover
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject

/**
 * An [OrgConsentApprover] with no consent page: answers every request with [status], and an
 * approved one with the outcome its kind makes ([orgId], [inviteToken], [joinedOrg]). Records what
 * it was asked in [requests], each built for [returnTo]. (Swift tests' `RecordingConsent`.)
 */
class RecordingOrgConsent(
    /** `approved`, `declined` or `pending`. */
    val status: String = "approved",
    val returnTo: String = RETURN_TO,
    val orgId: String = "org_consent",
    val inviteToken: String = "inv_consent",
    val joinedOrg: String = "org_joined",
    /** The request id every answer names. */
    val requestId: String = "r1",
    /** Replaces an approval's outcome (`"null"`: none), as the node's JSON. */
    val outcome: String? = null,
) : OrgConsentApprover {
    private val asked = java.util.Collections.synchronizedList(mutableListOf<ConsentRequest>())

    /** Every request asked, in order. */
    val requests: List<ConsentRequest> get() = asked.toList()

    override suspend fun decide(request: (returnTo: String) -> ConsentRequest): ConsentResult {
        val made = request(returnTo)
        asked += made
        val outcome =
            when (made) {
                is ConsentRequest.CreateOrg -> """{"orgId":"$orgId"}"""
                is ConsentRequest.CreateInvite -> """{"token":"$inviteToken"}"""
                is ConsentRequest.AcceptInvite -> """{"org":"$joinedOrg"}"""
                else -> "null"
            }
        val body = if (status == "approved") this.outcome ?: outcome else "null"
        val text = """{"requestId":"$requestId","kind":"${made.kind}","status":"$status","outcome":$body}"""
        return ConsentResult.fromJson(Json.parseToJsonElement(text) as JsonObject)
    }

    companion object {
        const val RETURN_TO: String = "https://chat.test/1.0.0/?native-consent=1"
    }
}
