package com.calimero.mero.sample.mock

import com.calimero.mero.app.MeroAppBackend
import com.calimero.mero.testkit.InMemoryAppBackend
import com.calimero.mero.testkit.RecordedContractCall
import com.calimero.mero.testkit.RecordingContextContract
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put

/**
 * The debug demo: mero-testkit's [InMemoryAppBackend] with a tiny in-memory mero-chat contract
 * (`get_messages`, `send_message`, `get_profiles`, `set_profile`), so the whole app runs with no
 * cluster and no passkey. Drives the instrumented `LoginFlowTest`.
 */
object MockBackend {
    const val AVAILABLE: Boolean = true

    fun backend(): MeroAppBackend {
        val messages = mutableMapOf<String, MutableList<JsonObject>>()
        val profiles = mutableMapOf<String, String>()
        lateinit var backend: InMemoryAppBackend
        val contract =
            RecordingContextContract { call: RecordedContractCall ->
                synchronized(messages) { answer(call, messages, profiles, backend.account) }
            }
        backend = InMemoryAppBackend(recording = contract)
        backend.applicationId = "com.calimero.chat"
        return backend
    }

    private fun answer(
        call: RecordedContractCall,
        messages: MutableMap<String, MutableList<JsonObject>>,
        profiles: MutableMap<String, String>,
        account: String,
    ): JsonElement {
        val args = call.args as? JsonObject ?: JsonObject(emptyMap())

        fun s(key: String) = (args[key] as? JsonPrimitive)?.contentOrNull.orEmpty()
        return when (call.method) {
            "get_messages" -> {
                val list = messages[call.contextId].orEmpty()
                buildJsonObject {
                    put("total_count", list.size)
                    put("messages", JsonArray(list))
                    put("start_position", 0)
                }
            }
            "send_message" -> {
                val list = messages.getOrPut(call.contextId) { mutableListOf() }
                list +=
                    buildJsonObject {
                        put("id", "m${list.size + 1}")
                        put("text", s("message"))
                        put("sender", account)
                        put("sender_username", profiles[account].orEmpty())
                        put("timestamp", System.currentTimeMillis() / 1000)
                    }
                JsonNull
            }
            "set_profile" -> {
                profiles[account] = s("username")
                JsonNull
            }
            "get_profiles" ->
                JsonArray(
                    profiles.map { (identity, name) ->
                        buildJsonObject {
                            put("identity", identity)
                            put("username", name)
                        }
                    },
                )
            else -> JsonNull
        }
    }
}
