package com.calimero.mero.app

import com.calimero.mero.invite.InviteCodec
import com.calimero.mero.invite.InviteLink
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put

/**
 * An invite to an organization, as an app passes it around: the node's invite [token] (`inv_…`,
 * from `POST /orgs/{org}/invites`) with the organization it opens. (Swift `OrgInvitation`.)
 *
 * In a link it travels as `base58(deflate(json))` in the `invitation` query parameter, the shape
 * 0.x links had, with the same keys:
 *
 * ```json
 * {"invitation": "inv_…", "namespaceId": "<orgId>", "contextId": "<appId, optional>", "name": "<org name, optional>"}
 * ```
 *
 * `invitation` was a signed group-open invitation object in 0.x; it is now the token string, which
 * is all `POST /invites/{token}/redeem` needs. [decode] refuses a 0.x payload (an object there).
 */
data class OrgInvitation(
    /** The organization it opens (0.x: `namespaceId`). */
    val namespaceId: String,
    /** The node's invite token, `inv_…`. Secret: whoever holds it may redeem it while it has uses left. */
    val token: String,
    /** The app instance to open after joining (0.x: `contextId`), when there is one. */
    val contextId: String? = null,
    /** The organization's name, for the confirmation dialog. Unsigned: a hint only. */
    val name: String? = null,
) {
    /** The payload as JSON. */
    fun toJson(): JsonObject =
        buildJsonObject {
            put("invitation", token)
            put("namespaceId", namespaceId)
            contextId?.let { put("contextId", it) }
            name?.let { put("name", it) }
        }

    /** `base58(deflate(json))`: what goes in a link's `invitation` parameter. */
    fun encoded(): String = InviteCodec.encode(toJson().toString())

    /** The shareable link for the app with package [slug]. */
    fun link(
        slug: String,
        host: String = InviteLink.DEFAULT_HOST,
    ): String = InviteLink.invitation(encoded(), slug, host)

    override fun toString(): String = "OrgInvitation(namespaceId=$namespaceId, contextId=$contextId, name=$name)"

    companion object {
        /** An invitation from a pasted link or code (see [InviteLink.tokenFromPasted]), or null. */
        fun parse(
            input: String,
            expectedSlug: String? = null,
        ): OrgInvitation? = InviteLink.tokenFromPasted(input, expectedSlug)?.let(::decode)

        /** An invitation from its encoded form ([encoded]), or null when it is not one. */
        fun decode(code: String): OrgInvitation? = InviteCodec.decode(code)?.let(::decodeJson)

        /** An invitation from its JSON payload, or null when it is not one (a 0.x payload included). */
        fun decodeJson(payload: String): OrgInvitation? {
            val o =
                runCatching {
                    kotlinx.serialization.json.Json
                        .parseToJsonElement(payload) as? JsonObject
                }.getOrNull() ?: return null

            fun s(key: String) = (o[key] as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull
            val token = s("invitation")?.takeIf { it.isNotBlank() } ?: return null
            val org = s("namespaceId")?.takeIf { it.isNotBlank() } ?: return null
            return OrgInvitation(org, token, s("contextId"), s("name"))
        }
    }
}
