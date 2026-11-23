package com.calimero.mero

import com.calimero.mero.crypto.Ed25519
import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.concat
import com.calimero.mero.crypto.randomBytes
import com.calimero.mero.crypto.sha256

/*
 * An anonymous visitor through a public link (core's `/apps/{app}/public-links`): the credential
 * each request carries to call an instance's `#[app::public]` methods, made of the link's token and
 * the visitor's own Ed25519 key, signed fresh per request. The node names the visitor to the app as
 * `Caller::Visitor { link, key }`. Mirrors core's `calimero_client::visitor` and mero-js's
 * `client/visitor.ts`; `VisitorTest` checks the vector they check.
 */

private const val PUBLIC_LINK_DOMAIN = "calimero.public-link.v1\u0000"
private const val VISITOR_DOMAIN = "calimero.visitor.v1\u0000"

/** A public link token's key (hex): how the node and the app name the link. */
fun publicLinkKey(token: String): String = Hex.encode(Ed25519.publicKey(sha256((PUBLIC_LINK_DOMAIN + token).encodeToByteArray())))

/** A new visitor seed: 32 random bytes. Keep it (in encrypted preferences, say) to stay the same visitor. */
fun newVisitorSeed(): ByteArray = randomBytes(Ed25519.SEED_BYTES)

/**
 * A visitor of one public link: its [token] and the visitor's key, whose seed is [seed] (32 bytes).
 * [CalimeroClient.useVisitor] makes one and signs every request with it.
 */
class Visitor(
    private val token: String,
    seed: ByteArray,
) {
    private val seed: ByteArray

    init {
        require(seed.size == Ed25519.SEED_BYTES) { "a visitor seed is 32 bytes, got ${seed.size}" }
        require(token.isNotEmpty()) { "a public link token is not empty" }
        this.seed = seed.copyOf()
    }

    /** The link's key (hex). */
    val link: String = publicLinkKey(token)

    /** The visitor's public key (64 lowercase hex): how the app names it. */
    val publicKey: String = Hex.encode(Ed25519.publicKey(this.seed))

    /**
     * The bearer credential for a request at [at] (Unix seconds; default now):
     * `vis1.<token>.<key>.<at>.<signature>`, valid within 5 minutes of the node's clock.
     */
    fun bearer(at: Long = System.currentTimeMillis() / 1000): String {
        val message =
            concat(
                (VISITOR_DOMAIN + link).encodeToByteArray(),
                byteArrayOf(0),
                publicKey.encodeToByteArray(),
                byteArrayOf(0),
                at.toString().encodeToByteArray(),
            )
        return "vis1.$token.$publicKey.$at.${Hex.encode(Ed25519.sign(seed, message))}"
    }

    override fun toString(): String = "Visitor(publicKey=$publicKey)"
}
