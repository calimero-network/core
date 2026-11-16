package com.calimero.mero.sample.app

import android.app.Application
import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.calimero.mero.CalimeroClient
import com.calimero.mero.CalimeroClientOptions
import com.calimero.mero.CloudPolicy
import com.calimero.mero.PasskeyProvider
import com.calimero.mero.SoftPasskey
import com.calimero.mero.app.MeroAppBackend
import com.calimero.mero.compose.MeroAppSession
import com.calimero.mero.sample.chat.ChatService
import com.calimero.mero.sample.mock.MockBackend
import com.calimero.mero.storage.EncryptedSessionStore
import com.calimero.mero.storage.MemorySessionStore
import com.calimero.mero.storage.SessionStore
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

/** How the sample is signed in. */
enum class SignInMode {
    /** A real cluster: real TDX quotes from a signed mero-tee release, the device's passkey. */
    CLOUD,

    /** `merod local` (or any `--dev` cluster): mock attestation allowed, a software passkey kept on the device. */
    DEV,

    /** Debug builds: no network at all, mero-testkit's in-memory backend. */
    DEMO,
}

/**
 * Owns the sample's session for the activity's lifetime: a [MeroAppSession] on the cluster the
 * person chose (its [CalimeroClient] is what the SDK tab drives), or the in-memory demo.
 *
 * The node URL and mode are kept beside the session ([SessionStore]), so a relaunch restores it.
 */
@Suppress("TooManyFunctions")
class SampleViewModel(
    app: Application,
) : AndroidViewModel(app) {
    private val store: SessionStore =
        runCatching<SessionStore> { EncryptedSessionStore(app, "mero_sample_session") }.getOrElse { MemorySessionStore() }

    var mode by mutableStateOf(store.value(KEY_MODE)?.let { runCatching { SignInMode.valueOf(it) }.getOrNull() } ?: SignInMode.CLOUD)
        private set
    var nodeUrl by mutableStateOf(store.value(KEY_NODE) ?: DEFAULT_CLOUD_NODE)
        private set

    /** The session on a real cluster; null in the demo and while signed out of every mode. */
    var session by mutableStateOf<MeroAppSession?>(null)
        private set

    /** What the screens run on: the session's backend, or the demo's. */
    var backend by mutableStateOf<MeroAppBackend?>(null)
        private set
    var busy by mutableStateOf(false)
        private set
    var error by mutableStateOf<String?>(null)
        private set
    var restoring by mutableStateOf(true)
        private set
    private val bumps = MutableStateFlow(0)

    /** Bumped whenever the session changes: screens re-read. */
    val revision: StateFlow<Int> get() = session?.revision ?: bumps

    /** The client the SDK tab drives; null in the demo. */
    val client: CalimeroClient? get() = session?.client

    private var started = false

    /** The Activity's passkey (Credential Manager), while one is on screen: [attach]. */
    private var activityPasskey: PasskeyProvider? = null

    /** The software passkey kept on the device for the development node in use; null on a real cluster. */
    val devPasskey: PasskeyProvider?
        get() = if (mode == SignInMode.DEV) store.value(KEY_DEV_PASSKEY + nodeUrl)?.let { runCatching { SoftPasskey.restore(it) }.getOrNull() } else null

    /**
     * The Activity on screen, or null once it is gone: its passkey approves the account's step-ups
     * (invites, member changes, leaving) on a real cluster. Held only while it lives.
     */
    fun attach(activity: Context?) {
        activityPasskey =
            activity?.let {
                com.calimero.mero.compose
                    .PlatformPasskey(it)
            }
        approve()
    }

    /** Hand the session the passkey that approves its step-ups. */
    private fun approve() {
        session?.passkey = devPasskey ?: activityPasskey
    }

    /** Restore the last session (or start the demo when [demo]). */
    fun start(demo: Boolean) {
        if (started) return
        started = true
        if (demo && MockBackend.AVAILABLE) {
            restoring = false
            return
        }
        viewModelScope.launch {
            try {
                if (store.load() != null && mode != SignInMode.DEMO) {
                    open(mode, nodeUrl).restore()
                }
            } finally {
                restoring = false
            }
        }
    }

    /** Sign in (or, with [create], sign up as [name]) on a real cluster with the device's passkey. */
    fun signInCloud(
        activity: Context,
        url: String,
        create: Boolean,
        name: String,
    ) = attempt {
        val s = open(SignInMode.CLOUD, url)
        if (create) {
            s.signUp(
                com.calimero.mero.compose
                    .PlatformPasskey(activity),
                name,
            )
        } else {
            s.signIn(
                com.calimero.mero.compose
                    .PlatformPasskey(activity),
            )
        }
    }

    /**
     * Sign in to a development cluster (`merod local`) with a software passkey kept on the device:
     * the first time it signs up as [name], after that it signs in with the same key.
     */
    fun signInDev(
        url: String,
        name: String,
    ) = attempt {
        val s = open(SignInMode.DEV, url)
        val saved = store.value(KEY_DEV_PASSKEY + url)?.let { runCatching { SoftPasskey.restore(it) }.getOrNull() }
        if (saved != null) {
            s.signIn(saved)
        } else {
            val passkey = SoftPasskey(rpId = url.toHttpUrlOrNull()?.host?.let(::devRpId) ?: "localhost", origin = DEV_ORIGIN)
            s.signUp(passkey, name.ifBlank { "android" })
            store.setValue(KEY_DEV_PASSKEY + url, passkey.export())
        }
        approve()
    }

    /** The in-memory demo: no network, no passkey. */
    fun startDemo() {
        mode = SignInMode.DEMO
        session = null
        backend = MockBackend.backend()
        bumps.value += 1
    }

    fun signOut() =
        viewModelScope.launch {
            session?.signOut()
            session = null
            backend = null
            bumps.value += 1
        }

    /** A link the app received: the consent page coming back, or an invite. */
    fun handleLink(url: String) =
        viewModelScope.launch {
            session?.handle(url)
        }

    private fun attempt(block: suspend () -> Unit) {
        error = null
        busy = true
        viewModelScope.launch {
            try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = friendly(e)
                SampleLog.log(SampleLog.Level.ERR, "sign-in failed: ${e.message}")
            } finally {
                busy = false
            }
        }
    }

    /** A [MeroAppSession] on [url] under [newMode]'s policy, its backend mirrored into [backend]. */
    private fun open(
        newMode: SignInMode,
        url: String,
    ): MeroAppSession {
        val trimmed = url.trim().trimEnd('/')
        mode = newMode
        nodeUrl = trimmed
        store.setValue(KEY_MODE, newMode.name)
        store.setValue(KEY_NODE, trimmed)
        val policy = if (newMode == SignInMode.DEV) CloudPolicy.development else CloudPolicy.production
        val client =
            CalimeroClient(
                CalimeroClientOptions(baseUrl = trimmed, policy = policy, httpClient = SampleHttp.client),
            )
        val s = MeroAppSession(client, ChatService.PACKAGE_NAME, CONSENT_CALLBACK, store, scope = viewModelScope)
        session = s
        approve()
        viewModelScope.launch { s.backend.collectLatest { backend = it } }
        return s
    }

    private fun friendly(e: Exception): String =
        when (e) {
            is com.calimero.mero.PasskeyCancelled -> "You dismissed the passkey sheet, so nothing changed."
            is com.calimero.mero.tee.TeeTrustException -> "The node did not attest: ${e.message}"
            is com.calimero.mero.CalimeroError -> "${e.code}: ${e.nodeMessage}"
            is java.io.IOException -> "Can't reach $nodeUrl. Is the node running?"
            else -> e.message ?: e.toString()
        }

    companion object {
        /** Where the consent page sends the person back; must match the manifest's intent filter. */
        const val CONSENT_CALLBACK = "mero-sample://consent"

        /** The origin `merod local` allows (`--origins`); a software passkey's client data claims it. */
        const val DEV_ORIGIN = "http://localhost:3000"

        /** `merod local` from the emulator. */
        const val DEFAULT_DEV_NODE = "http://10.0.2.2:2501"
        const val DEFAULT_CLOUD_NODE = ""
        private const val KEY_MODE = "mode"
        private const val KEY_NODE = "nodeUrl"
        private const val KEY_DEV_PASSKEY = "devPasskey:"

        /** The emulator reaches the host as 10.0.2.2, but `merod local`'s relying party is `localhost`. */
        private fun devRpId(host: String): String = if (host == "10.0.2.2" || host == "127.0.0.1") "localhost" else host
    }
}

/** The sample's HTTP client: the SDK's default, logged into [SampleLog]. */
object SampleHttp {
    val client: okhttp3.OkHttpClient by lazy {
        CalimeroClientOptions
            .defaultHttpClient()
            .newBuilder()
            .addInterceptor { chain ->
                val request = chain.request()
                SampleLog.log(SampleLog.Level.REQ, "${request.method} ${request.url.encodedPath}")
                chain.proceed(request)
            }.build()
    }
}
