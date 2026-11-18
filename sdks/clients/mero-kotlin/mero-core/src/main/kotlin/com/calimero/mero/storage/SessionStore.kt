package com.calimero.mero.storage

import android.content.Context
import com.calimero.mero.CalimeroSession
import kotlinx.serialization.json.Json

/**
 * Where a signed-in [CalimeroSession] is kept between launches, plus a few small values beside it
 * (the node URL, a development passkey). A refresh token is used once, so the store must always
 * hold the newest session: hand [save] to [com.calimero.mero.CalimeroClientOptions.onSession].
 */
interface SessionStore {
    fun load(): CalimeroSession?

    /** Keep [session]; null forgets it. */
    fun save(session: CalimeroSession?)

    /** A small value kept beside the session, or null. */
    fun value(key: String): String?

    /** Keep [value] under [key]; null removes it. */
    fun setValue(
        key: String,
        value: String?,
    )

    /** Forget the session and every value. */
    fun clear()
}

/** A [SessionStore] in memory: tests, previews, and apps that sign in on every launch. */
class MemorySessionStore : SessionStore {
    private var session: CalimeroSession? = null
    private val values = HashMap<String, String>()

    @Synchronized override fun load(): CalimeroSession? = session

    @Synchronized override fun save(session: CalimeroSession?) {
        this.session = session
    }

    @Synchronized override fun value(key: String): String? = values[key]

    @Synchronized override fun setValue(
        key: String,
        value: String?,
    ) {
        if (value == null) values.remove(key) else values[key] = value
    }

    @Synchronized override fun clear() {
        session = null
        values.clear()
    }
}

/**
 * A [SessionStore] in Keystore-backed encrypted preferences ([fileName] in the app's private
 * storage). A keyset that no longer decrypts (a restored backup) is reset: the person signs in again.
 */
class EncryptedSessionStore(
    context: Context,
    fileName: String = "calimero_session",
) : SessionStore {
    private val prefs = EncryptedPrefs.open(context, fileName)
    private val json = Json { ignoreUnknownKeys = true }

    override fun load(): CalimeroSession? =
        prefs.getString(SESSION, null)?.let { runCatching { json.decodeFromString(CalimeroSession.serializer(), it) }.getOrNull() }

    override fun save(session: CalimeroSession?) {
        prefs
            .edit()
            .apply { if (session == null) remove(SESSION) else putString(SESSION, json.encodeToString(CalimeroSession.serializer(), session)) }
            .apply()
    }

    override fun value(key: String): String? = prefs.getString(VALUE + key, null)

    override fun setValue(
        key: String,
        value: String?,
    ) {
        prefs
            .edit()
            .apply { if (value == null) remove(VALUE + key) else putString(VALUE + key, value) }
            .apply()
    }

    override fun clear() {
        prefs.edit().clear().apply()
    }

    private companion object {
        const val SESSION = "session"
        const val VALUE = "value."
    }
}
