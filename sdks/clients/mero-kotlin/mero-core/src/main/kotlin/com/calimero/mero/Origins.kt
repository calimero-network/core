package com.calimero.mero

import kotlinx.serialization.Serializable

// Sign-up policy, invites at sign-up and registered web origins (core 1.0.0-rc.9+; mero-js #326, #329).

/**
 * Where a sign-in says it runs. [origin] (`scheme://host[:port]`) names a web origin a package's
 * manager registered ([CalimeroClient.addPackageOrigin]): the passkey is asked for that origin's
 * relying-party id, as the node answers it, and the session is bound to the origin's package. Null
 * (the default) is the cluster's own origins, which is what a native app wants.
 */
data class SignInOptions(
    val origin: String? = null,
)

/**
 * [SignInOptions] for a sign-up, and the [invite] it is made with: an organization invite's token
 * (the new account joins the organization at the invite's role) or a cluster sign-up invite's. A
 * cluster whose policy is `invite` or `closed` ([CalimeroClient.signUpPolicy]) needs one.
 */
data class SignUpOptions(
    val origin: String? = null,
    val invite: String? = null,
)

/**
 * `GET /sign-up`: who may create an account on the cluster, read without a session, so a screen can
 * say "invite only" before any passkey prompt.
 */
@Serializable
data class SignUpPolicy(
    /** `open` (anyone), `invite` (an organization or sign-up invite), `closed` (a sign-up invite only). */
    val policy: String = "open",
    /** Whether an organization invite lets a new person sign up (and join it). */
    val orgInvites: Boolean = true,
    /** Whether a cluster sign-up invite does (always). */
    val signUpInvites: Boolean = true,
    /** Whether a sign-up without an invite is refused. */
    val inviteRequired: Boolean = false,
)

/** One web origin registered for a package (`GET /package-origins/{package}`). */
@Serializable
data class PackageOrigin(
    /** `scheme://host[:port]`. */
    val origin: String,
    /** The WebAuthn relying-party id passkeys made there are bound to. */
    val rpId: String = "",
    /** The account that registered it. */
    val addedBy: String = "",
    /** When, RFC 3339 (as core writes it). */
    val addedAt: String = "",
)

/** What registering an origin answers. */
@Serializable
data class PackageOriginAdded(
    val `package`: String,
    val origin: String,
    val rpId: String = "",
    val addedBy: String = "",
    val addedAt: String = "",
)

/** What removing an origin answers. */
@Serializable
data class PackageOriginRemoved(
    val `package`: String,
    val origin: String,
    val removed: Boolean = true,
)
