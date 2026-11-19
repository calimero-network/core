package com.calimero.mero

import com.calimero.mero.crypto.Ed25519
import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.Jcs
import com.calimero.mero.crypto.concat
import com.calimero.mero.crypto.randomBytes
import com.calimero.mero.crypto.sha256
import com.calimero.mero.http.PlainJsonHttp
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import okhttp3.OkHttpClient
import java.math.BigDecimal
import java.math.BigInteger

/*
 * Checking a pipeline run's receipt as core's `calimero-client` does (`Client::verify_app_cluster`,
 * `attest::verify_receipt_key`, `Client::verify_receipt`; mero-js `client/receipts.ts`): the
 * receipt is signed by the answering node's receipt key, and that key is one every replica's quote
 * attests, bound to the cluster and node the entry node names. A quote of the unbound
 * `receipt-key.v2` form is never accepted. A version 2 receipt also proves each job's report: its
 * job key signed what the proof says, and a job node's quote under the client's policy binds that
 * key, node, cluster and cluster identity.
 */

/** A receipt, its signer's attestation or a job's report that does not check out. Messages name what failed. */
class ReceiptVerificationException(
    message: String,
    cause: Throwable? = null,
) : Exception(message, cause)

private fun refuse(message: String): Nothing = throw ReceiptVerificationException(message)

private const val PIPELINE_RECEIPT_DOMAIN = "calimero.pipeline.receipt.v1\u0000"
private const val JOB_REPORT_DOMAIN = "calimero.compute.report.v1\u0000"
private const val JOB_REPORT_LOG_DOMAIN = "calimero.compute.report.log.v1\u0000"
private const val JOB_KEY_DOMAIN = "calimero.tee-attest.job-key.v1"
private const val RECEIPT_KEY_DOMAIN = "calimero.tee-attest.receipt-key.v3"
private const val RECEIPT_VERSION_JOBS = 2L
private val JOB_FAIL_REASONS = setOf("fuel", "memory", "trap", "app-error", "integrity", "cancelled")
private val LEADER_FAIL_REASONS = setOf("lost", "refused", "timeout", "quota", "not-granted", "too-large")
private val U64_MAX: BigInteger = BigInteger.ONE.shiftLeft(64).subtract(BigInteger.ONE)
private const val U32_MAX = 0xffffffffL

// ---------------------------------------------------------------- what a receipt says

/** A file named by id, size and plaintext SHA-256 (hex). */
data class FileRef(
    val id: String,
    val size: Long,
    val sha256: String,
)

/** A file as a job's report signs it: id, plaintext size and SHA-256, and each ciphertext segment's SHA-256 (lowercase hex). */
data class JobOutput(
    val id: String,
    val size: Long,
    val sha256: String,
    val segments: List<String>,
)

/** The rest of what a job's report signs: the result's hash, the failure text, each output's segment hashes, the module log. */
data class JobEvidence(
    val resultSha256: String,
    val message: String? = null,
    val segments: List<List<String>>,
    val log: JobOutput? = null,
)

/** A run's usage: CPU micros, GiB micros and fuel, as decimal strings. */
data class RunUsage(
    val cpuMicros: String,
    val gibMicros: String,
    val fuel: String,
)

/** One run of a step: a [CachedRun] or a [StepRunProof] (core's untagged `RunEntry`). */
sealed interface RunEntry {
    val index: Long
}

/** A run answered from the step cache: it ran nothing, so it has no proof. */
data class CachedRun(
    override val index: Long,
    val cached: Boolean,
    val cacheKey: String,
) : RunEntry

/** What one step run's proof records: which job node ran which assignment, and what came out. */
data class StepRunProof(
    val run: String,
    val step: String,
    override val index: Long,
    val outcome: String,
    val attempt: Long,
    val node: Long? = null,
    val jobKey: String? = null,
    val assignmentDigest: String? = null,
    val reportSha256: String? = null,
    val signature: String? = null,
    val outputs: List<FileRef>,
    val usage: RunUsage,
    /** What the job's report signs that the fields above do not (version 2 only). */
    val job: JobEvidence? = null,
) : RunEntry

data class StepReceipt(
    val status: String,
    val reason: String? = null,
    val runs: List<RunEntry>,
)

/** A job node's `purpose=jobs` attestation: the quote binds its transport key, job key, release, cluster, node and cluster identity. */
data class JobAttestation(
    val quote: String,
    val transportKey: String,
    val appHash: String,
    val nonce: String,
    val release: AttestedRelease,
    val cluster: String,
    val node: Long,
    val mock: Boolean,
    val jobKey: String? = null,
    val clusterIdentity: String? = null,
)

/**
 * What the node states about a finished pipeline run (core's `PipelineReceipt`), read from the
 * canonical bytes its signature covers. [cluster] is the cluster identity key, hex; [raw] the
 * receipt as JSON.
 */
data class PipelineReceipt(
    val version: Long,
    val cluster: String,
    val app: String,
    val run: String,
    val pipeline: String,
    val release: String,
    val pipelineHash: String,
    val modules: Map<String, String>,
    val inputs: Map<String, List<FileRef>>,
    val params: JsonObject,
    val state: String,
    val startTsMs: Long,
    val endTsMs: Long,
    val steps: Map<String, StepReceipt>,
    val costSoFar: String? = null,
    /** Each job key a proof names, its job node's attestation (version 2 only). */
    val jobAttestations: Map<String, JobAttestation>? = null,
    val raw: JsonObject,
)

/** Every replica of an app, attested: its receipt key by node id, the governance they all attest, and the cluster name every quote bound. */
class AttestedCluster(
    val keys: Map<Long, ByteArray>,
    val governance: Governance,
    val cluster: String,
)

/** A receipt key and the governance its quote binds, rebuilt from the governance's canonical fields. */
class AttestedReceiptKey(
    val key: ByteArray,
    val governance: Governance,
)

// ---------------------------------------------------------------- the receipt's schema

// As core's serde types read it: `deny_unknown_fields`, required fields, typed values.
private typealias Check = (JsonElement, String) -> Unit

private fun fail(
    path: String,
    what: String,
): Nothing = refuse("the receipt's $path $what")

private fun JsonElement.isText() = this is JsonPrimitive && this != JsonNull && isString

private fun JsonElement.isBool() = this is JsonPrimitive && this != JsonNull && !isString && (content == "true" || content == "false")

/** The number this element is, when it is an integer (any spelling of one). */
private fun JsonElement.integer(): BigInteger? {
    if (this !is JsonPrimitive || this == JsonNull || isString || isBool()) return null
    val number = runCatching { BigDecimal(content) }.getOrNull() ?: return null
    return runCatching { number.toBigIntegerExact() }.getOrNull()
}

private val str: Check = { v, p -> if (!v.isText()) fail(p, "is not a string") }
private val bool: Check = { v, p -> if (!v.isBool()) fail(p, "is not a boolean") }
private val any: Check = { _, _ -> }
private val obj: Check = { v, p -> if (v !is JsonObject) fail(p, "is not an object") }

private fun uint(max: BigInteger): Check =
    { v, p ->
        val n = v.integer()
        if (n == null || n.signum() < 0 || n > max) fail(p, "is not an unsigned integer in range")
    }

private val u32: Check = uint(BigInteger.valueOf(U32_MAX))

// A u64 is read as a Long here: a value past 2^63-1, which no receipt holds, is refused.
private val u64: Check = uint(BigInteger.valueOf(Long.MAX_VALUE))

private fun list(item: Check): Check =
    { v, p ->
        if (v !is JsonArray) fail(p, "is not an array")
        v.forEachIndexed { i, x -> item(x, "$p[$i]") }
    }

private fun map(value: Check): Check =
    { v, p ->
        if (v !is JsonObject) fail(p, "is not an object")
        for ((k, x) in v) value(x, "$p.$k")
    }

// `optional` fields are Options (null reads as absent); `defaulted` ones may be absent but never null;
// an `open` struct allows other fields, as a serde type without `deny_unknown_fields`.
private fun struct(
    required: Map<String, Check>,
    optional: Map<String, Check> = emptyMap(),
    defaulted: Map<String, Check> = emptyMap(),
    open: Boolean = false,
): Check =
    { v, p ->
        if (v !is JsonObject) fail(p, "is not an object")
        if (!open) {
            for (k in v.keys) if (k !in required && k !in optional && k !in defaulted) fail("$p.$k", "is not a known field")
        }
        for ((k, check) in required) {
            val x = v[k] ?: fail("$p.$k", "is missing")
            check(x, "$p.$k")
        }
        for ((k, check) in optional) v[k]?.takeIf { it != JsonNull }?.let { check(it, "$p.$k") }
        for ((k, check) in defaulted) v[k]?.let { check(it, "$p.$k") }
    }

private val fileRef = struct(mapOf("id" to str, "size" to u64, "sha256" to str))
private val jobOutput = struct(mapOf("id" to str, "size" to u64, "sha256" to str, "segments" to list(str)))
private val jobEvidence =
    struct(mapOf("resultSha256" to str, "segments" to list(list(str))), mapOf("log" to jobOutput), mapOf("message" to str))
private val attestationDoc =
    struct(
        mapOf(
            "quote" to str,
            "transportKey" to str,
            "appHash" to str,
            "nonce" to str,
            "release" to struct(mapOf("version" to str, "profile" to str), open = true),
            "cluster" to str,
            "node" to u64,
            "mock" to bool,
        ),
        mapOf("receiptKey" to str, "governance" to obj, "kms" to obj, "jobKey" to str, "clusterIdentity" to str),
        open = true,
    )
private val stepRun =
    struct(
        mapOf(
            "run" to str,
            "step" to str,
            "index" to u32,
            "outcome" to str,
            "attempt" to u32,
            "outputs" to list(fileRef),
            "usage" to struct(mapOf("cpuMicros" to str, "gibMicros" to str, "fuel" to str)),
        ),
        mapOf("node" to u64, "jobKey" to str, "assignmentDigest" to str, "reportSha256" to str, "signature" to str, "job" to jobEvidence),
    )
private val cachedRun = struct(mapOf("index" to u32, "cached" to bool, "cacheKey" to str))

// Core's `RunEntry` is untagged: a cached run first, else a step run proof.
private val runEntry: Check = { v, p ->
    try {
        cachedRun(v, p)
    } catch (_: ReceiptVerificationException) {
        stepRun(v, p)
    }
}
private val pipelineReceiptSchema =
    struct(
        mapOf(
            "version" to u32,
            "cluster" to str,
            "app" to str,
            "run" to str,
            "pipeline" to str,
            "release" to str,
            "pipelineHash" to str,
            "modules" to map(str),
            "inputs" to map(list(fileRef)),
            "params" to map(any),
            "state" to str,
            "startTsMs" to u64,
            "endTsMs" to u64,
            "steps" to map(struct(mapOf("status" to str, "runs" to list(runEntry)), mapOf("reason" to str))),
        ),
        mapOf("costSoFar" to str),
        mapOf("jobAttestations" to map(attestationDoc)),
    )

// ---------------------------------------------------------------- reading a checked receipt

private fun JsonObject.text(key: String): String = (getValue(key) as JsonPrimitive).content

private fun JsonObject.textOrNull(key: String): String? = (this[key] as? JsonPrimitive)?.takeIf { it != JsonNull }?.content

private fun JsonObject.long(key: String): Long = getValue(key).integer()!!.toLong()

private fun JsonObject.longOrNull(key: String): Long? = this[key]?.takeIf { it != JsonNull }?.integer()?.toLong()

private fun JsonElement.strings(): List<String> = (this as JsonArray).map { (it as JsonPrimitive).content }

private fun fileRefOf(o: JsonObject) = FileRef(o.text("id"), o.long("size"), o.text("sha256"))

private fun jobOutputOf(o: JsonObject) = JobOutput(o.text("id"), o.long("size"), o.text("sha256"), o.getValue("segments").strings())

private fun runOf(o: JsonObject): RunEntry =
    if (o.containsKey("cached") && o.containsKey("cacheKey") && o.size == 3) {
        CachedRun(o.long("index"), o.text("cached") == "true", o.text("cacheKey"))
    } else {
        val usage = o.getValue("usage") as JsonObject
        StepRunProof(
            run = o.text("run"),
            step = o.text("step"),
            index = o.long("index"),
            outcome = o.text("outcome"),
            attempt = o.long("attempt"),
            node = o.longOrNull("node"),
            jobKey = o.textOrNull("jobKey"),
            assignmentDigest = o.textOrNull("assignmentDigest"),
            reportSha256 = o.textOrNull("reportSha256"),
            signature = o.textOrNull("signature"),
            outputs = (o.getValue("outputs") as JsonArray).map { fileRefOf(it as JsonObject) },
            usage = RunUsage(usage.text("cpuMicros"), usage.text("gibMicros"), usage.text("fuel")),
            job =
                (o["job"] as? JsonObject)?.let { j ->
                    JobEvidence(
                        resultSha256 = j.text("resultSha256"),
                        message = j.textOrNull("message"),
                        segments = (j.getValue("segments") as JsonArray).map { it.strings() },
                        log = (j["log"] as? JsonObject)?.let(::jobOutputOf),
                    )
                },
        )
    }

private fun jobAttestationOf(o: JsonObject): JobAttestation {
    val release = o.getValue("release") as JsonObject
    return JobAttestation(
        quote = o.text("quote"),
        transportKey = o.text("transportKey"),
        appHash = o.text("appHash"),
        nonce = o.text("nonce"),
        release = AttestedRelease(release.text("version"), release.text("profile")),
        cluster = o.text("cluster"),
        node = o.long("node"),
        mock = o.text("mock") == "true",
        jobKey = o.textOrNull("jobKey"),
        clusterIdentity = o.textOrNull("clusterIdentity"),
    )
}

/** [value] checked against core's receipt schema, as a [PipelineReceipt]. */
internal fun pipelineReceiptOf(value: JsonElement): PipelineReceipt {
    pipelineReceiptSchema(value, "receipt")
    val r = value as JsonObject
    return PipelineReceipt(
        version = r.long("version"),
        cluster = r.text("cluster"),
        app = r.text("app"),
        run = r.text("run"),
        pipeline = r.text("pipeline"),
        release = r.text("release"),
        pipelineHash = r.text("pipelineHash"),
        modules = (r.getValue("modules") as JsonObject).mapValues { (it.value as JsonPrimitive).content },
        inputs = (r.getValue("inputs") as JsonObject).mapValues { e -> (e.value as JsonArray).map { fileRefOf(it as JsonObject) } },
        params = r.getValue("params") as JsonObject,
        state = r.text("state"),
        startTsMs = r.long("startTsMs"),
        endTsMs = r.long("endTsMs"),
        steps =
            (r.getValue("steps") as JsonObject).mapValues { e ->
                val s = e.value as JsonObject
                StepReceipt(s.text("status"), s.textOrNull("reason"), (s.getValue("runs") as JsonArray).map { runOf(it as JsonObject) })
            },
        costSoFar = r.textOrNull("costSoFar"),
        jobAttestations = (r["jobAttestations"] as? JsonObject)?.mapValues { jobAttestationOf(it.value as JsonObject) },
        raw = r,
    )
}

// ---------------------------------------------------------------- bytes

private fun utf8(text: String) = text.encodeToByteArray()

private fun u32be(n: Long): ByteArray = ByteArray(4) { i -> (n ushr (8 * (3 - i))).toByte() }

private fun u64be(n: BigInteger): ByteArray {
    if (n.signum() < 0 || n > U64_MAX) refuse("a number is past 2^64-1")
    return ByteArray(8) { i -> n.shiftRight(8 * (7 - i)).toInt().toByte() }
}

private fun u64be(n: Long): ByteArray = u64be(BigInteger.valueOf(n))

private fun prefixed(text: String): ByteArray {
    val bytes = utf8(text)
    return concat(u32be(bytes.size.toLong()), bytes)
}

private val STRICT_HEX = Regex("^[0-9a-fA-F]*$")
private val HASH_HEX = Regex("^[0-9a-f]{64}$")

/** Hex as core's `hex::decode` reads it (either case), of exactly [bytes] bytes: no trimming, nothing else. */
private fun strictHex(
    value: Any?,
    label: String,
    bytes: Int,
): ByteArray {
    val text = (value as? String) ?: ((value as? JsonElement)?.takeIf { it.isText() } as? JsonPrimitive)?.content
    if (text == null || text.length != bytes * 2 || !STRICT_HEX.matches(text)) refuse("$label is not $bytes bytes of hex")
    return Hex.decode(text, label, bytes)
}

/** The 32 bytes of a hash spelled as the wire spells it: 64 lowercase hex digits (core's `hash_bytes`). */
private fun hashBytes(text: String?): ByteArray {
    if (text == null || !HASH_HEX.matches(text)) refuse("a hash is 64 lowercase hex digits")
    return Hex.decode(text, "hash", 32)
}

private val DECIMAL = Regex("^[0-9]+$")

/** A decimal `u64` as the receipt spells usage: digits only. */
private fun decimal(text: String): BigInteger {
    if (!DECIMAL.matches(text)) refuse("\"$text\" is not a decimal integer")
    val n = BigInteger(text)
    if (n > U64_MAX) refuse("\"$text\" is not a decimal integer")
    return n
}

/** A node id: a non-negative integer a JSON number spells within 2^53 (mero-js's safe integers). */
private fun nodeId(
    value: JsonElement?,
    label: String,
): Long {
    val n = value?.integer()
    if (n == null || n.signum() < 0 || n > BigInteger.valueOf(MAX_SAFE_INTEGER)) refuse("$label is not a node id")
    return n.toLong()
}

private const val MAX_SAFE_INTEGER = 9_007_199_254_740_991L

// ---------------------------------------------------------------- governance and the receipt key

private fun one(
    value: JsonElement?,
    allowed: List<String>,
    label: String,
    fallback: String? = null,
): String {
    if (value == null && fallback != null) return fallback
    val text = (value as? JsonPrimitive)?.takeIf { it.isText() }?.content
    if (text == null || text !in allowed) refuse("the governance's $label is not one of ${allowed.joinToString(", ")}")
    return text
}

/** The bytes a quote binds: the JSON, fields in declaration order, the later ones only when not their default (core's `Governance::canonical`). */
fun governanceCanonical(governance: JsonObject): ByteArray {
    val g = governance
    val out = StringBuilder("{")

    fun field(
        name: String,
        value: String,
    ) {
        if (out.length > 1) out.append(',')
        out
            .append('"')
            .append(name)
            .append("\":\"")
            .append(value)
            .append('"')
    }
    field("releases", one(g["releases"], listOf("owner", "operator"), "releases"))
    field("appUpgrades", one(g["appUpgrades"], listOf("owner", "operator"), "appUpgrades"))
    field("outbound", one(g["outbound"], listOf("owner", "operator"), "outbound"))
    one(g["deployments"], listOf("open", "granted"), "deployments", "open").takeIf { it != "open" }?.let { field("deployments", it) }
    one(g["deployers"], listOf("operator", "owner"), "deployers", "operator").takeIf { it != "operator" }?.let { field("deployers", it) }
    one(g["signUp"], listOf("open", "invite", "closed"), "signUp", "open").takeIf { it != "open" }?.let { field("signUp", it) }
    return utf8(out.append('}').toString())
}

/**
 * What a receipt-key quote binds after its nonce (core's `receipt_key_binding`, `receipt-key.v3`),
 * hex: the key, the cluster's governance, and the cluster (u32-BE length-prefixed) and node id (u64
 * BE), so one node's quote never stands for another node's or another cluster's.
 */
fun receiptKeyBinding(
    receiptKey: ByteArray,
    governance: JsonObject,
    cluster: String,
    node: Long,
): String =
    Hex.encode(sha256(concat(utf8(RECEIPT_KEY_DOMAIN), receiptKey, sha256(governanceCanonical(governance)), prefixed(cluster), u64be(node))))

private val lenient = Json { ignoreUnknownKeys = true }

private fun attestationDocOf(doc: JsonObject): AttestationDoc {
    val release = doc["release"] as? JsonObject
    return AttestationDoc(
        quote = (doc["quote"] as? JsonPrimitive)?.takeIf { it.isText() }?.content ?: refuse("the attestation carries no quote"),
        transportKey = (doc["transportKey"] as? JsonPrimitive)?.content.orEmpty(),
        release = AttestedRelease((release?.get("version") as? JsonPrimitive)?.content.orEmpty(), (release?.get("profile") as? JsonPrimitive)?.content.orEmpty()),
    )
}

/**
 * Checks a receipt-key attestation as core's `verify_receipt_key` does: its quote binds the key, the
 * governance and the [cluster] and [node] the caller asked for (never the document's own fields,
 * which nothing signs). Answers the key and the governance rebuilt from its canonical fields.
 */
suspend fun verifyReceiptKey(
    doc: JsonObject,
    nonce: ByteArray,
    cluster: String,
    node: Long,
    policy: CloudPolicy,
    httpClient: OkHttpClient = PlainJsonHttp.defaultClient(),
): AttestedReceiptKey {
    val attestation = attestationDocOf(doc)
    val receiptKey = doc["receiptKey"]?.takeIf { it != JsonNull } ?: refuse("the attestation names no receipt key")
    val governance = doc["governance"] as? JsonObject ?: refuse("the attestation names no governance")
    val key = strictHex(receiptKey, "the receipt key", 32)
    val canonical = governanceCanonical(governance)
    val suffix = Hex.decode(receiptKeyBinding(key, governance, cluster, node), "binding", 32)
    verifyQuoteBinding(attestation, nonce, suffix, policy, "receipt key", httpClient)
    return AttestedReceiptKey(key, lenient.decodeFromString(Governance.serializer(), canonical.decodeToString()))
}

/**
 * Verifies every replica of [app] (core's `Client::verify_app_cluster`) from [attestation]'s answer
 * (`GET /apps/{app}/attestation` for a nonce): each one's quote over a fresh nonce, held to
 * [policy], binding its receipt key, the cluster's governance, and the cluster and node id the
 * entry node names. Any replica error, a node id that differs from the document's, or replicas
 * that attest other governance fails the whole check.
 */
@Suppress("ThrowsCount", "CyclomaticComplexMethod")
internal suspend fun verifyAppCluster(
    attestation: suspend (app: String, nonce: String) -> JsonElement,
    app: String,
    policy: CloudPolicy,
    httpClient: OkHttpClient,
): AttestedCluster {
    val nonce = randomBytes(32)
    val answer = attestation(app, Hex.encode(nonce)) as? JsonObject
    val cluster = (answer?.get("cluster") as? JsonPrimitive)?.takeIf { it.isText() }?.content
    val replicas = answer?.get("replicas") as? JsonArray
    if (cluster == null || replicas == null) refuse("the node answered a malformed attestation")
    val keys = LinkedHashMap<Long, ByteArray>()
    var governance: String? = null
    var attestedGovernance: Governance? = null
    for (replica in replicas) {
        if (replica !is JsonObject) refuse("a replica entry is not an object")
        val node = nodeId(replica["node"], "a replica")
        if (replica.containsKey("error")) {
            val why = (replica["error"] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: replica["error"].toString()
            refuse("replica $node gave no attestation: $why")
        }
        val doc = replica["attestation"] as? JsonObject ?: refuse("replica $node gave neither an attestation nor an error")
        val claimed = nodeId(doc["node"], "replica $node's document")
        if (claimed != node) refuse("replica $node answered as node $claimed")
        val attested =
            try {
                verifyReceiptKey(doc, nonce, cluster, node, policy, httpClient)
            } catch (e: ReceiptVerificationException) {
                throw ReceiptVerificationException("replica $node: ${e.message}", e)
            } catch (e: com.calimero.mero.tee.TeeTrustException) {
                throw ReceiptVerificationException("replica $node: ${e.message}", e)
            }
        // Equal canonical bytes are equal rules.
        val canonical = governanceCanonical(doc["governance"] as JsonObject).decodeToString()
        if (governance != null && governance != canonical) refuse("replica $node attests other governance")
        governance = canonical
        attestedGovernance = attested.governance
        keys[node] = attested.key
    }
    return AttestedCluster(keys, attestedGovernance ?: refuse("no replica of $app attested"), cluster)
}

// ---------------------------------------------------------------- the receipt's signature

/** The receipt's canonical JSON (RFC 8785); any finite number is allowed, as core canonicalizes every number as f64. */
private fun canonicalReceipt(receipt: JsonElement): String =
    try {
        Jcs.canonicalize(receipt)
    } catch (e: IllegalArgumentException) {
        throw ReceiptVerificationException("the receipt cannot be canonicalized: ${e.message}", e)
    }

/** The bytes a receipt's signature covers (core's `receipt_message`): the domain, then the canonical JSON. */
fun receiptMessage(receipt: JsonElement): ByteArray = concat(utf8(PIPELINE_RECEIPT_DOMAIN), utf8(canonicalReceipt(receipt)))

/**
 * Whether [answer] (`{ receipt, node, signature }`) is signed by [key] (core's
 * `verify_pipeline_receipt`). Answers the receipt as parsed from the canonical bytes that were
 * verified, checked against core's receipt schema.
 */
fun verifyReceiptSignature(
    answer: JsonElement,
    key: ByteArray,
): PipelineReceipt {
    if (answer !is JsonObject) refuse("the receipt answer is not an object")
    answer.keys.firstOrNull { it !in setOf("receipt", "node", "signature") }?.let { refuse("the receipt answer's $it is not a known field") }
    val signature = strictHex(answer["signature"], "the signature", 64)
    val receipt = answer["receipt"] ?: refuse("the receipt answer has no receipt")
    val canonical = canonicalReceipt(receipt)
    if (!Ed25519.verify(key, signature, concat(utf8(PIPELINE_RECEIPT_DOMAIN), utf8(canonical)))) refuse("the receipt is not signed by that key")
    return pipelineReceiptOf(Json.parseToJsonElement(canonical))
}

// ---------------------------------------------------------------- each job's report

/** One job report's signed fields, in the order [reportMessage] writes them (core's `ReportFields`). */
data class ReportFields(
    /** SHA-256 of the assignment message the job ran (hex). */
    val assignment: String,
    val app: String,
    val run: String,
    val attempt: Long,
    /** Null for a done report, else why it failed. */
    val reason: String?,
    val message: String,
    val resultSha256: String,
    val outputs: List<JobOutput>,
    /** CPU micros, GiB micros, fuel, as decimal strings. */
    val usage: List<String>,
    val log: JobOutput? = null,
)

@Suppress("SpreadOperator")
private fun outputBytes(o: JobOutput): ByteArray =
    concat(hashBytes(o.id), u64be(o.size), hashBytes(o.sha256), u32be(o.segments.size.toLong()), *o.segments.map(::hashBytes).toTypedArray())

/**
 * The bytes a job node signs for one report (core's `report_message`): the domain, the assignment
 * digest, the length-prefixed app, run, reason and message, the result's hash, the outputs, the
 * usage, then the log after its own domain; integers big-endian, hashes as their 32 bytes.
 */
@Suppress("SpreadOperator")
fun reportMessage(f: ReportFields): ByteArray {
    require(f.usage.size == 3) { "usage is three numbers" }
    return concat(
        utf8(JOB_REPORT_DOMAIN),
        hashBytes(f.assignment),
        prefixed(f.app),
        prefixed(f.run),
        u32be(f.attempt),
        byteArrayOf(if (f.reason == null) 0 else 1),
        prefixed(f.reason ?: ""),
        prefixed(f.message),
        hashBytes(f.resultSha256),
        u32be(f.outputs.size.toLong()),
        *f.outputs.map(::outputBytes).toTypedArray(),
        *f.usage.map { u64be(decimal(it)) }.toTypedArray(),
        *(f.log?.let { arrayOf(utf8(JOB_REPORT_LOG_DOMAIN), outputBytes(it)) } ?: emptyArray()),
    )
}

/**
 * What a job-node attestation's quote binds after the nonce (core's `job_key_binding`): both keys,
 * the release and cluster (length-prefixed), the node id, and the cluster identity when there is
 * one. Hex.
 */
fun jobKeyBinding(
    transportKey: ByteArray,
    jobKey: ByteArray,
    release: String,
    cluster: String,
    node: Long,
    clusterIdentity: ByteArray? = null,
): String = Hex.encode(sha256(concat(utf8(JOB_KEY_DOMAIN), transportKey, jobKey, prefixed(release), prefixed(cluster), u64be(node), clusterIdentity ?: ByteArray(0))))

/** The reason a job signed for [outcome]: `Signed(reason)` (null when done), or null for an end only the leader gives. */
private class Signed(
    val reason: String?,
)

private fun signedReason(
    outcome: String,
    reported: Boolean,
): Signed? {
    if (outcome == "done") return Signed(null)
    if (outcome == "cancelled") return if (reported) Signed("cancelled") else null
    val bad = { refuse("the outcome \"$outcome\" is not one a run ends with") }
    if (!outcome.startsWith("failed:")) bad()
    val reason = outcome.removePrefix("failed:")
    if (reason in JOB_FAIL_REASONS) return Signed(reason)
    if (reason !in LEADER_FAIL_REASONS) bad()
    return null
}

/** Ed25519 as core's `verify_strict` reads it: a non-canonical S and a small-order key or R are refused as well as a bad signature. */
internal fun verifyStrict(
    key: ByteArray,
    message: ByteArray,
    signature: ByteArray,
): Boolean {
    val s = BigInteger(1, signature.copyOfRange(32, 64).reversedArray())
    if (s >= Ed25519Points.L || Ed25519Points.isSmallOrder(key) || Ed25519Points.isSmallOrder(signature.copyOfRange(0, 32))) return false
    return Ed25519.verify(key, signature, message)
}

/** The arithmetic `verify_strict`'s refusals need: the group order, and whether a point's order divides 8. */
internal object Ed25519Points {
    // BigInteger.TWO is API 33; minSdk is 24.
    private val TWO: BigInteger = BigInteger.valueOf(2)
    val L: BigInteger = BigInteger.ONE.shiftLeft(252).add(BigInteger("27742317777372353535851937790883648493"))
    private val P: BigInteger = BigInteger.ONE.shiftLeft(255).subtract(BigInteger.valueOf(19))
    private val D: BigInteger =
        P
            .subtract(BigInteger.valueOf(121665))
            .mod(P)
            .multiply(BigInteger.valueOf(121666).modInverse(P))
            .mod(P)

    private fun inv(x: BigInteger) = x.mod(P).modPow(P.subtract(TWO), P)

    /** Whether [encoded] decodes to a point whose order divides 8 (false when it is not a point). */
    @Suppress("MagicNumber")
    fun isSmallOrder(encoded: ByteArray): Boolean {
        val bytes = encoded.reversedArray()
        val sign = (bytes[0].toInt() ushr 7) and 1
        bytes[0] = (bytes[0].toInt() and 0x7f).toByte()
        val y = BigInteger(1, bytes).mod(P)
        val y2 = y.multiply(y).mod(P)
        val x2 =
            y2
                .subtract(BigInteger.ONE)
                .mod(P)
                .multiply(inv(D.multiply(y2).add(BigInteger.ONE)))
                .mod(P)
        var x = x2.modPow(P.add(BigInteger.valueOf(3)).shiftRight(3), P)
        if (x.multiply(x).mod(P) != x2) x = x.multiply(TWO.modPow(P.subtract(BigInteger.ONE).shiftRight(2), P)).mod(P)
        if (x.multiply(x).mod(P) != x2) return false
        if (x.testBit(0) != (sign == 1)) x = P.subtract(x).mod(P)
        var px = x
        var py = y
        repeat(3) {
            val k =
                D
                    .multiply(px)
                    .multiply(px)
                    .multiply(py)
                    .multiply(py)
                    .mod(P)
            val nx =
                TWO
                    .multiply(px)
                    .multiply(py)
                    .multiply(inv(BigInteger.ONE.add(k)))
                    .mod(P)
            val ny =
                py
                    .multiply(py)
                    .add(px.multiply(px))
                    .multiply(inv(BigInteger.ONE.subtract(k)))
                    .mod(P)
            px = nx
            py = ny
        }
        return px.signum() == 0 && py == BigInteger.ONE
    }
}

private fun proofFailure(
    run: String,
    e: ReceiptVerificationException,
): Nothing = throw ReceiptVerificationException("$run: ${e.message}", e)

/**
 * Checks one step run's proof (core's `check_proof`): it is the run the receipt's run, step and
 * index name, and either the leader ended it (no report, outputs or usage) or its job node signed
 * a report that says exactly what the proof says. Answers the job key and node of a signed one.
 */
@Suppress("ThrowsCount", "CyclomaticComplexMethod", "ComplexCondition")
private fun checkProof(
    receipt: PipelineReceipt,
    step: String,
    p: StepRunProof,
): Pair<String, Long>? {
    val expected = "${receipt.run}.$step.${p.index}"
    if (p.run != expected || p.step != step) refuse("the proof of ${p.run} is listed as $expected")
    val reported = p.signature != null
    val signed = signedReason(p.outcome, reported)
    if (signed == null) {
        val zero = listOf(p.usage.cpuMicros, p.usage.gibMicros, p.usage.fuel).all { it == "0" }
        if (reported || p.reportSha256 != null || p.job != null || p.outputs.isNotEmpty() || !zero) {
            refuse("${p.run} ended ${p.outcome} by the leader, yet names a report, outputs or usage")
        }
        return null
    }
    val missing = { what: String -> refuse("${p.run} ended ${p.outcome} but its proof has no $what") }
    val node = p.node ?: missing("job node")
    val jobKey = p.jobKey ?: missing("job key")
    val assignment = p.assignmentDigest ?: missing("assignment digest")
    val reportSha256 = p.reportSha256 ?: missing("report hash")
    val signature = p.signature ?: missing("report signature")
    val job = p.job ?: missing("job evidence (it ended before receipts carried it)")
    if (job.segments.size != p.outputs.size) {
        refuse("${p.run}: the job evidence lists segments for ${job.segments.size} outputs, the proof ${p.outputs.size}")
    }
    val outputs = p.outputs.mapIndexed { i, o -> JobOutput(o.id, o.size, o.sha256, job.segments[i]) }
    val message =
        try {
            reportMessage(
                ReportFields(
                    assignment = assignment,
                    app = receipt.app,
                    run = p.run,
                    attempt = p.attempt,
                    reason = signed.reason,
                    message = job.message ?: "",
                    resultSha256 = job.resultSha256,
                    outputs = outputs,
                    usage = listOf(p.usage.cpuMicros, p.usage.gibMicros, p.usage.fuel),
                    log = job.log,
                ),
            )
        } catch (e: ReceiptVerificationException) {
            proofFailure(p.run, e)
        }
    if (Hex.encode(sha256(message)) != reportSha256) refuse("${p.run}: the report hash is not that of what the proof says")
    val key =
        try {
            hashBytes(jobKey)
        } catch (e: ReceiptVerificationException) {
            proofFailure(p.run, e)
        }
    if (!Regex("^[0-9a-fA-F]{128}$").matches(signature)) refuse("${p.run}: the report signature is not 64 bytes of hex")
    if (!verifyStrict(key, message, Hex.decode(signature, "signature", 64))) refuse("${p.run}: the job key did not sign what the proof says")
    return jobKey to node
}

/**
 * Checks every step run's proof of a version 2 receipt (core's `verify_job_reports`), each step's
 * runs listed once in index order, and answers each job key that signed a report with the node it
 * ran on; a key named on two nodes is refused. Whose key each is (its attestation) is for the
 * caller to verify. What a job signed is proven; the rest (step and run states, cached runs, and
 * the node and key a leader-ended run names) is the signing replica's word.
 */
fun verifyJobReports(receipt: PipelineReceipt): Map<String, Long> {
    if (receipt.version != RECEIPT_VERSION_JOBS) {
        refuse("the receipt is version ${receipt.version}; only version $RECEIPT_VERSION_JOBS carries what each job signed")
    }
    val keys = LinkedHashMap<String, Long>()
    for ((step, s) in receipt.steps) {
        s.runs.forEachIndexed { i, entry ->
            if (entry.index != i.toLong()) refuse("step $step lists run ${entry.index} where run $i belongs")
            when (entry) {
                is CachedRun -> if (!entry.cached) refuse("step $step run $i is listed as not cached")
                is StepRunProof ->
                    checkProof(receipt, step, entry)?.let { (key, node) ->
                        if ((keys[key] ?: node) != node) refuse("job key $key is named on two nodes")
                        keys[key] = node
                    }
            }
        }
    }
    return keys
}

/**
 * Checks that [receipt] carries the attestation of job key [key]: a job node's quote, under
 * [policy], binding that key to node [node] of [cluster] taking work from the cluster identity the
 * receipt names.
 */
@Suppress("ThrowsCount")
private suspend fun verifyJobAttestation(
    receipt: PipelineReceipt,
    cluster: String,
    key: String,
    node: Long,
    policy: CloudPolicy,
    httpClient: OkHttpClient,
) {
    val doc = receipt.jobAttestations?.get(key) ?: refuse("the receipt carries no attestation for it")
    if (doc.jobKey != key) refuse("its attestation is another key's")
    if (doc.node != node || doc.cluster != cluster) refuse("it is attested as node ${doc.node} of cluster ${doc.cluster}, not node $node of $cluster")
    if (doc.clusterIdentity != receipt.cluster) refuse("its job node takes work from another cluster identity")
    val nonce =
        try {
            hashBytes(doc.nonce)
        } catch (_: ReceiptVerificationException) {
            refuse("its attestation's nonce is not 32 bytes of hex")
        }
    val binding =
        jobKeyBinding(
            strictHex(doc.transportKey, "the transport key", 32),
            Hex.decode(key, "the job key", 32),
            doc.release.version,
            doc.cluster,
            doc.node,
            strictHex(doc.clusterIdentity, "the cluster identity", 32),
        )
    verifyQuoteBinding(AttestationDoc(quote = doc.quote, transportKey = doc.transportKey, release = doc.release), nonce, Hex.decode(binding, "binding", 32), policy, "job key", httpClient)
}

/**
 * Checks that the node that answered holds a receipt key attested under [policy]
 * ([verifyAppCluster]) and signed [answer] with it, that the receipt is for [app] and [run], that
 * every job report it names is signed by its job key ([verifyJobReports]), and that each such key
 * is a job node's of the attested cluster and the receipt's cluster identity, under [policy]. A
 * version 1 receipt is refused. Answers the receipt.
 */
internal suspend fun verifyPipelineReceipt(
    attestation: suspend (app: String, nonce: String) -> JsonElement,
    app: String,
    run: String,
    answer: JsonElement,
    policy: CloudPolicy,
    httpClient: OkHttpClient,
): PipelineReceipt {
    val attested = verifyAppCluster(attestation, app, policy, httpClient)
    if (answer !is JsonObject) refuse("the receipt answer is not an object")
    val node = nodeId(answer["node"], "the receipt answer")
    val key = attested.keys[node] ?: refuse("node $node is not an attested replica of $app")
    val receipt = verifyReceiptSignature(answer, key)
    if (receipt.app != app) refuse("the receipt is for app ${receipt.app}, not $app")
    if (receipt.run != run) refuse("the receipt is for run ${receipt.run}, not $run")
    for ((jobKey, jobNode) in verifyJobReports(receipt)) {
        try {
            verifyJobAttestation(receipt, attested.cluster, jobKey, jobNode, policy, httpClient)
        } catch (e: ReceiptVerificationException) {
            throw ReceiptVerificationException("job key $jobKey: ${e.message}", e)
        } catch (e: com.calimero.mero.tee.TeeTrustException) {
            throw ReceiptVerificationException("job key $jobKey: ${e.message}", e)
        }
    }
    return receipt
}
