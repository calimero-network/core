package com.calimero.mero.agentgate

import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.randomBytes
import com.calimero.mero.tee.FakeNet
import com.calimero.mero.tee.TeeFixtures
import com.calimero.mero.tee.VerifyTransportQuote
import com.calimero.mero.tee.X25519KeyPair
import com.calimero.mero.tee.assertRefuses
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import org.bouncycastle.crypto.hpke.HPKE
import org.bouncycastle.crypto.params.X25519PrivateKeyParameters
import org.bouncycastle.util.encoders.Base64
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import javax.crypto.Cipher
import javax.crypto.spec.IvParameterSpec
import javax.crypto.spec.SecretKeySpec

/** Port of mero-js `src/agent-gate/gate.test.ts` and `chacha20poly1305.test.ts`. */
class AgentGateTest {
    private fun fill(byte: Int) = ByteArray(32) { byte.toByte() }

    private fun b64(bytes: ByteArray) = Base64.toBase64String(bytes)

    private fun unb64(value: String) = Base64.decode(value)

    // ---- the report-data binding -------------------------------------------------------------

    @Test
    fun `the binding is the gate's, byte for byte`() {
        // The vectors mero-agent-gate pins (protocol.rs, the_binding_is_pinned).
        assertEquals("61c86ed9782e4b7f3d2a0d2146f66934ce5a2c99a79be440d8419c301dfeb1f5", Hex.encode(agentKeyBinding(fill(1), "ana", fill(2), fill(4))))
        assertEquals("315a41492f5b7820a5dd9d72c5223a4a5efd7d4c96f9991d23c9c8b54fb18f6f", Hex.encode(agentKeyBinding(fill(1), "")))
    }

    // ---- HPKE with the gate ------------------------------------------------------------------

    @Test
    fun `opens a bundle mero-agent-gate sealed`() {
        val fixture = Json.parseToJsonElement(TeeFixtures.text("agent-gate/rust-sealed-bundle.json")).jsonObject
        val s = { k: String -> unb64(fixture[k]!!.jsonPrimitive.content) }
        val plaintext =
            hpkeOpenAuth(
                s("recipientSecret"),
                s("senderPublic"),
                provisionInfo(s("recipientPublic"), fixture["agentId"]!!.jsonPrimitive.content),
                HpkeSealed(s("enc"), s("ciphertext")),
            )
        assertEquals("""{"secrets":{"NEAR_AI_API_KEY":"sk-rust"}}""", String(plaintext))
    }

    @Test
    fun `seals what the gate opens, and Bouncy Castle's own HPKE agrees`() {
        val recipient = X25519KeyPair.of(fill(0x11))
        val message = """{"secrets":{"NEAR_AI_API_KEY":"sk-kotlin"}}""".encodeToByteArray()
        val info = provisionInfo(recipient.publicKey, "ana")
        val sealed = hpkeSealAuth(recipient.publicKey, fill(0x22), info, message)
        assertEquals(String(message), String(hpkeOpenAuth(fill(0x11), X25519KeyPair.of(fill(0x22)).publicKey, info, sealed)))

        // RFC 9180 Auth mode as Bouncy Castle implements it opens the same message.
        val hpke = HPKE(HPKE.mode_auth, HPKE.kem_X25519_SHA256, HPKE.kdf_HKDF_SHA256, HPKE.aead_CHACHA20_POLY1305)
        val kpR = hpke.deserializePrivateKey(fill(0x11), recipient.publicKey)
        val pkS = hpke.deserializePublicKey(X25519PrivateKeyParameters(fill(0x22), 0).generatePublicKey().encoded)
        val opened = hpke.open(sealed.enc, kpR, info, ByteArray(0), sealed.ciphertext, null, null, pkS)
        assertEquals(String(message), String(opened))
    }

    @Test
    fun `an Auth-mode message opens only under its sender's key`() {
        val recipient = X25519KeyPair.of(fill(0x11))
        val info = provisionInfo(recipient.publicKey, "ana")
        val sealed = hpkeSealAuth(recipient.publicKey, fill(0x22), info, "x".encodeToByteArray(), ephemeralSecret = fill(0x33))
        assertEquals(Hex.encode(X25519KeyPair.of(fill(0x33)).publicKey), Hex.encode(sealed.enc))
        assertRefuses("did not authenticate") { hpkeOpenAuth(fill(0x11), X25519KeyPair.of(fill(0x44)).publicKey, info, sealed) }
        assertRefuses("did not authenticate") { hpkeOpenAuth(fill(0x11), X25519KeyPair.of(fill(0x22)).publicKey, provisionInfo(recipient.publicKey, "bob"), sealed) }
    }

    // ---- ChaCha20-Poly1305 (RFC 8439) ----------------------------------------------------------

    @Test
    fun `matches the AEAD vector of RFC 8439 section 2_8_2`() {
        val key = Hex.decodeUnsized("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f", "key")
        val nonce = Hex.decodeUnsized("070000004041424344454647", "nonce")
        val aad = Hex.decodeUnsized("50515253c0c1c2c3c4c5c6c7", "aad")
        val plaintext =
            "Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.".encodeToByteArray()
        val sealed = chacha20Poly1305Seal(key, nonce, plaintext, aad)
        assertEquals("1ae10b594f09e26a7e902ecbd0600691", Hex.encode(sealed.copyOfRange(sealed.size - 16, sealed.size)))
        assertEquals(String(plaintext), String(chacha20Poly1305Open(key, nonce, sealed, aad)))
    }

    @Test
    fun `agrees with the JDK's implementation on random inputs of every length around a block`() {
        for (length in listOf(0, 1, 15, 16, 17, 63, 64, 65, 200, 4096)) {
            val key = randomBytes(32)
            val nonce = randomBytes(12)
            val aad = randomBytes(length % 37)
            val plaintext = randomBytes(length)
            val jdk = Cipher.getInstance("ChaCha20-Poly1305")
            jdk.init(Cipher.ENCRYPT_MODE, SecretKeySpec(key, "ChaCha20"), IvParameterSpec(nonce))
            jdk.updateAAD(aad)
            assertEquals(Hex.encode(jdk.doFinal(plaintext)), Hex.encode(chacha20Poly1305Seal(key, nonce, plaintext, aad)))
        }
    }

    @Test
    fun `refuses a message altered by one bit`() {
        val key = ByteArray(32) { 7 }
        val nonce = ByteArray(12) { 1 }
        val sealed = chacha20Poly1305Seal(key, nonce, "secret".encodeToByteArray())
        sealed[0] = (sealed[0].toInt() xor 1).toByte()
        assertRefuses("did not authenticate") { chacha20Poly1305Open(key, nonce, sealed) }
    }

    // ---- an agent gate client ----------------------------------------------------------------

    /**
     * mero-agent-gate in memory, as its HTTP tests run it: the quote is the report data itself
     * (an echo quoter), so the verifier below checks the binding the client computed against the
     * one the gate committed to.
     */
    private class FakeGate {
        val provisioningSecret: ByteArray = randomBytes(32)
        val claims = mutableMapOf<String, Triple<ByteArray, ByteArray, MutableMap<String, String>>>()

        /** Answer for this agent id instead of the one asked about. */
        var answerFor: String? = null

        /** Replace the provisioning key the gate names (one the quote does not commit to). */
        var swapProvisioningKey: ByteArray? = null

        val publicKey: ByteArray get() = X25519KeyPair.of(provisioningSecret).publicKey

        val net =
            FakeNet { req, body ->
                val json = Json.parseToJsonElement(String(body)).jsonObject
                try {
                    FakeNet.respond(req, 200, handle(req.url.encodedPath, json).toString())
                } catch (e: IllegalStateException) {
                    val (status, code) = e.message!!.split(" ")
                    FakeNet.respond(req, status.toInt(), """{"error":"$code","details":"$code"}""")
                }
            }

        private fun s(
            o: JsonObject,
            k: String,
        ) = o[k]?.jsonPrimitive?.content

        private fun handle(
            path: String,
            body: JsonObject,
        ): JsonObject {
            val provisioning = publicKey
            if (path == "/attest") {
                val agentId = answerFor ?: s(body, "agentId").orEmpty()
                val claim = claims[agentId]
                val binding = agentKeyBinding(provisioning, agentId, claim?.second, claim?.first)
                val quote = Base64.decode(s(body, "nonceB64")!!) + binding
                return buildJsonObject {
                    put("quoteB64", Base64.toBase64String(quote))
                    put("provisioningPublicKeyB64", Base64.toBase64String(swapProvisioningKey ?: provisioning))
                    if (agentId.isNotEmpty()) put("agentId", agentId)
                    claim?.let {
                        put("signingPublicKeyB64", Base64.toBase64String(it.second))
                        put("ownerPublicKeyB64", Base64.toBase64String(it.first))
                    }
                }
            }
            val agentId = s(body, "agentId")!!
            val sender = Base64.decode(s(body, "senderPublicKeyB64")!!)
            val claim = claims[agentId]
            if (claim != null && !claim.first.contentEquals(sender)) error("403 not_the_owner")
            val message =
                try {
                    val plain =
                        hpkeOpenAuth(
                            provisioningSecret,
                            sender,
                            provisionInfo(provisioning, agentId),
                            HpkeSealed(Base64.decode(s(body, "encappedKeyB64")!!), Base64.decode(s(body, "ciphertextB64")!!)),
                        )
                    Json.parseToJsonElement(String(plain)).jsonObject
                } catch (_: Exception) {
                    error("403 not_from_sender")
                }
            if (path == "/release") {
                if (claim == null) error("404 unknown_agent")
                assertEquals(true, (message["release"] as JsonPrimitive).booleanOrNull)
                claims.remove(agentId)
                return buildJsonObject { put("released", agentId) }
            }
            val now = claim ?: Triple(sender, randomBytes(32), mutableMapOf())
            val secrets = message["secrets"]!!.jsonObject
            secrets.forEach { (k, v) -> now.third[k] = v.jsonPrimitive.content }
            claims[agentId] = now
            return buildJsonObject {
                putJsonArray("written") { secrets.keys.sorted().forEach { add(JsonPrimitive(it)) } }
                put("claimed", claim == null)
            }
        }
    }

    /** Accepts an echo quote exactly when it is the nonce followed by the binding. */
    private val echo =
        VerifyTransportQuote { q ->
            if (Hex.encode(Base64.decode(q.quoteB64)) != q.nonce + q.reportDataSuffix) error("The quote does not commit to this nonce and binding")
            true
        }

    private val gateUrl = "http://gate.test:8090"

    private fun attest(
        gate: FakeGate,
        agentId: String,
    ) = runBlocking { attestAgent(gateUrl, agentId, echo, gate.net.client) }

    private fun provision(
        gate: FakeGate,
        agent: VerifiedAgent,
        owner: OwnerKey,
        secrets: Map<String, String>,
    ) = runBlocking { provisionAgent(gateUrl, agent, owner.secret, secrets, echo, gate.net.client) }

    @Test
    fun `claims an unclaimed agent, and a fresh quote names its owner`() {
        val gate = FakeGate()
        val owner = generateOwnerKey()
        val agent = attest(gate, "a-1")
        assertEquals(VerifiedAgent("a-1", b64(gate.publicKey)), agent)
        val done = provision(gate, agent, owner, mapOf("NEAR_AI_API_KEY" to "sk-1", "MERO_AGENTS_JSON" to "{}"))
        assertTrue(done.claimed)
        assertEquals(listOf("MERO_AGENTS_JSON", "NEAR_AI_API_KEY"), done.written)
        assertEquals(owner.publicKey, done.ownerPublicKey)
        assertEquals("sk-1", gate.claims["a-1"]!!.third["NEAR_AI_API_KEY"])
        assertEquals(listOf("$gateUrl/attest", "$gateUrl/provision", "$gateUrl/attest"), gate.net.requests)
    }

    @Test
    fun `refuses, before sending anything, an agent another key claimed`() {
        val gate = FakeGate()
        val first = generateOwnerKey()
        val agent = attest(gate, "a-1")
        provision(gate, agent, first, mapOf("A" to "1"))

        val second = generateOwnerKey()
        val claimed = attest(gate, "a-1")
        val sentBefore = gate.net.requests.size
        assertRefuses("not yours") { provision(gate, claimed, second, mapOf("A" to "evil")) }
        assertEquals(sentBefore, gate.net.requests.size)
        // Trusting a stale quote instead, the gate refuses it.
        val e = assertRefuses("not_the_owner") { provision(gate, agent, second, mapOf("A" to "evil")) } as AgentGateException
        assertEquals(403, e.status)
        assertEquals("not_the_owner", e.code)
        assertEquals("1", gate.claims["a-1"]!!.third["A"])
    }

    @Test
    fun `refuses a quote for another agent than the one asked about`() {
        val gate = FakeGate().also { it.answerFor = "a-2" }
        assertRefuses("the gate answered for \"a-2\"") { attest(gate, "a-1") }
    }

    @Test
    fun `catches keys the quote does not commit to`() {
        val gate = FakeGate().also { it.swapProvisioningKey = fill(9) }
        assertRefuses("does not commit") { attest(gate, "a-1") }
    }

    @Test
    fun `a verifier that answers false refuses too`() {
        assertRefuses("did not verify") { runBlocking { attestAgent(gateUrl, "a-1", { false }, FakeGate().net.client) } }
    }

    @Test
    fun `releases an agent as its owner, and a fresh quote names no owner`() {
        val gate = FakeGate()
        val owner = generateOwnerKey()
        provision(gate, attest(gate, "a-1"), owner, mapOf("A" to "1"))
        val agent = attest(gate, "a-1")
        runBlocking { releaseAgent(gateUrl, agent, owner.secret, echo, gate.net.client) }
        assertFalse("a-1" in gate.claims)
        assertNull(attest(gate, "a-1").ownerPublicKey)
        assertRefuses("not claimed") { runBlocking { releaseAgent(gateUrl, attest(gate, "a-1"), owner.secret, echo, gate.net.client) } }
    }

    @Test
    fun `checks agent ids and secret names before anything is sent`() {
        val gate = FakeGate()
        assertRefuses("must match") { attest(gate, "A_1") }
        val owner = generateOwnerKey()
        val agent = attest(gate, "a-1")
        assertRefuses("must match") { provision(gate, agent, owner, mapOf("../x" to "1")) }
        assertRefuses("no secrets") { provision(gate, agent, owner, emptyMap()) }
        assertTrue(gate.claims.isEmpty())
        assertRefuses("names no agent") { provision(gate, attest(gate, ""), owner, mapOf("A" to "1")) }
    }

    @Test
    fun `an owner key is 32 bytes, its public half the X25519 one`() {
        val owner = generateOwnerKey()
        assertEquals(owner.publicKey, ownerPublicKey(owner.secret))
        assertEquals(b64(X25519KeyPair.of(fill(5)).publicKey), ownerPublicKey(fill(5)))
        assertRefuses("32 bytes") { ownerPublicKey(ByteArray(31)) }
        assertEquals("mero-agent-gate/attest/v3", AGENT_ATTEST_DOMAIN)
        assertEquals("mero-agent-gate/provision/v2", AGENT_PROVISION_INFO_DOMAIN)
    }
}
