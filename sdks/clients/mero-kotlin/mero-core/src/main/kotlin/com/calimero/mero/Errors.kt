package com.calimero.mero

import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.longOrNull

/*
 * What the cloud node's refusals mean to a caller (mero-js `cloud-v2/errors.ts`).
 *
 * The node answers an error as `{ error: CODE, message, details }` with an HTTP status.
 * [errorFrom] is the one place the client turns that into an exception a caller can switch on;
 * every call, view and event stream goes through it. It matches the status and the code
 * together, never the status alone: the node sends other 409s (`CONFLICT`, `NOT_INSTALLED`) that
 * are not an idempotency mismatch, and other 413s (`TOO_LARGE` from the sealed transport) that are
 * not oversized call arguments. Anything it does not recognise stays a plain [CalimeroError]
 * carrying both.
 *
 * Every typed error extends [CalimeroError], so `catch (e: CalimeroError)` and its [CalimeroError.status]
 * and [CalimeroError.code] keep working for all of them.
 */

/** A refusal the node answered: its HTTP [status], its [code] and its [details]. */
open class CalimeroError(
    val status: Int,
    val code: String,
    /** The node's message, without the status and code. */
    val nodeMessage: String,
    val details: JsonObject = JsonObject(emptyMap()),
    cause: Throwable? = null,
) : Exception("$status $code: $nodeMessage", cause)

/**
 * `401 UNAUTHENTICATED` that refreshing could not answer: sign in (again) and repeat the request.
 *
 * A session token is signed by the node that issued it, and other nodes take it only while they
 * can vouch for that node's signing key: a `401` often means only that the issuer was replaced or
 * cannot be reached. The client answers that itself when it holds a refresh token: it refreshes
 * once (any node takes the refresh) and repeats the request. This reaches a caller when there is
 * no refresh token (an API key, say), or the refresh was refused.
 */
class LoginRequired(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(401, "UNAUTHENTICATED", message, details)

/**
 * `421 WRONG_NODE`: this node runs a mero-tee release the account's owners have not approved, so
 * it neither issues nor takes the account's sessions. The client moves to the next node it was
 * given ([CalimeroClientOptions.nodes]) and repeats the request once by itself (a refresh: once
 * per node). This reaches a caller when it knows no other node, when the next one refused too, or
 * after a sign-in (which needs the passkey again: repeat it, and it goes to the next node).
 */
class WrongNode(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(421, "WRONG_NODE", message, details)

/**
 * `421 NOT_LEADER`: the request runs on a group's leader only (cluster membership and replica
 * moves, operator routes), and this node is not it. [leaderUrl] is where to send it, when the node
 * knows. Not a move to another node of the client: the client does not repeat it by itself
 * (mero-js leaves it a plain `CalimeroError`; this only names it).
 */
class NotLeader(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(421, "NOT_LEADER", message, details) {
    /** The leader's URL, when the node named it (`details.leaderUrl`). */
    val leaderUrl: String? = details.string("leaderUrl")
}

/**
 * `404 NOT_FOUND`: what the request names does not exist for this caller. For an app this does not
 * say whether it exists: an account the app does not admit gets the same answer as a missing app.
 */
class NotFound(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(404, "NOT_FOUND", message, details)

/**
 * `412 CONFIRM_REQUIRED`: the call needs the account holder's passkey over a challenge in
 * [details]. The `*Confirmed` methods answer it themselves; it reaches a caller only from a method
 * that was given no passkey.
 */
class ConfirmRequired(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(412, "CONFIRM_REQUIRED", message, details)

/** The user declined the passkey prompt a confirmation asked for. Nothing was sent. */
class ConfirmCancelled(
    cause: Throwable? = null,
) : Exception("the passkey confirmation was cancelled", cause)

/**
 * `428 OPERATOR_SIGNATURE_REQUIRED`: an operator decision (`/ops/...`, or a cluster owner's grant)
 * needs the operator key's Ed25519 signature over the statement in [details]: [message] (the exact
 * bytes, base64) and its [nonce]. The generated API answers it itself when given the operator's
 * signer (`CalimeroClient.api(operator = ...)`); it reaches a caller only from a call that was given
 * none.
 */
class OperatorSignatureRequired(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(428, "OPERATOR_SIGNATURE_REQUIRED", message, details) {
    /** The statement's nonce, sent back beside the signature. */
    val nonce: String = details.string("nonce").orEmpty()

    /** The exact bytes to sign, base64 (`details.message`). */
    val statement: String = details.string("message").orEmpty()
}

/** `413 ARGS_TOO_LARGE`: the call's arguments are over the node's limit; it did not run. */
class ArgsTooLarge(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(413, "ARGS_TOO_LARGE", message, details)

/** `409 IDEMPOTENCY_MISMATCH`: the idempotency key was used before with different arguments. */
class IdempotencyMismatch(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(409, "IDEMPOTENCY_MISMATCH", message, details)

/**
 * `409 RELEASE_DEADLINE`: a mero-tee release's deadline has passed and the set that governs this
 * work has not approved it or a newer one, so new work (creating an app, installing a release,
 * upgrading an app) is refused. Apps already running keep running. Approving [release] lifts it.
 */
class ReleaseDeadlineError(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(409, "RELEASE_DEADLINE", message, details) {
    /** The release to approve. */
    val release: String = details.string("release").orEmpty()

    /** Its deadline, RFC 3339 (`""` when the node did not say). */
    val after: String = details.string("after").orEmpty()
}

/**
 * `409 CONSENT_STALE`: what a consent page showed changed before the person approved. Nothing was
 * approved; ask for a new consent request so they see the new facts.
 */
class ConsentStale(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(409, "CONSENT_STALE", message, details)

/**
 * `403 ORIGIN_NOT_REGISTERED`: a sign-up or sign-in named a web origin ([SignInOptions.origin])
 * the cluster does not know: neither one of its own nor one a package's manager registered
 * ([CalimeroClient.addPackageOrigin]). [origin] is the origin asked for and [package] the package
 * it was expected to be registered for, when known (`""` otherwise).
 */
class OriginNotRegistered(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(403, "ORIGIN_NOT_REGISTERED", message, details) {
    val origin: String = details.string("origin").orEmpty()
    val `package`: String = details.string("package").orEmpty()
}

/**
 * `400 BAD_ORIGIN` or `400 BAD_RP_ID`: the node does not take the origin a package's manager tried
 * to register (a bare web origin, `scheme://host[:port]`), or the relying-party id given for it
 * (which must fit the origin's host). [code] says which.
 */
class BadOrigin(
    code: String,
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(400, code, message, details)

/** `409 ORIGIN_TAKEN`: the origin is registered already (for this package or another), or is one of the cluster's own. */
class OriginTaken(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(409, "ORIGIN_TAKEN", message, details)

/**
 * `429 QUOTA_EXCEEDED`: the caller holds as many of something as it may (registered origins, a
 * component app's blobs, an organization's sealed-file bytes, ...).
 */
class QuotaExceeded(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(429, "QUOTA_EXCEEDED", message, details)

/** Why a sign-up was refused ([SignUpRefused.reason]), read from what the node said. */
enum class SignUpRefusal(
    /** mero-js's name for it (`invite-only`, ...). */
    val wire: String,
) {
    /** The cluster takes new accounts with an invite only. */
    InviteOnly("invite-only"),

    /** Only a cluster sign-up invite (the operator's or the owner's) admits. */
    Closed("closed"),

    /** The invite is not one the cluster knows (mistyped, revoked, or used up). */
    UnknownInvite("unknown-invite"),

    /** The invite is past its expiry, or has no use left. */
    ExpiredInvite("expired-invite"),

    /** The organization the invite was for was deleted. */
    OrgGone("org-gone"),

    /** Anything else the node said. */
    Other("other"),
    ;

    internal companion object {
        fun of(message: String): SignUpRefusal =
            when {
                "invite only" in message -> InviteOnly
                "pre-authorized" in message || "sign-up invite" in message -> Closed
                "unknown invite" in message -> UnknownInvite
                "invite has expired" in message -> ExpiredInvite
                "organization is gone" in message -> OrgGone
                else -> Other
            }
    }
}

/**
 * `403 SIGN_UP_REFUSED`: the cluster's sign-up policy ([CalimeroClient.signUpPolicy]) does not
 * take this sign-up, or the invite is unknown, expired or used. Answered by `register/begin`
 * before any passkey is made, and by `register/finish` before its challenge is spent. [reason]
 * says why, so a screen can say "invite only" instead of the node's sentence.
 */
class SignUpRefused(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(403, "SIGN_UP_REFUSED", message, details) {
    val reason: SignUpRefusal = SignUpRefusal.of(message)
}

/**
 * `409 RECOVERY_NOT_READY`: an organization's recovery may finish only once its 72-hour delay has
 * passed. [readyAt] (Unix seconds) is when it may; `0` when the node did not say.
 */
class RecoveryNotReady(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(409, "RECOVERY_NOT_READY", message, details) {
    val readyAt: Long = (details["readyAt"] as? JsonPrimitive)?.longOrNull ?: 0
}

/**
 * `422 APP_ERROR`: the app refused the call. [appCode] is the app's declared error (`Taken`,
 * `Forbidden`, ...) and [fields] its named fields. The call was logged, so its idempotency key is
 * spent.
 */
class AppError(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(422, "APP_ERROR", message, details) {
    val appCode: String
    val fields: JsonObject

    init {
        // The node puts them in `details` as `{ code, fields }`; an older node nests the app's
        // error under `details.error`. Positional payloads (`"0"`, `"1"`) are never read as fields.
        val source = if (details.string("code") != null) details else (details["error"] as? JsonObject ?: JsonObject(emptyMap()))
        appCode = source.string("code").orEmpty()
        fields = source["fields"] as? JsonObject ?: JsonObject(emptyMap())
    }
}

/** `422 TRAP`: the run trapped (`OutOfFuel`, `OutOfMemory`, ...). The call was logged. */
class TrapError(
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
) : CalimeroError(422, "TRAP", message, details) {
    val kind: String = details.string("kind").orEmpty()
}

/**
 * Whether a call the node answered 503 may have committed: [No] when the node says it did not
 * (`TIMEOUT`, `MIGRATING`), [Unknown] otherwise (a call that raced a leader change). Never a plain
 * boolean: "unknown" must not read as "no". (mero-js `Committed`, `false | 'unknown'`.)
 */
enum class Committed { No, Unknown }

/**
 * Any `503`, or a connection that failed, after the client's retries with the same idempotency key
 * ran out. [reason] is the node's code in lower case (`timeout`, `migrating`, `unavailable`, ...),
 * or `network` when the node gave no answer at all (code `NETWORK`, [cause] the I/O failure); such
 * a call may have committed, so its [committed] is [Committed.Unknown]. (mero-js `UnavailableError`.)
 */
class UnavailableError(
    code: String,
    message: String,
    details: JsonObject = JsonObject(emptyMap()),
    /** Seconds the node asked to wait, when it said. */
    val retryAfter: Double? = null,
    cause: Throwable? = null,
) : CalimeroError(503, code, message, details, cause) {
    val reason: String = code.lowercase()
    val committed: Committed = if (code == "TIMEOUT" || code == "MIGRATING") Committed.No else Committed.Unknown
}

internal fun JsonObject?.string(key: String): String? = ((this?.get(key)) as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull

private fun retryAfterOf(
    details: JsonObject,
    header: String?,
): Double? =
    listOf((details["retryAfter"] as? JsonPrimitive)?.doubleOrNull, header?.toDoubleOrNull())
        .firstOrNull { it != null && it.isFinite() && it >= 0 }

/**
 * The error for a refusal the node answered with [status] and [body] (its JSON
 * `{ error, message, details }`, or whatever it sent). The one place statuses become typed errors.
 */
@Suppress("CyclomaticComplexMethod")
fun errorFrom(
    status: Int,
    body: JsonElement?,
    retryAfterHeader: String? = null,
): CalimeroError {
    val value = body as? JsonObject ?: JsonObject(emptyMap())
    val code = value.string("error") ?: "ERROR"
    val message = value.string("message").orEmpty()
    val details = value["details"] as? JsonObject ?: JsonObject(emptyMap())
    if (status == 503) return UnavailableError(code, message, details, retryAfterOf(details, retryAfterHeader))
    return when ("$status $code") {
        "401 UNAUTHENTICATED" -> LoginRequired(message, details)
        "421 WRONG_NODE" -> WrongNode(message, details)
        "421 NOT_LEADER" -> NotLeader(message, details)
        "404 NOT_FOUND" -> NotFound(message, details)
        "409 IDEMPOTENCY_MISMATCH" -> IdempotencyMismatch(message, details)
        "409 RELEASE_DEADLINE" -> ReleaseDeadlineError(message, details)
        "409 CONSENT_STALE" -> ConsentStale(message, details)
        "403 ORIGIN_NOT_REGISTERED" -> OriginNotRegistered(message, details)
        "400 BAD_ORIGIN", "400 BAD_RP_ID" -> BadOrigin(code, message, details)
        "409 ORIGIN_TAKEN" -> OriginTaken(message, details)
        "429 QUOTA_EXCEEDED" -> QuotaExceeded(message, details)
        "403 SIGN_UP_REFUSED" -> SignUpRefused(message, details)
        "409 RECOVERY_NOT_READY" -> RecoveryNotReady(message, details)
        "412 CONFIRM_REQUIRED" -> ConfirmRequired(message, details)
        "428 OPERATOR_SIGNATURE_REQUIRED" -> OperatorSignatureRequired(message, details)
        "413 ARGS_TOO_LARGE" -> ArgsTooLarge(message, details)
        "422 APP_ERROR" -> AppError(message, details)
        "422 TRAP" -> TrapError(message, details)
        else -> CalimeroError(status, code, message, details)
    }
}
