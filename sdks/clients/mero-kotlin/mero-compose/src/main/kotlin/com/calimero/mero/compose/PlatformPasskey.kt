package com.calimero.mero.compose

import android.content.Context
import androidx.credentials.CreatePublicKeyCredentialRequest
import androidx.credentials.CreatePublicKeyCredentialResponse
import androidx.credentials.CredentialManager
import androidx.credentials.GetCredentialRequest
import androidx.credentials.GetPublicKeyCredentialOption
import androidx.credentials.PublicKeyCredential
import androidx.credentials.exceptions.CreateCredentialCancellationException
import androidx.credentials.exceptions.GetCredentialCancellationException
import com.calimero.mero.AssertionJSON
import com.calimero.mero.PasskeyCancelled
import com.calimero.mero.PasskeyProvider
import com.calimero.mero.RegistrationJSON
import com.calimero.mero.b64url
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * The device's passkeys, through Android's Credential Manager (Google Password Manager, or any
 * provider the person set): the Android `browserPasskey()` of mero-js. (Swift `PlatformPasskey`.)
 *
 * [context] must be an Activity: the Credential Manager shows its sheet over it.
 *
 * A passkey is bound to the node's relying-party id (`rpId`, the cluster's domain), and Android
 * offers it to an app only when that domain's `/.well-known/assetlinks.json` names the app with
 * the `delegate_permission/common.get_login_creds` relation. A development cluster on `localhost`
 * has no such file: sign in there with a [com.calimero.mero.SoftPasskey].
 *
 * A dismissed sheet throws [PasskeyCancelled].
 */
class PlatformPasskey(
    private val context: Context,
    private val manager: CredentialManager = CredentialManager.create(context),
) : PasskeyProvider {
    override suspend fun create(options: JsonObject): RegistrationJSON {
        val request = JsonObject(options + ("attestation" to (options["attestation"] ?: JsonPrimitive("none"))))
        val created =
            try {
                manager.createCredential(context, CreatePublicKeyCredentialRequest(request.toString()))
            } catch (e: CreateCredentialCancellationException) {
                throw PasskeyCancelled("passkey creation was cancelled", e)
            }
        val response = (created as? CreatePublicKeyCredentialResponse)?.registrationResponseJson ?: error("the credential provider answered no passkey")
        val r = responseOf(response)
        return RegistrationJSON(attestationObject = r.field("attestationObject"), clientDataJson = r.field("clientDataJSON"))
    }

    override suspend fun get(
        challenge: ByteArray,
        rpId: String,
        allowCredentials: List<String>?,
    ): AssertionJSON {
        val request =
            buildJsonObject {
                put("challenge", b64url(challenge))
                put("rpId", rpId)
                put("userVerification", "preferred")
                put(
                    "allowCredentials",
                    JsonArray(
                        allowCredentials.orEmpty().map {
                            buildJsonObject {
                                put("type", "public-key")
                                put("id", it)
                            }
                        },
                    ),
                )
            }
        val got =
            try {
                manager.getCredential(context, GetCredentialRequest(listOf(GetPublicKeyCredentialOption(request.toString()))))
            } catch (e: GetCredentialCancellationException) {
                throw PasskeyCancelled("passkey sign-in was cancelled", e)
            }
        val json = (got.credential as? PublicKeyCredential)?.authenticationResponseJson ?: error("the credential provider answered no passkey")
        val whole = Json.parseToJsonElement(json) as JsonObject
        val r = responseOf(json)
        return AssertionJSON(
            credentialId = (whole["rawId"] as? JsonPrimitive)?.content ?: (whole["id"] as JsonPrimitive).content,
            authenticatorData = r.field("authenticatorData"),
            clientDataJson = r.field("clientDataJSON"),
            signature = r.field("signature"),
        )
    }

    private fun responseOf(json: String): JsonObject = (Json.parseToJsonElement(json) as JsonObject)["response"] as? JsonObject ?: error("the passkey response is malformed")

    private fun JsonObject.field(key: String): String = (this[key] as? JsonPrimitive)?.content ?: error("the passkey response has no $key")
}
