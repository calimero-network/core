package com.calimero.mero.api.infrastructure

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonBuilder

object Serializer {
    private var isJsonInitialized = false

    @JvmStatic
    val kotlinxSerializationJson: Json by lazy {
        isJsonInitialized = true
        Json {
            encodeDefaults = true
            explicitNulls = false
            ignoreUnknownKeys = true
            coerceInputValues = true
            isLenient = true

            apply(kotlinxSerializationJsonConfiguration)
        }
    }

    var kotlinxSerializationJsonConfiguration: JsonBuilder.() -> Unit = {}
        set(value) {
            check(!isJsonInitialized) {
                "Cannot configure kotlinxSerializationJsonConfiguration after kotlinxSerializationJson has been initialized."
            }
            field = value
        }
}
