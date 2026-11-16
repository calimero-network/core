package com.calimero.mero.cluster

import com.calimero.mero.crypto.Ed25519
import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.randomBytes
import com.calimero.mero.crypto.sha256
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.ByteArrayOutputStream
import java.math.BigInteger
import java.util.Base64
import java.util.zip.GZIPOutputStream

/**
 * A signed single-service `.mpk`, made the way core's `calimero-bundle` signs one
 * (`sign_manifest_json`), as mero-js's `tests/cluster/mpk.ts` does: `signerId` is the key's
 * `did:key`, the signature is Ed25519 over `SHA-256("calimero.bundle.manifest.v1\0" || JCS(manifest))`
 * with `signature` left out, and the archive is a gzipped tar of `manifest.json` and `app.wasm`.
 */
class Publisher(
    val seed: ByteArray = randomBytes(32),
) {
    val publicKey: ByteArray = Ed25519.publicKey(seed)
    val signerId: String = "did:key:z" + base58(byteArrayOf(0xed.toByte(), 0x01) + publicKey)
}

fun signedMpk(
    pkg: String,
    version: String,
    wasm: ByteArray,
    key: Publisher,
): ByteArray {
    val manifest =
        buildJsonObject {
            put("version", "1.0")
            put("package", pkg)
            put("appVersion", version)
            put("signerId", key.signerId)
            put("minRuntimeVersion", "0.1.0")
            put(
                "wasm",
                buildJsonObject {
                    put("path", "app.wasm")
                    put("hash", Hex.encode(sha256(wasm)))
                    put("size", wasm.size)
                },
            )
        }
    val payload = sha256("calimero.bundle.manifest.v1\u0000".toByteArray() + jcs(manifest).toByteArray())
    val b64url = Base64.getUrlEncoder().withoutPadding()
    val signed =
        JsonObject(
            manifest +
                (
                    "signature" to
                        buildJsonObject {
                            put("algorithm", "ed25519")
                            put("publicKey", b64url.encodeToString(key.publicKey))
                            put("signature", b64url.encodeToString(Ed25519.sign(key.seed, payload)))
                        }
                ),
        )
    return gzip(tar(listOf("manifest.json" to signed.toString().toByteArray(), "app.wasm" to wasm)))
}

/** RFC 8785 for what a manifest holds: objects, arrays, strings, integers, booleans. */
private fun jcs(value: JsonElement): String =
    when (value) {
        is JsonObject -> value.entries.sortedBy { it.key }.joinToString(",", "{", "}") { (k, v) -> "${JsonPrimitive(k)}:${jcs(v)}" }
        is JsonArray -> value.joinToString(",", "[", "]") { jcs(it) }
        else -> value.toString()
    }

private const val B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"

private fun base58(bytes: ByteArray): String {
    var n = BigInteger(1, bytes)
    val out = StringBuilder()
    val radix = BigInteger.valueOf(58)
    while (n > BigInteger.ZERO) {
        val (q, r) = n.divideAndRemainder(radix)
        out.append(B58[r.toInt()])
        n = q
    }
    for (b in bytes) {
        if (b.toInt() != 0) break
        out.append('1')
    }
    return out.reverse().toString()
}

private fun tar(entries: List<Pair<String, ByteArray>>): ByteArray {
    val out = ByteArrayOutputStream()
    for ((name, body) in entries) {
        val h = ByteArray(512)

        fun write(
            at: Int,
            s: String,
        ) = s.toByteArray().copyInto(h, at)
        write(0, name)
        write(100, "0000644\u0000")
        write(108, "0000000\u0000")
        write(116, "0000000\u0000")
        write(124, body.size.toString(8).padStart(11, '0') + "\u0000")
        write(136, "00000000000\u0000")
        write(148, "        ")
        write(156, "0")
        write(257, "ustar\u0000")
        write(263, "00")
        val sum = h.sumOf { it.toInt() and 0xff }
        write(148, sum.toString(8).padStart(6, '0') + "\u0000 ")
        out.write(h)
        out.write(body)
        out.write(ByteArray((512 - body.size % 512) % 512))
    }
    out.write(ByteArray(1024))
    return out.toByteArray()
}

private fun gzip(bytes: ByteArray): ByteArray = ByteArrayOutputStream().also { o -> GZIPOutputStream(o).use { it.write(bytes) } }.toByteArray()
