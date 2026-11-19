package com.calimero.mero.app

import com.calimero.mero.AppHost
import com.calimero.mero.CalimeroClient
import com.calimero.mero.CalimeroClientOptions
import com.calimero.mero.ConsentRequest
import com.calimero.mero.ConsentResult
import com.calimero.mero.ConsentStarted
import com.calimero.mero.ServedPageHandoff
import com.calimero.mero.WebAuthenticating
import com.calimero.mero.WebAuthenticationCancelled
import okhttp3.OkHttpClient

/**
 * Asks the person to approve an organization action on the node's consent page and answers the
 * node's decision (calimero-network/core#5014). (Swift `OrgConsentApprover`.)
 *
 * A session from the package's served page ([com.calimero.mero.signInWithServedPage]) is bound to
 * the package, and the node refuses it organization routes (`POST /orgs`,
 * `POST /orgs/{org}/invites`, `POST /invites/{token}/redeem`). [CalimeroAppBackend] given one of
 * these founds, invites and joins through it instead, with the same answers.
 */
interface OrgConsentApprover {
    /** The decision on [request], built for the `returnTo` the page sends the person back to. */
    suspend fun decide(request: (returnTo: String) -> ConsentRequest): ConsentResult
}

/**
 * The person declined an organization action on the consent page, or left it without deciding.
 * Nothing was changed. (Swift `OrgConsentNotApproved`.)
 */
class OrgConsentNotApproved(
    /** The consent kind (`createOrg`, `createInvite`, `acceptInvite`). */
    val kind: String,
    /** `declined`, or `pending` when the person left the page without deciding. */
    val status: String,
) : Exception(if (status == "declined") "Declined. Nothing was changed." else "Not approved. Nothing was changed.") {
    override fun equals(other: Any?): Boolean = other is OrgConsentNotApproved && other.kind == kind && other.status == status

    override fun hashCode(): Int = 31 * kind.hashCode() + status.hashCode()
}

/**
 * [OrgConsentApprover] for a session bound to the package on Calimero Cloud: the consent page opens
 * in [opener] and sends the person back through the package's served page
 * ([ServedPageHandoff.consentReturnTo]), which passes the answer on to [callback]. The decision is
 * read from the node and must be the one on this request ([CalimeroClient.resumeConsent]: a return
 * naming another request throws `CONSENT_REQUEST_MISMATCH`, one whose request is still pending
 * `CONSENT_NOT_DECIDED`). (Swift `ServedPageConsent`.)
 */
class ServedPageConsent(
    val client: CalimeroClient,
    /** The package's frontend origin ([AppHost.origin]). */
    val origin: String,
    /** The app's own link the served page passes the answer on to (`mero-chat://consent`). */
    val callback: String,
    val opener: WebAuthenticating,
    val httpClient: OkHttpClient = CalimeroClientOptions.defaultHttpClient(),
    /**
     * Runs before the page opens, with the node's answer: show its `code`, which the page shows
     * too. Answer `false` not to open it (the decision is then `pending`).
     */
    val confirm: (suspend (ConsentStarted) -> Boolean)? = null,
) : OrgConsentApprover {
    override suspend fun decide(request: (returnTo: String) -> ConsentRequest): ConsentResult {
        val version = AppHost.latestVersion(origin, httpClient)
        val started = client.requestConsent(request(ServedPageHandoff.consentReturnTo(origin, version)))
        CalimeroClient.checkConsentCode(started)
        if (confirm?.invoke(started) == false) return client.consentResult(started.requestId)
        val returned =
            try {
                opener.authenticate(started.url, callback)
            } catch (_: WebAuthenticationCancelled) {
                return client.consentResult(started.requestId)
            }
        return client.resumeConsent(returned, started.requestId) ?: client.consentResult(started.requestId)
    }
}

/**
 * Where organization actions are approved: on Calimero Cloud the node's consent page, returning
 * through [packageName]'s served page ([ServedPageConsent]); null on a development node (a client
 * that allows mock attestation), where the backend calls the node directly. (Swift
 * `MeroClient.servedPageConsent`.)
 *
 * @param callback the app's own link the served page passes the answer on to (`mero-chat://consent`).
 */
@Suppress("LongParameterList")
fun CalimeroClient.servedPageConsent(
    packageName: String,
    callback: String,
    opener: WebAuthenticating,
    cluster: String? = null,
    appsDomain: String = AppHost.DEFAULT_DOMAIN,
    confirm: (suspend (ConsentStarted) -> Boolean)? = null,
): OrgConsentApprover? {
    if (options.policy.allowMock || packageName.isEmpty()) return null
    return ServedPageConsent(this, AppHost.origin(packageName, cluster, appsDomain), callback, opener, options.httpClient, confirm)
}
