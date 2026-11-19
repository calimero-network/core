package com.calimero.mero.sample.app

import android.content.Context
import android.util.Log
import androidx.compose.runtime.mutableStateListOf
import com.calimero.mero.sample.BuildConfig
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * The Diagnostics log: what the session did (sign-in, node login, chat steps), newest last.
 * Shown by the Diagnostics screen, copyable, and mirrored to logcat under `MeroSample` in debug
 * builds only (it names spaces, contexts and accounts).
 */
object SampleLog {
    enum class Level(
        val tag: String,
    ) {
        INFO("info"),
        OK("ok"),
        WARN("warn"),
        ERR("error"),
        REQ("req"),
    }

    data class Line(
        val id: Long,
        val time: Long,
        val level: Level,
        val text: String,
    )

    val lines = mutableStateListOf<Line>()
    private var next = 0L
    private val format = SimpleDateFormat("HH:mm:ss.SSS", Locale.US)

    fun log(
        level: Level,
        text: String,
    ) {
        val line = Line(next++, System.currentTimeMillis(), level, text)
        lines.add(line)
        if (lines.size > MAX_LINES) lines.removeRange(0, lines.size - MAX_LINES)
        if (BuildConfig.DEBUG) runCatching { Log.i("MeroSample", "${level.tag} $text") }
    }

    fun clear() = lines.clear()

    fun stamp(line: Line): String = format.format(Date(line.time))

    fun text(): String = lines.joinToString("\n") { "${stamp(it)} [${it.level.tag}] ${it.text}" }

    private const val MAX_LINES = 300
}

/** The node URL and username last used on the sign-in screen. The password is never stored. */
class NodePrefs(
    context: Context,
) {
    private val prefs = context.getSharedPreferences("mero-sample", Context.MODE_PRIVATE)

    var lastNodeUrl: String
        get() = prefs.getString(KEY_URL, null) ?: DEFAULT_NODE_URL
        set(value) = prefs.edit().putString(KEY_URL, value).apply()

    var lastNodeUser: String
        get() = prefs.getString(KEY_USER, null) ?: DEFAULT_NODE_USER
        set(value) = prefs.edit().putString(KEY_USER, value).apply()

    companion object {
        /** The emulator's alias for the host computer, where `merod` runs. */
        const val DEFAULT_NODE_URL = "http://10.0.2.2:4001"
        const val DEFAULT_NODE_USER = "dev"
        private const val KEY_URL = "lastNodeURL"
        private const val KEY_USER = "lastNodeUser"
    }
}
