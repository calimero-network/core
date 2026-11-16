package com.calimero.mero.storage

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import java.security.GeneralSecurityException
import javax.crypto.AEADBadTagException

/**
 * A keyset restored onto a device without its Keystore master key does not decrypt: the file is
 * reset and opened afresh rather than crashing the app at start (SEC-12).
 */
class EncryptedPrefsTest {
    @Test
    fun `a keyset that does not decrypt resets the file and opens again`() {
        var opens = 0
        var resets = 0
        val opened =
            EncryptedPrefs.openOrReset(
                open = {
                    opens++
                    if (resets == 0) throw AEADBadTagException("restored from a backup")
                    "fresh"
                },
                reset = { resets++ },
            )
        assertEquals("fresh", opened)
        assertEquals(2, opens)
        assertEquals(1, resets)
    }

    @Test
    fun `a file that opens is never reset`() {
        var resets = 0
        assertEquals("prefs", EncryptedPrefs.openOrReset(open = { "prefs" }, reset = { resets++ }))
        assertEquals(0, resets)
    }

    @Test
    fun `a failure that survives the reset is thrown`() {
        var resets = 0
        assertThrows(GeneralSecurityException::class.java) {
            EncryptedPrefs.openOrReset(open = { throw GeneralSecurityException("keystore gone") }, reset = { resets++ })
        }
        assertEquals(1, resets)
    }
}
