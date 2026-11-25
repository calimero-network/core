package com.calimero.mero

import com.calimero.mero.tee.PRODUCTION_PROFILE
import com.calimero.mero.tee.TeeTrustException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Which image profile a node's quote is judged against (mero-js `trustedProfile`), and what each
 * Calimero Cloud trusts. The quote is then checked against that profile's signed image, as
 * `trustSignedRelease` already does (NodeReleaseTest), so naming another profile only gets a node
 * refused.
 */
class CloudEnvironmentTest {
    private fun doc(profile: String) =
        AttestationDoc(quote = "", transportKey = "", release = AttestedRelease(version = "2.3.109", profile = profile))

    private val staging = CloudEnvironment.STAGING.policy().release!!

    @Test
    fun `production trusts locked-read-only only, whatever the node names`() {
        val production = CloudPolicy.production.release!!
        assertNull(production.profiles)
        assertEquals(PRODUCTION_PROFILE, trustedProfile(doc("debug"), production))
        assertEquals(DEFAULT_MIN_RELEASE, production.minReleaseVersion)
        assertEquals(false, production.allowDebugProfile)
        assertEquals(false, CloudPolicy.production.allowMock)
        assertEquals(DEFAULT_DIRECTORY, CloudEnvironment.PRODUCTION.directory)
    }

    @Test
    fun `staging trusts each of its profiles, the one the node names`() {
        assertEquals(listOf(PRODUCTION_PROFILE, "debug", "debug-read-only"), staging.profiles)
        assertTrue(staging.allowDebugProfile)
        for (profile in staging.profiles!!) assertEquals(profile, trustedProfile(doc(profile), staging))
        assertEquals(staging.profiles, CloudPolicy.staging.release!!.profiles)
        assertEquals(
            "2.3.140",
            CloudEnvironment.STAGING
                .policy("2.3.140")
                .release!!
                .minReleaseVersion,
        )
        assertEquals(false, CloudPolicy.staging.allowMock)
        assertEquals("https://manager.staging.cloud.calimero.network/api/directory", CloudEnvironment.STAGING.directory)
    }

    @Test
    fun `a node naming a profile off the list is refused`() {
        val refused = assertThrows(TeeTrustException::class.java) { trustedProfile(doc("debug-ssh"), staging) }
        assertTrue(refused.message!!.contains("not one of"))
        val onlyDebug = staging.copy(profiles = listOf("debug"))
        assertThrows(TeeTrustException::class.java) { trustedProfile(doc(PRODUCTION_PROFILE), onlyDebug) }
    }

    @Test
    fun `a listed debug profile needs allowDebugProfile, and the list is never empty nor doubled`() {
        val silent = staging.copy(allowDebugProfile = false)
        val refused = assertThrows(TeeTrustException::class.java) { trustedProfile(doc("debug"), silent) }
        assertTrue(refused.message!!.contains("which an operator can read"))
        assertThrows(TeeTrustException::class.java) { trustedProfile(doc(PRODUCTION_PROFILE), staging.copy(profiles = emptyList())) }
        assertThrows(TeeTrustException::class.java) { trustedProfile(doc("debug"), staging.copy(profile = "debug")) }
    }
}
