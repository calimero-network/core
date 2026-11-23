package com.calimero.mero

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertThrows
import org.junit.Test

/**
 * The vector core's client (`calimero_client::visitor`), mero-js (`client/visitor.test.ts`),
 * mero-py and mero-swift check too: Ed25519 signs deterministically, so the credential is the same
 * byte for byte.
 */
class VisitorTest {
    private val link = "d4205f1ac7bb3fe43278131b6e621ebf952f0a5f51a38505327572d863c46c42"
    private val key = "fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618"
    private val bearer =
        "vis1.pl_token.fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618.1000." +
            "9b5205e4deb4b534a612c3b03be773b60aea16192f2fb6d99496d194f2dd2591" +
            "73173731e56a46ec7b37cc3d2d2482fe7be5d70c0fadda54c22f5ea2b8fa4e09"

    @Test
    fun `names the link and the visitor and signs the credential core expects`() {
        assertEquals(link, publicLinkKey("pl_token"))
        val v = Visitor("pl_token", ByteArray(32) { 9 })
        assertEquals(link, v.link)
        assertEquals(key, v.publicKey)
        assertEquals(bearer, v.bearer(1000))
    }

    @Test
    fun `gives two seeds two keys and one link`() {
        val one = Visitor("pl_token", ByteArray(32) { 1 })
        val two = Visitor("pl_token", ByteArray(32) { 2 })
        assertNotEquals(one.publicKey, two.publicKey)
        assertEquals(one.link, two.link)
        assertEquals(32, newVisitorSeed().size)
        assertNotEquals(newVisitorSeed().toList(), newVisitorSeed().toList())
    }

    @Test
    fun `refuses a seed that is not 32 bytes`() {
        assertThrows(IllegalArgumentException::class.java) { Visitor("pl_token", ByteArray(31)) }
    }
}
