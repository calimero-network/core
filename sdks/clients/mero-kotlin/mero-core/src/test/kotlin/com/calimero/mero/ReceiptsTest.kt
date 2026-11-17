package com.calimero.mero

import com.calimero.mero.crypto.Ed25519
import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.Jcs
import com.calimero.mero.crypto.base64
import com.calimero.mero.crypto.concat
import com.calimero.mero.crypto.sha256
import com.calimero.mero.tee.TeeTrustException
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import okhttp3.OkHttpClient
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.math.BigInteger

/**
 * mero-js `client/receipts.test.ts`, over core's `receipt_vectors` (`fixtures/receipt-vectors.json`,
 * a copy of `crates/client/tests/fixtures/receipt-vectors.json` made by
 * `scripts/refresh-receipt-vectors.sh`).
 */
@Suppress("LargeClass", "UNCHECKED_CAST")
class ReceiptsTest {
    private val v: JsonObject =
        Json
            .parseToJsonElement(
                checkNotNull(javaClass.classLoader?.getResourceAsStream("fixtures/receipt-vectors.json")).use { it.readBytes().decodeToString() },
            ).jsonObject

    private fun s(
        o: JsonObject,
        key: String,
    ) = o.getValue(key).jsonPrimitive.content

    private val mock = CloudPolicy(allowMock = true)
    private val http = OkHttpClient()
    private val app = "app-1"
    private val run = "run-1"
    private val nonce = Hex.decode(s(v, "nonce"), "nonce", 32)
    private val clusterName = s(v, "cluster")
    private val governance = v.getValue("governance").jsonObject
    private val jobAnswer =
        v
            .getValue("jobReceipt")
            .jsonObject
            .getValue("answer")
            .jsonObject
    private val jobKey = s(v.getValue("jobKeyBindings").jsonArray[0].jsonObject, "jobKey")

    private fun keyOf(node: String) = Hex.decode(s(v.getValue("nodes").jsonObject, node), "key", 32)

    private fun mockQuote(
        n: ByteArray,
        suffix: String,
    ) = base64(concat(MOCK_QUOTE_HEADER.encodeToByteArray(), n, Hex.decode(suffix, "suffix", 32)))

    // ---------------------------------------------------------------- mutable JSON, to change a receipt

    private fun mut(e: JsonElement): Any? =
        when (e) {
            is JsonObject -> e.mapValuesTo(LinkedHashMap()) { mut(it.value) }
            is JsonArray -> e.mapTo(ArrayList()) { mut(it) }
            else -> e
        }

    private fun js(x: Any?): JsonElement =
        when (x) {
            null -> JsonNull
            is JsonElement -> x
            is Map<*, *> -> JsonObject(x.entries.associate { it.key as String to js(it.value) })
            is List<*> -> JsonArray(x.map(::js))
            is String -> JsonPrimitive(x)
            is Number -> JsonPrimitive(x)
            is Boolean -> JsonPrimitive(x)
            else -> error("not JSON: $x")
        }

    private fun Any?.o(key: String) = (this as MutableMap<String, Any?>)[key] as MutableMap<String, Any?>

    private fun Any?.l(key: String) = (this as MutableMap<String, Any?>)[key] as MutableList<Any?>

    private fun Any?.t(key: String) = ((this as MutableMap<String, Any?>)[key] as JsonPrimitive).content

    private fun Any?.set(
        key: String,
        value: Any?,
    ) {
        (this as MutableMap<String, Any?>)[key] = if (value is String || value is Number || value is Boolean) js(value) else value
    }

    private fun goodReceipt() = mut(jobAnswer.getValue("receipt")) as MutableMap<String, Any?>

    private fun loadRun(r: Any?) = r.o("steps").o("load").l("runs")[0] as MutableMap<String, Any?>

    private fun mergeRun(r: Any?) = r.o("steps").o("merge").l("runs")[0] as MutableMap<String, Any?>

    private fun answer(
        base: JsonObject = jobAnswer,
        vararg fields: Pair<String, JsonElement?>,
    ): JsonObject = JsonObject(base.toMutableMap().apply { fields.forEach { (k, x) -> if (x == null) remove(k) else put(k, x) } })

    /** An answer for [receipt], signed by node 1's key (the Rust program's seed), as the node would. */
    private fun signed(
        receipt: Any?,
        node: JsonElement = JsonPrimitive(1),
    ): JsonObject {
        val r = js(receipt)
        val sig = Ed25519.sign(ByteArray(32) { 11 }, receiptMessage(r))
        return JsonObject(mapOf("receipt" to r, "node" to node, "signature" to JsonPrimitive(Hex.encode(sig))))
    }

    /** Re-signs the report of [proof] with the job key (seed 0x41) over the fields it now has, as the job node would. */
    private fun rereport(
        r: Any?,
        proof: MutableMap<String, Any?>,
        reason: String?,
    ) {
        val job = proof.o("job")
        val segments = job.l("segments")
        val message =
            reportMessage(
                ReportFields(
                    assignment = proof.t("assignmentDigest"),
                    app = r.t("app"),
                    run = proof.t("run"),
                    attempt = (proof["attempt"] as JsonPrimitive).long,
                    reason = reason,
                    message = (job["message"] as? JsonPrimitive)?.content ?: "",
                    resultSha256 = job.t("resultSha256"),
                    outputs =
                        proof.l("outputs").mapIndexed { i, o ->
                            JobOutput(o.t("id"), ((o as Map<String, Any?>)["size"] as JsonPrimitive).long, o.t("sha256"), (segments[i] as List<Any?>).map { (it as JsonPrimitive).content })
                        },
                    usage = listOf(proof.o("usage").t("cpuMicros"), proof.o("usage").t("gibMicros"), proof.o("usage").t("fuel")),
                    log =
                        (job["log"] as? MutableMap<String, Any?>)?.let { l ->
                            JobOutput(l.t("id"), (l["size"] as JsonPrimitive).long, l.t("sha256"), l.l("segments").map { (it as JsonPrimitive).content })
                        },
                ),
            )
        proof.set("reportSha256", Hex.encode(sha256(message)))
        proof.set("signature", Hex.encode(Ed25519.sign(ByteArray(32) { 0x41 }, message)))
    }

    // ---------------------------------------------------------------- the stand-in cluster

    private class Replica(
        val node: Long,
        val docNode: Long? = null,
        val bindCluster: String? = null,
        val bindNode: Long? = null,
        val governance: JsonObject? = null,
        val error: String? = null,
        val unbound: Boolean = false,
    )

    /** `apps.attestation` answering with [replicas], quoting over the nonce it was asked with; [patch] changes the answer. */
    private fun cluster(
        vararg replicas: Replica,
        entryCluster: String = clusterName,
        patch: (MutableMap<String, Any?>) -> Unit = {},
    ): suspend (String, String) -> JsonElement =
        { _, asked ->
            val list =
                replicas.map { r ->
                    if (r.error != null) {
                        mapOf("node" to JsonPrimitive(r.node), "error" to JsonPrimitive(r.error))
                    } else {
                        val key = keyOf(r.node.toString())
                        val g = r.governance ?: governance
                        val suffix =
                            if (r.unbound) {
                                Hex.encode(sha256(concat("calimero.tee-attest.receipt-key.v2".encodeToByteArray(), key, sha256(governanceCanonical(g)))))
                            } else {
                                receiptKeyBinding(key, g, r.bindCluster ?: entryCluster, r.bindNode ?: r.node)
                            }
                        mutableMapOf<String, Any?>(
                            "node" to JsonPrimitive(r.node),
                            "attestation" to
                                mutableMapOf<String, Any?>(
                                    "quote" to JsonPrimitive(mockQuote(Hex.decode(asked, "nonce", 32), suffix)),
                                    "transportKey" to JsonPrimitive("00".repeat(32)),
                                    "appHash" to JsonPrimitive("00".repeat(32)),
                                    "nonce" to JsonPrimitive(asked),
                                    "release" to JsonObject(mapOf("version" to JsonPrimitive("2.3.109"), "profile" to JsonPrimitive("locked-read-only"))),
                                    "cluster" to JsonPrimitive(r.bindCluster ?: entryCluster),
                                    "node" to JsonPrimitive(r.docNode ?: r.node),
                                    "mock" to JsonPrimitive(true),
                                    "receiptKey" to JsonPrimitive(Hex.encode(key)),
                                    "governance" to g,
                                ),
                        )
                    }
                }
            val answer = mutableMapOf<String, Any?>("appId" to JsonPrimitive(app), "cluster" to JsonPrimitive(entryCluster), "replicas" to list.toMutableList())
            patch(answer)
            js(answer)
        }

    private val both get() = cluster(Replica(1), Replica(2))

    /** Rejects with a [ReceiptVerificationException] (or the quote check's [TeeTrustException]) whose message matches. */
    private fun refuses(
        message: Regex,
        block: suspend () -> Unit,
    ) {
        val error =
            try {
                runBlocking { block() }
                null
            } catch (e: Exception) {
                e
            } ?: fail("expected a refusal matching $message").let { return }
        assertTrue("not a verification error: $error", error is ReceiptVerificationException || error is TeeTrustException)
        assertTrue("'${error.message}' does not match $message", message.containsMatchIn(error.message.orEmpty()))
    }

    private fun refuses(
        message: String,
        block: suspend () -> Unit,
    ) = refuses(Regex(Regex.escape(message)), block)

    private fun check(
        answer: JsonElement,
        appId: String = app,
        runId: String = run,
    ) = runBlocking { verifyPipelineReceipt(both, appId, runId, answer, mock, http) }

    private fun refusedAs(
        r: Any?,
        message: Regex,
        runId: String = run,
        policy: CloudPolicy = mock,
        replicas: Array<Replica> = arrayOf(Replica(1), Replica(2)),
    ) = refuses(message) { verifyPipelineReceipt(cluster(*replicas), app, runId, signed(r), policy, http) }

    private fun refusedAs(
        r: Any?,
        message: String,
        runId: String = run,
        policy: CloudPolicy = mock,
    ) = refusedAs(r, Regex(Regex.escape(message)), runId, policy)

    // ---------------------------------------------------------------- receiptKeyBinding

    @Test
    fun `receiptKeyBinding matches every vector core produced`() {
        for (b in v.getValue("bindings").jsonArray.map { it.jsonObject }) {
            val g = b.getValue("governance").jsonObject
            assertEquals(s(b, "governanceCanonical"), governanceCanonical(g).decodeToString())
            assertEquals(s(b, "binding"), receiptKeyBinding(Hex.decode(s(b, "receiptKey"), "key", 32), g, s(b, "cluster"), b.getValue("node").jsonPrimitive.long))
        }
    }

    @Test
    fun `governanceCanonical drops defaults and keeps field order whatever the node serialized`() {
        val noisy =
            Json.parseToJsonElement("""{"signUp":"open","outbound":"owner","deployers":"operator","releases":"owner","deployments":"open","appUpgrades":"owner"}""").jsonObject
        assertEquals("""{"releases":"owner","appUpgrades":"owner","outbound":"owner"}""", governanceCanonical(noisy).decodeToString())
        refuses("releases") { governanceCanonical(JsonObject(noisy + ("releases" to JsonPrimitive("nobody")))) }
    }

    @Test
    fun `governanceCanonical is byte-identical to the JSON of every governance the fixtures hold`() {
        for (g in listOf(governance) + v.getValue("bindings").jsonArray.map { it.jsonObject.getValue("governance").jsonObject }) {
            assertEquals(g.toString(), governanceCanonical(g).decodeToString())
        }
    }

    // ---------------------------------------------------------------- verifyReceiptKey

    private val mockDoc = v.getValue("mockDoc").jsonObject

    @Test
    fun `verifyReceiptKey accepts the document core accepts, and answers its key and governance`() =
        runBlocking {
            val got = verifyReceiptKey(mockDoc, nonce, clusterName, 1, mock, http)
            assertEquals(s(v.getValue("nodes").jsonObject, "1"), Hex.encode(got.key))
            assertEquals(Governance(releases = "operator", outbound = "operator", deployments = DeploymentsRule.Granted, deployers = DeployersRule.Owner, signUp = "invite"), got.governance)
        }

    @Test
    fun `verifyReceiptKey refuses the unbound, another node's or cluster's binding, and a mock quote without allowMock`() {
        refuses("does not bind") { verifyReceiptKey(v.getValue("unboundMockDoc").jsonObject, nonce, clusterName, 1, mock, http) }
        refuses("does not bind") { verifyReceiptKey(mockDoc, nonce, clusterName, 2, mock, http) }
        refuses("does not bind") { verifyReceiptKey(mockDoc, nonce, "other", 1, mock, http) }
        refuses("mock quote refused") { verifyReceiptKey(mockDoc, nonce, clusterName, 1, CloudPolicy(), http) }
        refuses("no receipt key") { verifyReceiptKey(JsonObject(mockDoc - "receiptKey"), nonce, clusterName, 1, mock, http) }
        refuses("no governance") { verifyReceiptKey(JsonObject(mockDoc - "governance"), nonce, clusterName, 1, mock, http) }
    }

    // ---------------------------------------------------------------- verifyAppCluster

    @Test
    fun `verifyAppCluster answers each replica key, the shared governance and the cluster`() =
        runBlocking {
            val got = verifyAppCluster(both, app, mock, http)
            assertEquals(mapOf(1L to s(v.getValue("nodes").jsonObject, "1"), 2L to s(v.getValue("nodes").jsonObject, "2")), got.keys.mapValues { Hex.encode(it.value) })
            assertEquals("invite", got.governance.signUp)
            assertEquals(clusterName, got.cluster)
        }

    @Test
    fun `verifyAppCluster refuses what core refuses`() {
        refuses("replica 1") { verifyAppCluster(cluster(Replica(1, bindNode = 2)), app, mock, http) }
        refuses("replica 1") { verifyAppCluster(cluster(Replica(1, bindCluster = "other")), app, mock, http) }
        refuses("answered as node 2") { verifyAppCluster(cluster(Replica(1, docNode = 2)), app, mock, http) }
        val other = JsonObject(governance + ("releases" to JsonPrimitive("owner")))
        refuses("replica 2 attests other governance") { verifyAppCluster(cluster(Replica(1), Replica(2, governance = other)), app, mock, http) }
        refuses("replica 2 gave no attestation: unreachable") { verifyAppCluster(cluster(Replica(1), Replica(2, error = "unreachable")), app, mock, http) }
        refuses("replica 1") { verifyAppCluster(cluster(Replica(1, unbound = true)), app, mock, http) }
        refuses("mock quote refused") { verifyAppCluster(cluster(Replica(1)), app, CloudPolicy(), http) }
        refuses("no replica of app-1 attested") { verifyAppCluster(cluster(), app, mock, http) }
    }

    @Test
    fun `verifyAppCluster refuses a malformed answer`() {
        val replicas = { x: JsonElement -> { _: String, _: String -> JsonObject(mapOf("appId" to JsonPrimitive(app), "cluster" to JsonPrimitive(clusterName), "replicas" to x)) } }
        refuses("malformed attestation") { verifyAppCluster({ a, n -> replicas(JsonPrimitive("x"))(a, n) }, app, mock, http) }
        refuses("neither an attestation nor an error") { verifyAppCluster({ a, n -> replicas(Json.parseToJsonElement("""[{"node":1}]"""))(a, n) }, app, mock, http) }
        refuses("not an object") { verifyAppCluster({ a, n -> replicas(JsonArray(listOf(JsonNull)))(a, n) }, app, mock, http) }
    }

    @Test
    fun `verifyAppCluster returns the governance from its canonical fields only`() =
        runBlocking {
            val c = cluster(Replica(1), patch = { a -> (a.l("replicas")[0].o("attestation")).let { it["governance"] = JsonObject(governance + ("evil" to JsonPrimitive(1))) } })
            assertEquals("invite", verifyAppCluster(c, app, mock, http).governance.signUp)
        }

    @Test
    fun `refuses a replica or document whose node is not a node id, and a padded receipt key`() {
        refuses("not a node id") { verifyAppCluster(cluster(Replica(1), patch = { a -> a.l("replicas")[0].set("node", "1") }), app, mock, http) }
        refuses("not a node id") { verifyAppCluster(cluster(Replica(1), patch = { a -> a.l("replicas")[0].o("attestation").set("node", "1") }), app, mock, http) }
        refuses("not a node id") { verifyAppCluster(cluster(Replica(1), patch = { a -> a.l("replicas")[0].set("node", 9_007_199_254_740_992L) }), app, mock, http) }
        refuses("receipt key") {
            verifyAppCluster(cluster(Replica(1), patch = { a -> a.l("replicas")[0].o("attestation").let { it.set("receiptKey", " " + it.t("receiptKey")) } }), app, mock, http)
        }
    }

    // ---------------------------------------------------------------- verifyPipelineReceipt

    @Test
    fun `accepts the answer core signed under its node key and answers the receipt`() {
        val receipt = check(jobAnswer)
        assertEquals(jobAnswer.getValue("receipt"), receipt.raw)
        assertEquals(2L, receipt.version)
        assertEquals(mapOf(jobKey to 4L), verifyJobReports(receipt))
        val proof = receipt.steps.getValue("load").runs[0] as StepRunProof
        assertEquals("run-1.load.0", proof.run)
        assertEquals(CachedRun(1, true, "07".repeat(32)), receipt.steps.getValue("load").runs[1])
    }

    @Test
    fun `refuses the version 1 receipt core also signs, though its signature holds`() {
        val v1 = v.getValue("receiptAnswer").jsonObject
        refuses("the receipt is version 1") { verifyPipelineReceipt(both, app, run, v1, mock, http) }
        assertEquals(1L, verifyReceiptSignature(v1, keyOf("1")).version)
    }

    @Test
    fun `refuses a receipt not signed by the answering node, an unattested node, and one byte changed`() {
        refuses("not signed by that key") { check(answer(jobAnswer, "node" to JsonPrimitive(2))) }
        refuses("not an attested replica") { verifyPipelineReceipt(cluster(Replica(2)), app, run, jobAnswer, mock, http) }
        val r = goodReceipt().also { it.set("state", "succeedeD") }
        refuses("not signed by that key") { check(answer(jobAnswer, "receipt" to js(r))) }
        refuses("signature") { check(answer(jobAnswer, "signature" to JsonPrimitive("abcd"))) }
    }

    @Test
    fun `accepts params holding floats and integers past 2^53 as core canonicalizes them`() {
        val floats = verifyReceiptSignature(Json.parseToJsonElement(s(v, "floatAnswerText")), keyOf("1"))
        assertEquals(
            "0.1",
            floats.params
                .getValue("a")
                .jsonPrimitive.content,
        )
        assertEquals(
            "1e+21",
            floats.params
                .getValue("b")
                .jsonPrimitive.content,
        )
        assertEquals(
            "5e-324",
            floats.params
                .getValue("d")
                .jsonPrimitive.content,
        )
        val big = verifyReceiptSignature(Json.parseToJsonElement(s(v, "bigNumberAnswerText")), keyOf("1"))
        assertEquals(
            "9007199254740992",
            big.params
                .getValue("a")
                .jsonPrimitive.content,
        )
        assertEquals(
            "-9223372036854776000",
            big.params
                .getValue("c")
                .jsonPrimitive.content,
        )
        assertEquals(
            "10000000000000000000",
            big.params
                .getValue("d")
                .jsonPrimitive.content,
        )
    }

    @Test
    fun `refuses a receipt for another app or run, and an unattested cluster first`() {
        refuses("is for app app-1, not app-2") { verifyPipelineReceipt(cluster(Replica(1)), "app-2", run, jobAnswer, mock, http) }
        refuses("is for run run-1, not run-2") { check(jobAnswer, runId = "run-2") }
        refuses("replica 1") { verifyPipelineReceipt(cluster(Replica(1, unbound = true)), app, run, jobAnswer, mock, http) }
    }

    @Test
    fun `accepts a signed receipt with a null optional field`() {
        val r = goodReceipt().also { it["costSoFar"] = JsonNull }
        assertEquals(run, check(signed(r)).run)
    }

    @Test
    fun `refuses what core's schema refuses, even under a valid signature`() {
        val mutations: List<Triple<String, (MutableMap<String, Any?>) -> Unit, Regex>> =
            listOf(
                Triple("an unknown field", { r -> r.set("extra", 1) }, Regex("receipt\\.extra is not a known field")),
                Triple("a missing field", { r -> r.remove("pipeline") }, Regex("receipt\\.pipeline is missing")),
                Triple("a wrong type", { r -> r.set("state", 5) }, Regex("receipt\\.state is not a string")),
                Triple("a negative size", { r -> r.o("inputs").l("data")[0].set("size", -1) }, Regex("inputs\\.data\\[0]\\.size is not an unsigned integer")),
                Triple("an unknown nested field", { r -> r.o("steps").o("load").set("extra", 1) }, Regex("steps\\.load\\.extra is not a known field")),
                Triple("a wrong nested type", { r ->
                    r
                        .o("steps")
                        .o("load")
                        .l("runs")[0]
                        .set("index", "0")
                }, Regex("steps\\.load\\.runs\\[0]")),
                Triple("a run that is neither", { r -> r.o("steps").o("load").l("runs")[1] = mutableMapOf<String, Any?>("index" to JsonPrimitive(1)) }, Regex("steps\\.load\\.runs\\[1]")),
                Triple("params that are not an object", { r -> r["params"] = JsonArray(emptyList()) }, Regex("receipt\\.params is not an object")),
                Triple("a u64 of 2^64", { r -> r["startTsMs"] = JsonPrimitive(BigInteger.ONE.shiftLeft(64)) }, Regex("unsigned integer in range")),
            )
        for ((name, mutate, message) in mutations) {
            val r = goodReceipt().also(mutate)
            try {
                refuses(message) { check(signed(r)) }
            } catch (e: AssertionError) {
                throw AssertionError("$name: ${e.message}", e)
            }
        }
    }

    @Test
    fun `refuses a field the answer does not have, and malformed answers`() {
        refuses("extra is not a known field") { verifyReceiptSignature(answer(jobAnswer, "extra" to JsonPrimitive(1)), keyOf("1")) }
        refuses(Regex(".")) { check(JsonNull) }
        refuses(Regex(".")) { check(answer(jobAnswer, "signature" to JsonPrimitive(5))) }
        refuses(Regex(".")) { check(answer(jobAnswer, "receipt" to null)) }
        refuses(Regex(".")) { check(answer(jobAnswer, "receipt" to JsonPrimitive("x"))) }
    }

    @Test
    fun `refuses an answering node that is not a node id`() {
        val bad = listOf(JsonPrimitive("1"), JsonPrimitive(true), JsonArray(listOf(JsonPrimitive(1))), JsonPrimitive(-1), JsonPrimitive(9_007_199_254_740_992L), JsonPrimitive(1.5), JsonNull)
        for (node in bad) refuses("not a node id") { check(answer(jobAnswer, "node" to node)) }
        refuses("not a node id") { check(answer(jobAnswer, "node" to null)) }
    }

    @Test
    fun `reads hex as core reads it`() {
        assertEquals(run, check(answer(jobAnswer, "signature" to JsonPrimitive(s(jobAnswer, "signature").uppercase()))).run)
        refuses("signature") { check(answer(jobAnswer, "signature" to JsonPrimitive(" ${s(jobAnswer, "signature")}\n"))) }
    }

    // ---------------------------------------------------------------- each job's report

    @Test
    fun `rebuilds every report message core signs, and its hash`() {
        for (m in v.getValue("reportMessages").jsonArray.map { it.jsonObject }) {
            val message = reportMessage(fieldsOf(m))
            assertEquals(s(m, "messageHex"), Hex.encode(message))
            assertEquals(s(m, "sha256"), Hex.encode(sha256(message)))
        }
    }

    private fun outputOf(o: JsonObject) = JobOutput(s(o, "id"), o.getValue("size").jsonPrimitive.long, s(o, "sha256"), o.getValue("segments").jsonArray.map { it.jsonPrimitive.content })

    private fun fieldsOf(m: JsonObject) =
        ReportFields(
            assignment = s(m, "assignment"),
            app = s(m, "app"),
            run = s(m, "run"),
            attempt = m.getValue("attempt").jsonPrimitive.long,
            reason = (m["reason"] as? JsonPrimitive)?.takeIf { it != JsonNull }?.content,
            message = s(m, "message"),
            resultSha256 = s(m, "resultSha256"),
            outputs = m.getValue("outputs").jsonArray.map { outputOf(it.jsonObject) },
            usage = m.getValue("usage").jsonArray.map { it.jsonPrimitive.content },
            log = (m["log"] as? JsonObject)?.let(::outputOf),
        )

    @Test
    fun `rebuilds every job key binding core computes`() {
        for (b in v.getValue("jobKeyBindings").jsonArray.map { it.jsonObject }) {
            val identity = (b["clusterIdentity"] as? JsonPrimitive)?.takeIf { it != JsonNull }?.let { Hex.decode(it.content, "identity", 32) }
            val got = jobKeyBinding(Hex.decode(s(b, "transportKey"), "key", 32), Hex.decode(s(b, "jobKey"), "key", 32), s(b, "release"), s(b, "cluster"), b.getValue("node").jsonPrimitive.long, identity)
            assertEquals(s(b, "binding"), got)
        }
    }

    @Test
    fun `refuses a number or a hash it cannot write`() {
        val good = fieldsOf(v.getValue("reportMessages").jsonArray[0].jsonObject)
        refuses("not a decimal integer") { reportMessage(good.copy(usage = listOf("18446744073709551616", "0", "0"))) }
        refuses("not a decimal integer") { reportMessage(good.copy(usage = listOf("+1", "0", "0"))) }
        refuses("not a decimal integer") { reportMessage(good.copy(usage = listOf("1", "0", ""))) }
        refuses("lowercase hex") { reportMessage(good.copy(assignment = good.assignment.uppercase())) }
    }

    @Test
    fun `verifies a done report with no log, and a failure that carries its message`() {
        val r = goodReceipt()
        loadRun(r).o("job")["log"] = JsonNull
        rereport(r, loadRun(r), null)
        assertEquals(run, check(signed(r)).run)
        loadRun(r).set("outcome", "failed:app-error")
        loadRun(r)["outputs"] = mutableListOf<Any?>()
        loadRun(r).o("job").set("message", "invalid input: row 7")
        loadRun(r).o("job")["segments"] = mutableListOf<Any?>()
        rereport(r, loadRun(r), "app-error")
        assertEquals(run, check(signed(r)).run)
    }

    private fun cancelled(
        outcome: String,
        reason: String?,
    ): MutableMap<String, Any?> {
        val r = goodReceipt()
        loadRun(r).set("outcome", outcome)
        loadRun(r)["outputs"] = mutableListOf<Any?>()
        loadRun(r).o("job")["segments"] = mutableListOf<Any?>()
        loadRun(r).o("job")["log"] = JsonNull
        rereport(r, loadRun(r), reason)
        return r
    }

    @Test
    fun `verifies a cancel the job reported, and refuses one it reported as not cancelled`() {
        for (outcome in listOf("failed:cancelled", "cancelled")) assertEquals(run, check(signed(cancelled(outcome, "cancelled"))).run)
        refusedAs(cancelled("cancelled", null), "the report hash is not that of what the proof says")
    }

    @Test
    fun `refuses anything the job did not sign`() {
        val edits: List<Pair<String, (MutableMap<String, Any?>) -> Unit>> =
            listOf(
                "signature" to { r -> loadRun(r).set("signature", "06".repeat(64)) },
                "job key" to { r -> loadRun(r).set("jobKey", "08".repeat(32)) },
                "assignment" to { r -> loadRun(r).set("assignmentDigest", "45".repeat(32)) },
                "attempt" to { r -> loadRun(r).set("attempt", 3) },
                "outcome" to { r -> loadRun(r).set("outcome", "failed:trap") },
                "output id" to { r -> loadRun(r).l("outputs")[0].set("id", "11".repeat(32)) },
                "output size" to { r -> loadRun(r).l("outputs")[0].let { it.set("size", ((it as Map<String, Any?>)["size"] as JsonPrimitive).int + 1) } },
                "output hash" to { r -> loadRun(r).l("outputs")[1].set("sha256", "99".repeat(32)) },
                "output order" to { r -> loadRun(r).l("outputs").reverse() },
                "dropped output" to { r ->
                    loadRun(r).l("outputs").removeAt(1)
                    loadRun(r).o("job").l("segments").removeAt(1)
                },
                "segment count" to { r -> loadRun(r).o("job").l("segments").removeAt(1) },
                "an extra segment list" to { r -> loadRun(r).o("job").l("segments").add(mutableListOf<Any?>()) },
                "segment" to { r -> (loadRun(r).o("job").l("segments")[0] as MutableList<Any?>)[0] = JsonPrimitive("77".repeat(32)) },
                "result" to { r -> loadRun(r).o("job").set("resultSha256", "34".repeat(32)) },
                "message" to { r -> loadRun(r).o("job").set("message", "bang") },
                "log" to { r -> loadRun(r).o("job")["log"] = JsonNull },
                "log id" to { r -> loadRun(r).o("job").o("log").set("id", "12".repeat(32)) },
                "usage" to { r -> loadRun(r).o("usage").let { it.set("fuel", (it.t("fuel").toLong() + 1).toString()) } },
                "report hash" to { r -> loadRun(r).set("reportSha256", "00".repeat(32)) },
            )
        for ((what, edit) in edits) {
            val r = goodReceipt().also(edit)
            try {
                refusedAs(r, Regex("run-1\\.load\\.0|job evidence|segments"))
            } catch (e: AssertionError) {
                throw AssertionError("$what: ${e.message}", e)
            }
        }
        refusedAs(goodReceipt().also { loadRun(it).remove("job") }, "has no job evidence")
        refusedAs(goodReceipt().also { r -> loadRun(r).let { it.set("assignmentDigest", it.t("assignmentDigest").uppercase()) } }, "lowercase hex")
        refusedAs(goodReceipt().also { r -> loadRun(r).let { it.set("jobKey", it.t("jobKey").uppercase()) } }, "lowercase hex")
        refusedAs(goodReceipt().also { r -> loadRun(r).o("job").let { it.set("resultSha256", it.t("resultSha256").uppercase()) } }, "lowercase hex")
    }

    private val smallOrder =
        listOf(
            "01" + "00".repeat(31), // identity
            "ec" + "ff".repeat(30) + "7f", // order 2
            "00".repeat(32), // order 4
            "00".repeat(31) + "80",
            "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a", // order 8
            "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa",
            "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05",
            "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85",
        )

    @Test
    fun `knows the small-order points, and a real key is not one`() {
        for (point in smallOrder) assertTrue(point, Ed25519Points.isSmallOrder(Hex.decode(point, "point", 32)))
        assertFalse(Ed25519Points.isSmallOrder(Hex.decode(jobKey, "key", 32)))
        assertFalse(Ed25519Points.isSmallOrder(keyOf("1")))
    }

    @Test
    fun `refuses a small-order R or key, and a malleated S`() {
        for (point in smallOrder) {
            val smallR = goodReceipt().also { r -> loadRun(r).let { it.set("signature", point + it.t("signature").substring(64)) } }
            refuses("the job key did not sign") { verifyJobReports(pipelineReceiptOf(js(smallR))) }
            val smallKey = goodReceipt().also { loadRun(it).set("jobKey", point) }
            refuses("the job key did not sign") { verifyJobReports(pipelineReceiptOf(js(smallKey))) }
        }
        val malleated = goodReceipt()
        val sig = loadRun(malleated).t("signature")
        val s = BigInteger(1, Hex.decode(sig.substring(64), "s", 32).reversedArray()).add(Ed25519Points.L)
        val sBytes =
            s
                .toByteArray()
                .let { b -> ByteArray(32 - b.size) + b }
                .takeLast(32)
                .toByteArray()
                .reversedArray()
        loadRun(malleated).set("signature", sig.substring(0, 64) + Hex.encode(sBytes))
        refuses("the job key did not sign") { verifyJobReports(pipelineReceiptOf(js(malleated))) }
        assertEquals(mapOf(jobKey to 4L), verifyJobReports(pipelineReceiptOf(js(goodReceipt()))))
    }

    @Test
    fun `refuses a signature by a small-order key, which verifies for any message in a lax check`() {
        val r = goodReceipt()
        val identity = "01" + "00".repeat(31)
        loadRun(r).set("jobKey", identity)
        rereport(r, loadRun(r), null)
        loadRun(r).set("signature", identity + "00".repeat(32))
        val attestations = r.o("jobAttestations")
        attestations[identity] = attestations.getValue(jobKey).let { (it as MutableMap<String, Any?>).toMutableMap().also { d -> d.set("jobKey", identity) } }
        attestations.remove(jobKey)
        refusedAs(r, "the job key did not sign")
    }

    @Test
    fun `binds a proof to its run, step and index, each step's runs listed once in order`() {
        val other = goodReceipt().also { it.set("run", "run-2") }
        refusedAs(other, "run-1.load.0 is listed as run-2.load.0", "run-2")
        val step = goodReceipt().also { it.o("steps")["other"] = it.o("steps").remove("load") }
        refusedAs(step, "run-1.load.0 is listed as run-1.other.0")
        refusedAs(goodReceipt().also { loadRun(it).set("index", 1) }, "lists run 1 where run 0 belongs")
        refusedAs(
            goodReceipt().also {
                it
                    .o("steps")
                    .o("load")
                    .l("runs")[1]
                    .set("index", 2)
            },
            "lists run 2 where run 1 belongs",
        )
        refusedAs(
            goodReceipt().also {
                it
                    .o("steps")
                    .o("load")
                    .l("runs")[1]
                    .set("index", 0)
            },
            "lists run 0 where run 1 belongs",
        )
        refusedAs(
            goodReceipt().also {
                it
                    .o("steps")
                    .o("load")
                    .l("runs")[1]
                    .set("cached", false)
            },
            "run 1 is listed as not cached",
        )
        refusedAs(goodReceipt().also { it.set("version", 1) }, "the receipt is version 1")
    }

    @Test
    fun `refuses one job key named on two nodes`() {
        val r = goodReceipt()
        val second = mut(js(loadRun(r))) as MutableMap<String, Any?>
        second.set("run", "run-1.load.1")
        second.set("index", 1)
        second.set("node", 5)
        r.o("steps").o("load").l("runs")[1] = second
        rereport(r, second, null)
        refusedAs(r, "job key $jobKey is named on two nodes")
        second.set("node", 4)
        assertEquals(run, check(signed(r)).run)
    }

    @Test
    fun `accepts a run the leader ended, and refuses one that names a report, outputs or usage`() {
        for (outcome in listOf("failed:lost", "failed:refused", "failed:timeout", "failed:quota", "failed:not-granted", "failed:too-large", "cancelled")) {
            assertEquals(run, check(signed(goodReceipt().also { mergeRun(it).set("outcome", outcome) })).run)
        }
        val edits: List<Triple<String, (MutableMap<String, Any?>) -> Unit, String>> =
            listOf(
                Triple("outputs", { r -> mergeRun(r).l("outputs").add(mutableMapOf<String, Any?>("id" to JsonPrimitive("10".repeat(32)), "size" to JsonPrimitive(1), "sha256" to JsonPrimitive("11".repeat(32)))) }, "by the leader"),
                Triple("usage", { r -> mergeRun(r).o("usage").set("fuel", "5") }, "by the leader"),
                Triple("usage spelled 00", { r -> mergeRun(r).o("usage").set("cpuMicros", "00") }, "by the leader"),
                Triple("a report hash", { r -> mergeRun(r).set("reportSha256", "05".repeat(32)) }, "by the leader"),
                Triple("a signature", { r -> mergeRun(r).set("signature", "06".repeat(64)) }, "by the leader"),
                Triple("job evidence", { r -> mergeRun(r)["job"] = (mut(js(loadRun(r).o("job"))) as MutableMap<String, Any?>).also { it["segments"] = mutableListOf<Any?>() } }, "by the leader"),
                Triple("done without a report", { r -> mergeRun(r).set("outcome", "done") }, "has no assignment digest"),
                Triple("a job failure without its report", { r -> mergeRun(r).set("outcome", "failed:fuel") }, "has no assignment digest"),
                Triple("a reason no run ends with", { r -> mergeRun(r).set("outcome", "failed:nonsense") }, "is not one a run ends with"),
                Triple("an unknown outcome", { r -> mergeRun(r).set("outcome", "exploded") }, "is not one a run ends with"),
            )
        for ((what, edit, message) in edits) {
            try {
                refusedAs(goodReceipt().also(edit), message)
            } catch (e: AssertionError) {
                throw AssertionError("$what: ${e.message}", e)
            }
        }
    }

    @Test
    fun `refuses a job attestation that is missing or another's, naming the job key`() {
        val doc = { r: MutableMap<String, Any?> -> r.o("jobAttestations").o(jobKey) }
        val edits: List<Triple<String, (MutableMap<String, Any?>) -> Unit, String>> =
            listOf(
                Triple("missing", { r -> r.remove("jobAttestations") }, "carries no attestation for it"),
                Triple("for another key", { r -> doc(r).set("jobKey", "08".repeat(32)) }, "attestation is another key's"),
                Triple("for no key", { r -> doc(r).remove("jobKey") }, "attestation is another key's"),
                Triple("for another node", { r -> doc(r).set("node", 5) }, "not node 4 of eu-shared-1"),
                Triple("for another cluster", { r -> doc(r).set("cluster", "eu-other") }, "not node 4 of eu-shared-1"),
                Triple("for another cluster identity", { r -> doc(r).set("clusterIdentity", "22".repeat(32)) }, "another cluster identity"),
                Triple("without a cluster identity", { r -> doc(r).remove("clusterIdentity") }, "another cluster identity"),
                Triple("under another identity", { r -> r.set("cluster", "22".repeat(32)) }, "another cluster identity"),
                Triple("with a bad nonce", { r -> doc(r).set("nonce", "zz") }, "nonce is not 32 bytes of hex"),
            )
        for ((what, edit, message) in edits) {
            try {
                refusedAs(goodReceipt().also(edit), Regex("^job key $jobKey: .*${Regex.escape(message)}"))
            } catch (e: AssertionError) {
                throw AssertionError("$what: ${e.message}", e)
            }
        }
    }

    @Test
    fun `refuses a job quote that binds another job key, node or cluster identity`() {
        fun rebind(
            r: MutableMap<String, Any?>,
            key: String? = null,
            node: Long? = null,
            identity: String? = null,
        ) {
            val d = r.o("jobAttestations").o(jobKey)
            val suffix =
                jobKeyBinding(
                    Hex.decode(d.t("transportKey"), "key", 32),
                    Hex.decode(key ?: d.t("jobKey"), "key", 32),
                    d.o("release").t("version"),
                    d.t("cluster"),
                    node ?: (d["node"] as JsonPrimitive).long,
                    Hex.decode(identity ?: d.t("clusterIdentity"), "identity", 32),
                )
            d.set("quote", mockQuote(Hex.decode(d.t("nonce"), "nonce", 32), suffix))
        }
        refusedAs(goodReceipt().also { rebind(it, key = "08".repeat(32)) }, Regex("^job key .*does not bind"))
        refusedAs(goodReceipt().also { rebind(it, node = 5) }, Regex("^job key .*does not bind"))
        refusedAs(goodReceipt().also { rebind(it, identity = "22".repeat(32)) }, Regex("^job key .*does not bind"))
        assertEquals(run, check(signed(goodReceipt().also { rebind(it) })).run)
    }

    @Test
    fun `refuses every mock quote under a policy without allowMock`() {
        refusedAs(goodReceipt(), "mock quote refused", policy = CloudPolicy())
    }

    @Test
    fun `refuses a job attestation missing a required field, allows an unknown one, refuses attestations that are not a map`() {
        refusedAs(goodReceipt().also { it.o("jobAttestations").o(jobKey).remove("transportKey") }, Regex("jobAttestations\\.[0-9a-f]+\\.transportKey is missing"))
        assertEquals(run, check(signed(goodReceipt().also { it.o("jobAttestations").o(jobKey).set("future", 1) })).run)
        refusedAs(goodReceipt().also { it["jobAttestations"] = JsonNull }, Regex("jobAttestations is not an object"))
        for (field in listOf("governance", "kms")) {
            refusedAs(goodReceipt().also { it.o("jobAttestations").o(jobKey).set(field, 5) }, "$field is not an object")
        }
    }

    // ---------------------------------------------------------------- canonical JSON

    @Test
    fun `writes numbers as ECMAScript does`() {
        val cases =
            mapOf(
                "0" to "0",
                "-0" to "0",
                "1" to "1",
                "1.0" to "1",
                "0.1" to "0.1",
                "1e21" to "1e+21",
                "1e20" to "100000000000000000000",
                "5e-324" to "5e-324",
                "1e-7" to "1e-7",
                "0.000001" to "0.000001",
                "123.456" to "123.456",
                "-1.5" to "-1.5",
                "9223372036854775807" to "9223372036854776000",
                "1.7976931348623157e308" to "1.7976931348623157e+308",
                "4.35" to "4.35",
            )
        for ((literal, expected) in cases) assertEquals(literal, expected, Jcs.number(literal))
        assertEquals(
            """{"\u0000a":"\u001f\n\"\\","b":[1,true,null],"é":{}}""",
            Jcs.canonicalize(Json.parseToJsonElement("""{"é":{},"b":[1.0,true,null],"\u0000a":"\u001f\n\"\\"}""")),
        )
        refuses(Regex(".")) { receiptMessage(JsonPrimitive("\ud800")) }
    }
}
