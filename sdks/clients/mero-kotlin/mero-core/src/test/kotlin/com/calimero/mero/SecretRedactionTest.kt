package com.calimero.mero

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** What holds a credential never prints it: a log line or a crash report must not hand it out. */
class SecretRedactionTest {
    @Test
    fun `a session prints neither of its tokens`() {
        val text = CalimeroSession("acc-1", "v2.session-secret", 42, "rt_refresh-secret").toString()
        assertTrue(text, "acc-1" in text)
        assertFalse(text, "session-secret" in text)
        assertFalse(text, "refresh-secret" in text)
    }

    @Test
    fun `a new API key prints its id, not the key`() {
        val text = ApiKeyCreated("key-1", "cal_api-secret").toString()
        assertTrue(text, "key-1" in text)
        assertFalse(text, "api-secret" in text)
    }
}
