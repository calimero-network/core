package com.calimero.mero.compose

import com.calimero.mero.CONSENT_STORAGE_KEY
import com.calimero.mero.CalimeroClient
import com.calimero.mero.CalimeroError
import com.calimero.mero.CloudPolicy
import com.calimero.mero.storage.MemorySessionStore
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * [MeroAppSession.handle] resumes the consent request it started from the id kept in its store,
 * as mero-js `resumeConsent` does from `sessionStorage`: a return that arrives after the process
 * was killed still finds it, and one that is not its decision leaves it for the real return.
 */
class MeroAppSessionConsentTest {
    private val back = "mero-chat://consent?request_id=%s&result=approved"

    private fun session(store: MemorySessionStore) =
        MeroAppSession(CalimeroClient("http://127.0.0.1:9", CloudPolicy.development), "com.example.chat", "mero-chat://consent", store)

    @Test
    fun `refuses a return for another request than the one kept, and keeps it`() =
        runBlocking {
            val store = MemorySessionStore().apply { setValue(CONSENT_STORAGE_KEY, "1.mine") }
            val s = session(store)
            assertTrue(s.handle(back.format("1.forged")))
            assertEquals(CalimeroClient.CONSENT_REQUEST_MISMATCH, (s.consentError.value as CalimeroError).code)
            assertNull(s.consentResult.value)
            assertEquals("1.mine", store.value(CONSENT_STORAGE_KEY))
        }

    @Test
    fun `refuses a return when no request was started`() =
        runBlocking {
            val store = MemorySessionStore()
            val s = session(store)
            assertTrue(s.handle(back.format("1.any")))
            assertEquals(CalimeroClient.CONSENT_NOT_STARTED, (s.consentError.value as CalimeroError).code)
            assertNull(store.value(CONSENT_STORAGE_KEY))
        }

    @Test
    fun `keeps the request when the node cannot be asked`() =
        runBlocking {
            val store = MemorySessionStore().apply { setValue(CONSENT_STORAGE_KEY, "1.mine") }
            val s = session(store)
            assertTrue(s.handle(back.format("1.mine")))
            assertTrue("${s.consentError.value}", s.consentError.value != null && s.consentError.value !is CalimeroError)
            assertEquals("1.mine", store.value(CONSENT_STORAGE_KEY))
        }

    @Test
    fun `is not a consent return without request_id and result`() =
        runBlocking {
            val store = MemorySessionStore().apply { setValue(CONSENT_STORAGE_KEY, "1.mine") }
            assertEquals(false, session(store).handle("mero-chat://consent?result=approved"))
            assertEquals("1.mine", store.value(CONSENT_STORAGE_KEY))
        }
}
