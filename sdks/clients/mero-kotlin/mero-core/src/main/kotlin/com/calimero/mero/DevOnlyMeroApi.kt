package com.calimero.mero

/**
 * Marks an API that weakens a trust check and exists for development rigs and tests only: today
 * the relay key verifiers that do not verify the relay's TDX quote
 * ([com.calimero.mero.relay.TlsRelayKeyVerifier], [com.calimero.mero.relay.PinnedRelayKeyVerifier]).
 * A shipped app uses [com.calimero.mero.relay.DcapRelayKeyVerifier]. (Swift marks the same types
 * deprecated.)
 *
 * Opt in with `@OptIn(DevOnlyMeroApi::class)`; without it the compiler warns.
 */
@RequiresOptIn(
    message = "Development and tests only: this weakens a trust check. Use DcapRelayKeyVerifier in a shipped app.",
    level = RequiresOptIn.Level.WARNING,
)
@Retention(AnnotationRetention.BINARY)
@Target(AnnotationTarget.CLASS, AnnotationTarget.PROPERTY, AnnotationTarget.FUNCTION, AnnotationTarget.CONSTRUCTOR)
annotation class DevOnlyMeroApi
