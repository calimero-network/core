package com.calimero.mero.sample.explorer

import android.content.Context
import com.calimero.mero.CalimeroClient
import com.calimero.mero.PasskeyProvider
import com.calimero.mero.compose.PlatformPasskey
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json

/** One input field for an operation form. */
enum class OpFieldKind { LINE, MULTILINE }

data class OpField(
    val id: String,
    val label: String,
    val placeholder: String,
    val kind: OpFieldKind,
) {
    companion object {
        fun line(
            id: String,
            label: String,
            placeholder: String = "",
        ): OpField =
            OpField(id, label, placeholder, OpFieldKind.LINE)

        fun json(
            id: String = "body",
            label: String = "Request JSON",
            placeholder: String = "{}",
        ): OpField =
            OpField(id, label, placeholder, OpFieldKind.MULTILINE)
    }
}

/**
 * What an operation runs against: the signed-in [CalimeroClient], and the passkey for the calls the
 * node asks the account holder to confirm (`412 CONFIRM_REQUIRED`).
 */
class OpEnv(
    val client: CalimeroClient,
    private val context: Context,
    /** The software passkey of a development node, if signed in to one. */
    private val devPasskey: PasskeyProvider? = null,
) {
    /** The development node's software passkey, else the device's (Credential Manager). */
    val passkey: PasskeyProvider get() = devPasskey ?: PlatformPasskey(context)
}

/**
 * A single invokable SDK method: metadata + input fields + an async runner that returns a rendered
 * (pretty-printed) result string. 1:1 with the Swift sample's `SDKOperation`.
 */
class SDKOperation(
    val id: String,
    val category: String,
    val name: String,
    val summary: String,
    val fields: List<OpField>,
    val run: suspend (OpEnv, Map<String, String>) -> String,
)

/** Rendering / decoding helpers shared by the operation catalog. */
object Fmt {
    val pretty: Json =
        Json {
            prettyPrint = true
            ignoreUnknownKeys = true
            encodeDefaults = false
            explicitNulls = false
        }

    /** Pretty-print any serializable value. */
    inline fun <reified T> json(value: T): String = pretty.encodeToString(value)

    /** Decode a user-entered JSON string into a request type (empty → `{}`). */
    inline fun <reified T> decode(raw: String): T {
        val text = raw.trim()
        return pretty.decodeFromString(if (text.isEmpty()) "{}" else text)
    }
}

/** Trimmed value for a field id (empty string if absent). */
fun Map<String, String>.v(key: String): String = (this[key] ?: "").trim()

/** Optional trimmed value (null if empty). */
fun Map<String, String>.opt(key: String): String? = v(key).ifEmpty { null }
