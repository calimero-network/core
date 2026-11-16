package com.calimero.mero.http

/**
 * Base type of the SDK's own failures outside the node's API: a release or quote that does not
 * verify ([com.calimero.mero.tee.TeeTrustException]), an agent gate's refusal. The node's
 * refusals are [com.calimero.mero.CalimeroError]s.
 */
abstract class MeroException(
    message: String,
    cause: Throwable? = null,
) : Exception(message, cause)

/** A transport-level failure with no HTTP response (DNS, connection reset, timeout, ...). */
class NetworkException(
    message: String,
    cause: Throwable? = null,
) : MeroException(message, cause)
