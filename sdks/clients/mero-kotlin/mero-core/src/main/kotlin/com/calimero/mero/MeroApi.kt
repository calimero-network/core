package com.calimero.mero

import com.calimero.mero.api.apis.AccountsApi
import com.calimero.mero.api.apis.AppsApi
import com.calimero.mero.api.apis.ConsentApi
import com.calimero.mero.api.apis.OpsApi
import com.calimero.mero.api.apis.OrganizationsApi
import com.calimero.mero.api.apis.PackagesApi
import com.calimero.mero.api.apis.PublicApi
import com.calimero.mero.api.apis.SignUpAndDeploymentsApi
import okhttp3.Call

/**
 * The OpenAPI Generator client for core's node API (mero-core/src/generated), one class per tag,
 * over a [CalimeroClient]'s transport: every request goes sealed to the attested node in use, with
 * the client's credential, `401` refresh, `421` failover and the `503` retries of calls and views.
 * A refusal throws the [CalimeroError] it means.
 *
 * App code uses the facade instead (`client.orgs`, `client.apps`, ...: named parameters, no request
 * models); this layer is for what the facade does not say, such as the `WithHttpInfo` variants.
 * Made by [CalimeroClient.api].
 */
class MeroApi internal constructor(
    factory: Call.Factory,
) {
    val accounts = AccountsApi(API_BASE, factory)
    val apps = AppsApi(API_BASE, factory)
    val consent = ConsentApi(API_BASE, factory)
    val ops = OpsApi(API_BASE, factory)
    val organizations = OrganizationsApi(API_BASE, factory)
    val packages = PackagesApi(API_BASE, factory)
    val publicApi = PublicApi(API_BASE, factory)
    val signUpAndDeployments = SignUpAndDeploymentsApi(API_BASE, factory)

    internal companion object {
        /**
         * The base the generated client builds its URLs on. Never reached: the transport sends each
         * request to the node the client uses now, whose URL changes on failover.
         */
        const val API_BASE = "https://node.calimero.invalid"
    }
}

/**
 * How a request answers a step-up: the passkey for a `412`, the operator's key for a `428` (null:
 * it reaches the caller), and the body fields to send as `null` ([Explicit]).
 */
internal data class StepUp(
    val passkey: PasskeyProvider? = null,
    val operator: com.calimero.mero.crypto.DeviceSigner? = null,
    val nulls: Set<String> = emptySet(),
)

/**
 * A request field whose `null` means something (the spec types it `[T, "null"]`): a team's `parent`
 * (`null`: to the top), an organization's recovery `publicKey` (`null`: remove it), a presence
 * `state` (`null`: retract). The facade's parameter for one is an `Explicit<T>?`: leave it null to
 * send nothing, pass `Explicit(value)` to send the value, or `Explicit(null)` to send `null`
 * (`client.orgs.teamUpdate(org = o, team = t, parent = Explicit(null))`).
 */
data class Explicit<out T>(
    val value: T?,
)
