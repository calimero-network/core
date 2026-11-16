package com.calimero.mero

/**
 * Marks an API that may change without a major version: today the WebSocket event client
 * ([com.calimero.mero.ws.WsClient], `Mero.ws`), which mero-js also ships as
 * `@experimental`. SSE ([Mero.sse], [Mero.events]) is the recommended event transport.
 *
 * Opt in with `@OptIn(ExperimentalMeroApi::class)`; without it the compiler warns.
 */
@RequiresOptIn(
    message = "Experimental Mero API: the WebSocket event client may change. SSE (Mero.sse / Mero.events) is the recommended transport.",
    level = RequiresOptIn.Level.WARNING,
)
@Retention(AnnotationRetention.BINARY)
@Target(AnnotationTarget.CLASS, AnnotationTarget.PROPERTY, AnnotationTarget.FUNCTION)
annotation class ExperimentalMeroApi
