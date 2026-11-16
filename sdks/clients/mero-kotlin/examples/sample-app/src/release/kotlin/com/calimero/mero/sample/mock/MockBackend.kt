package com.calimero.mero.sample.mock

import com.calimero.mero.app.MeroAppBackend

/** Release builds have no demo: mero-testkit is a debug-only dependency. */
object MockBackend {
    const val AVAILABLE: Boolean = false

    fun backend(): MeroAppBackend = throw UnsupportedOperationException("the in-memory demo is in debug builds only")
}
