package com.calimero.mero.invite

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class InviteLinkTest {
    @Test
    fun `builds the canonical shareable link`() {
        assertEquals(
            "https://links.calimero.network/com.calimero.mero-tag/join?invitation=TOKEN",
            InviteLink.invitation(token = "TOKEN", slug = "com.calimero.mero-tag"),
        )
    }

    @Test
    fun `percent encodes the token`() {
        val link = InviteLink.invitation(token = "a b+c", slug = "com.calimero.mero-tag")
        assertFalse(link.contains("a b"))
        assertTrue(link.contains("a%20b"))
    }

    @Test
    fun `reads the token back out of its own link`() {
        val link = InviteLink.invitation(token = "ROUNDTRIP", slug = "com.calimero.mero-tag")
        assertEquals("ROUNDTRIP", InviteLink.tokenFromPasted(link))
    }

    @Test
    fun `reads a token from a calimero scheme link`() {
        // The dotted slug is why the query is split by hand: URI host parsing on
        // a non-special scheme mangles `com.calimero.mero-tag`.
        val pasted = "calimero://com.calimero.mero-tag/join?invitation=DEEPLINK"
        assertEquals("DEEPLINK", InviteLink.tokenFromPasted(pasted))
    }

    @Test
    fun `accepts a bare token`() {
        assertEquals("JUSTATOKEN", InviteLink.tokenFromPasted("  JUSTATOKEN  "))
    }

    @Test
    fun `decodes percent encoding on the way back`() {
        val pasted = "https://links.calimero.network/com.calimero.mero-tag/join?invitation=a%20b"
        assertEquals("a b", InviteLink.tokenFromPasted(pasted))
    }

    @Test
    fun `survives extra query parameters`() {
        val pasted = "https://links.calimero.network/com.calimero.mero-tag/join?ref=x&invitation=TOK&y=2"
        assertEquals("TOK", InviteLink.tokenFromPasted(pasted))
    }

    @Test
    fun `returns null only for blank input`() {
        assertNull(InviteLink.tokenFromPasted("   "))
        assertNotNull(InviteLink.tokenFromPasted("anything else"))
    }

    @Test
    fun `a link with no invitation parameter comes back unchanged`() {
        val pasted = "https://links.calimero.network/com.calimero.mero-tag/join?other=1"
        assertEquals(pasted, InviteLink.tokenFromPasted(pasted))
    }

    @Test
    fun `a round trip through the codec and the link survives`() {
        val json = """{"invitation":{"group_id":"abc"},"__name":"Space"}"""
        val link = InviteLink.invitation(InviteCodec.encode(json), "com.calimero.mero-tag")
        val token = InviteLink.tokenFromPasted(link)
        assertNotNull(token)
        assertEquals(json, InviteCodec.decode(token!!))
    }

    // ---- expectedSlug ----------------------------------------------------------------------

    @Test
    fun `accepts a link carrying the expected slug`() {
        val link = InviteLink.invitation("MINE", "com.calimero.mero-ar")
        assertEquals("MINE", InviteLink.tokenFromPasted(link, expectedSlug = "com.calimero.mero-ar"))
    }

    @Test
    fun `rejects a link carrying another app's slug`() {
        // The payload shape is shared across the fleet, so another app's invitation decodes fine;
        // redeeming it would join a namespace belonging to that app.
        val link = InviteLink.invitation("THEIRS", "com.calimero.merostream")
        assertNull(InviteLink.tokenFromPasted(link, expectedSlug = "com.calimero.mero-ar"))
    }

    @Test
    fun `rejects a foreign slug on the custom scheme too`() {
        val link = "calimero://com.calimero.merostream/join?invitation=THEIRS"
        assertNull(InviteLink.tokenFromPasted(link, expectedSlug = "com.calimero.mero-ar"))
    }

    @Test
    fun `the slug is matched as a whole segment, not a substring`() {
        val link = InviteLink.invitation("T", "com.calimero.mero-ar")
        assertNull(InviteLink.tokenFromPasted(link, expectedSlug = "com.calimero.mero"))
        assertEquals("T", InviteLink.tokenFromPasted(link.uppercase().replace("INVITATION=T", "invitation=T"), "com.calimero.mero-ar"))
    }

    @Test
    fun `a bare token is accepted even with an expected slug`() {
        assertEquals("BARE", InviteLink.tokenFromPasted("BARE", expectedSlug = "com.calimero.mero-ar"))
    }

    @Test
    fun `omitting the expected slug keeps the old behaviour`() {
        val link = InviteLink.invitation("ANY", "com.calimero.merostream")
        assertEquals("ANY", InviteLink.tokenFromPasted(link))
    }
}
