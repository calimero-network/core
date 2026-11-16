package com.calimero.mero.compose

import android.content.Context
import com.calimero.mero.AppHost
import com.calimero.mero.CalimeroClient
import com.calimero.mero.CalimeroClientOptions
import com.calimero.mero.CalimeroError
import com.calimero.mero.CloudPolicy
import com.calimero.mero.ConsentRequest
import com.calimero.mero.ConsentResult
import com.calimero.mero.ConsentStarted
import com.calimero.mero.LoginRequired
import com.calimero.mero.PasskeyProvider
import com.calimero.mero.ServedPageHandoff
import com.calimero.mero.ServedPagePasskey
import com.calimero.mero.WebAuthenticating
import com.calimero.mero.WebAuthenticationCancelled
import com.calimero.mero.app.CalimeroAppBackend
import com.calimero.mero.app.ContextContract
import com.calimero.mero.app.MeroAppBackend
import com.calimero.mero.app.OrgConsentApprover
import com.calimero.mero.app.OrgInvitation
import com.calimero.mero.app.SignedOutContextContract
import com.calimero.mero.app.servedPageConsent
import com.calimero.mero.consentReturn
import com.calimero.mero.invite.InviteLink
import com.calimero.mero.signInWithServedPage
import com.calimero.mero.storage.EncryptedSessionStore
import com.calimero.mero.storage.SessionStore
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import okhttp3.OkHttpClient
import java.io.IOException

/**
 * Where an app's [MeroAppSession] signs in. (Swift `MeroAppConfig`.)
 *
 * @property nodeUrl one node of the app's cluster (the URL the Calimero Cloud console shows for
 *   it, or `http://10.0.2.2:2501` for `merod local` from the emulator).
 * @property nodes the cluster's other nodes, to move to on `421 WRONG_NODE`.
 * @property policy what the node must attest to: [CloudPolicy.production] (real TDX quotes from a
 *   signed mero-tee release) unless you run `merod local` ([CloudPolicy.development]).
 */
data class MeroAppConfig(
    val nodeUrl: String,
    val nodes: List<String> = emptyList(),
    val policy: CloudPolicy = CloudPolicy.production,
    /** The app's package (`com.example.board`); [MeroAppSession.create] sets it. */
    val packageName: String? = null,
    /** Carries the attestation and the sealed requests; add interceptors (logging, pinning) here. */
    val httpClient: OkHttpClient? = null,
)

/** How a [MeroAppSession] treats links. (Swift `MeroAppSessionOptions`.) */
data class MeroAppSessionOptions(
    /**
     * Whether a link the system handed the app is one of this app's invitations. Null takes
     * [MeroAppSession.isInvitationLink]: a link carrying `invitation=…` whose path names the app's
     * package.
     */
    val isInvitation: ((String) -> Boolean)? = null,
)

/**
 * A Calimero app's signed-in account on Calimero 1.0: a passkey session on the app's cluster
 * (`v2.…` session token, `rt_…` refresh token, kept encrypted), and the [MeroAppBackend] built over
 * it. Keep one for the app's life (in an `AndroidViewModel`, with its `viewModelScope`). (Swift
 * `MeroKitUI.MeroAppSession`.)
 *
 * ```kotlin
 * val session = MeroAppSession.create(app, "com.example.board", "board://consent", MeroAppConfig(nodeUrl), scope = viewModelScope)
 * viewModelScope.launch { session.restore() }
 * // the sign-in button: session.signIn(activity)
 * // every deep link: viewModelScope.launch { session.handle(url) }
 * ```
 *
 * Screens collect [backend] (and re-read when [revision] moves); view models take it as a
 * [MeroAppBackend], so a test hands them mero-testkit's `InMemoryAppBackend`. A generated
 * contract client calls `session.contract.bound(appId)`.
 *
 * @param callbackUrl where the node's consent page sends the person back (an App Link or an app
 *   scheme the app receives): the `returnTo` of [requestConsent]. 0.x: the wallet's callback.
 */
@Suppress("TooManyFunctions")
class MeroAppSession(
    val client: CalimeroClient,
    /** The app's package: what app instances are created from and invitation links carry. */
    val packageName: String,
    val callbackUrl: String,
    val store: SessionStore,
    val options: MeroAppSessionOptions = MeroAppSessionOptions(),
    private val scope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Default),
) {
    private val _backend = MutableStateFlow<MeroAppBackend?>(null)

    /** The signed-in account's backend, or null when signed out. */
    val backend: StateFlow<MeroAppBackend?> = _backend.asStateFlow()

    private val _revision = MutableStateFlow(0)

    /** Bumped whenever [backend] is rebuilt, so screens re-read. */
    val revision: StateFlow<Int> = _revision.asStateFlow()

    private val _isRestoring = MutableStateFlow(false)

    /** [restore] is running. */
    val isRestoring: StateFlow<Boolean> = _isRestoring.asStateFlow()

    private val _isSigningIn = MutableStateFlow(false)

    /** A passkey sign-in or sign-up is running. */
    val isSigningIn: StateFlow<Boolean> = _isSigningIn.asStateFlow()

    private val _lastError = MutableStateFlow<Throwable?>(null)

    /** Why the last sign-in, sign-up or restore failed; cleared by the next attempt. */
    val lastError: StateFlow<Throwable?> = _lastError.asStateFlow()

    private val _pendingInvitation = MutableStateFlow<String?>(null)

    /** An invitation link that arrived ([handle]) and waits to be redeemed. Clear it with [dismissPendingInvitation]. */
    val pendingInvitation: StateFlow<String?> = _pendingInvitation.asStateFlow()

    private val _consentResult = MutableStateFlow<ConsentResult?>(null)

    /** The decision on the request [requestConsent] started, read from the node when the page sent the person back ([handle]). */
    val consentResult: StateFlow<ConsentResult?> = _consentResult.asStateFlow()

    private val _consentError = MutableStateFlow<Throwable?>(null)

    /**
     * Why the last consent return was refused ([handle]): a [com.calimero.mero.CalimeroError] with
     * `CONSENT_NOT_STARTED`, `CONSENT_REQUEST_MISMATCH`, `CONSENT_NOT_DECIDED` or `CONSENT_REPLACED`
     * ([CalimeroClient.resumeConsent]), or the request's own failure. Do not act on the return then.
     */
    val consentError: StateFlow<Throwable?> = _consentError.asStateFlow()

    /** The request [requestConsent] started and no return has settled yet. */
    @Volatile
    private var consentRequestId: String? = null

    /**
     * The passkey that approves the account's step-ups on 1.0 (`412 CONFIRM_REQUIRED`): inviting,
     * member and role changes, leaving an organization or a team, through [backend]. The passkey
     * sheet needs an Activity, so set it from the one on screen and clear it when it goes:
     * `session.passkey = PlatformPasskey(this)` in `onCreate`, `null` in `onDestroy`. While it is
     * null those methods throw [com.calimero.mero.ConfirmRequired]; a dismissed sheet throws
     * [com.calimero.mero.ConfirmCancelled].
     */
    @Volatile var passkey: PasskeyProvider? = null

    /** The page [webPage] opened that waits for its link ([handle]). */
    @Volatile
    private var openPage: CustomTabAuthenticator? = null

    private val restoring = Mutex()
    private var signing: Job? = null
    private var knownAccount: String? = null

    init {
        client.onSessionChange { session ->
            store.save(session)
            if (session == null || session.accountId != knownAccount) {
                knownAccount = session?.accountId
                rebuild()
            }
        }
    }

    // ---- state ---------------------------------------------------------------------------

    /** Signed in. */
    val isSignedIn: Boolean get() = client.session != null

    /** The account id, or null. */
    val account: String? get() = client.session?.accountId

    /** The app's package: what 0.x called its application. */
    val applicationId: String get() = packageName

    /** Whether the account reaches the cluster: `true` while signed in (0.x: "a relay serves it"). */
    val hasRelay: Boolean get() = _backend.value?.hasRelay ?: false

    /** Whether [MeroAppBackend.events] streams: `true` while signed in. */
    val canSubscribe: Boolean get() = _backend.value?.canSubscribe ?: false

    /** The contract in any app instance: [SignedOutContextContract] while signed out. */
    val contract: ContextContract get() = _backend.value?.contract ?: SignedOutContextContract

    // ---- sign-in -------------------------------------------------------------------------

    /**
     * Reconnect the stored session, if there is one, and check it with the node (`GET /auth/me`;
     * an expired session token is refreshed by itself). A refused refresh forgets it; a node that
     * cannot be reached keeps it, so an offline launch stays signed in.
     */
    suspend fun restore() {
        restoring.withLock {
            _isRestoring.value = true
            try {
                val saved = store.load() ?: return@withLock
                client.useSession(saved)
                knownAccount = saved.accountId
                rebuild()
                try {
                    client.auth.me()
                } catch (e: LoginRequired) {
                    client.forget()
                    _lastError.value = e
                } catch (e: IOException) {
                    _lastError.value = e
                }
            } finally {
                _isRestoring.value = false
            }
        }
    }

    /** Sign in with the device's passkey (Credential Manager over [activity]). Watch [isSigningIn] and [lastError]. */
    fun signIn(activity: Context) = launchSignIn { signIn(PlatformPasskey(activity)) }

    /** Create an account named [name] with a new passkey on the device, and sign in. */
    fun signUp(
        activity: Context,
        name: String,
    ) = launchSignIn { signUp(PlatformPasskey(activity), name) }

    /** Sign in with [passkey] (a [com.calimero.mero.SoftPasskey] on a development cluster). */
    suspend fun signIn(passkey: PasskeyProvider) {
        client.signIn(passkey)
        rebuild()
    }

    /** Create an account with [passkey], and sign in. */
    suspend fun signUp(
        passkey: PasskeyProvider,
        name: String,
    ) {
        client.signUp(passkey, name)
        rebuild()
    }

    /** Stop a sign-in that is waiting (the person left the passkey sheet or the served page). */
    fun cancelSignIn() {
        cancelWebPage()
        signing?.cancel()
        _isSigningIn.value = false
    }

    // ---- the package's served page -------------------------------------------------------

    /**
     * Sign in through the package's served page ([ServedPageHandoff]), opened in a Custom Tab over
     * [activity]: the person signs in there with their passkey, and the page sends back a session
     * bound to this package, sealed to a key made for this sign-in only. The page's link comes back
     * to [callback] (`mero-chat://signed-in`): hand it to [handle]. Watch [isSigningIn] and
     * [lastError]. (Swift `MeroClient.signInWithServedPage`.)
     *
     * @param cluster the Calimero Cloud cluster to sign in on; null: the package's primary cluster.
     */
    fun signInWithServedPage(
        activity: Context,
        callback: String,
        cluster: String? = null,
        appsDomain: String = AppHost.DEFAULT_DOMAIN,
    ) = launchSignIn { signInWithServedPage(callback, webPage(activity), cluster, appsDomain) }

    /**
     * [signInWithServedPage] with [opener] showing the page. Answers whether it worked: a page the
     * person left answers `false` with no [lastError]; any other failure lands in [lastError].
     */
    suspend fun signInWithServedPage(
        callback: String,
        opener: WebAuthenticating,
        cluster: String? = null,
        appsDomain: String = AppHost.DEFAULT_DOMAIN,
    ): Boolean {
        _lastError.value = null
        return try {
            val signedIn = client.signInWithServedPage(packageName, callback, opener, cluster, appsDomain)
            store.save(signedIn.session)
            knownAccount = signedIn.accountId
            rebuild()
            true
        } catch (_: WebAuthenticationCancelled) {
            false
        } catch (e: CancellationException) {
            throw e
        } catch (
            @Suppress("TooGenericExceptionCaught") e: Exception,
        ) {
            _lastError.value = e
            false
        }
    }

    /**
     * The passkey the package's own step-ups go through: on Calimero Cloud the app's served page in
     * a Custom Tab over [activity] ([ServedPagePasskey]), its approval coming back to [callback]
     * (hand it to [handle]); on a development node (a client that allows mock attestation)
     * [passkey]. Organization actions go to the consent page ([servedPageConsent]).
     * (Swift `MeroClient.approvalPasskey`.)
     */
    fun approvalPasskey(
        activity: Context,
        callback: String,
        cluster: String? = null,
        appsDomain: String = AppHost.DEFAULT_DOMAIN,
    ): PasskeyProvider? {
        if (client.options.policy.allowMock) return passkey
        return ServedPagePasskey(AppHost.origin(packageName, cluster, appsDomain), callback, webPage(activity), client.options.httpClient)
    }

    /**
     * Where the node's consent page sends a package-bound session back: the package's served page
     * (`{origin}/{version}/?native-consent=1`), which passes the answer on to the app's own link. The
     * node takes only the package's own https origin as `returnTo`, never an app scheme. Null on a
     * development node, where [callbackUrl] is used as it is. (Swift `MeroClient.servedConsentReturnTo`.)
     */
    suspend fun servedConsentReturnTo(
        cluster: String? = null,
        appsDomain: String = AppHost.DEFAULT_DOMAIN,
    ): String? {
        if (client.options.policy.allowMock) return null
        val origin = AppHost.origin(packageName, cluster, appsDomain)
        return ServedPageHandoff.consentReturnTo(origin, AppHost.latestVersion(origin, client.options.httpClient))
    }

    /**
     * Where organization actions are approved: on Calimero Cloud the node's consent page in a Custom
     * Tab over [activity], returning through the package's served page to [callback] (hand that link
     * to [handle]); null on a development node, where the backend calls the node directly. Give it
     * to a [CalimeroAppBackend] as its `consent`. (Swift `MeroClient.servedPageConsent`.)
     *
     * @param confirm runs before the page opens with the node's answer; show its `code`, which the
     *   page shows too. Answer `false` not to open it.
     */
    fun servedPageConsent(
        activity: Context,
        callback: String,
        cluster: String? = null,
        appsDomain: String = AppHost.DEFAULT_DOMAIN,
        confirm: (suspend (ConsentStarted) -> Boolean)? = null,
    ): OrgConsentApprover? = client.servedPageConsent(packageName, callback, webPage(activity), cluster, appsDomain, confirm)

    /** The person left a served or consent page this session opened ([webPage]): its wait ends. */
    fun cancelWebPage() {
        openPage?.cancel()
    }

    /** A Custom Tab over [context] whose link [handle] hands back. */
    fun webPage(context: Context): WebAuthenticating =
        WebAuthenticating { url, callback ->
            val page = CustomTabAuthenticator(context)
            openPage?.cancel()
            openPage = page
            try {
                page.authenticate(url, callback)
            } finally {
                if (openPage === page) openPage = null
            }
        }

    /** Sign out: end the session on the node, and forget it here. [forgetDevice] also clears every stored value. */
    suspend fun signOut(forgetDevice: Boolean = false) {
        try {
            client.signOut()
        } catch (_: CalimeroError) {
            client.forget()
        } catch (_: IOException) {
            client.forget()
        }
        if (forgetDevice) store.clear()
        rebuild()
    }

    /**
     * A link the system handed the app: a page [webPage] opened sending the person back (the served
     * page's sign-in, approval or consent answer), the consent page sending the person back
     * (`callbackUrl?request_id=…&result=…`, confirmed with the node into [consentResult]), or one of
     * this app's invitation links (held in [pendingInvitation] for the person to confirm). Returns
     * whether it was either.
     */
    suspend fun handle(url: String): Boolean {
        if (openPage?.complete(url) == true) return true
        restoring.withLock { }
        if (consentReturn(url) != null) {
            runCatching { client.resumeConsent(url, consentRequestId) }
                .onSuccess {
                    _consentResult.value = it
                    _consentError.value = null
                    consentRequestId = null
                }.onFailure {
                    if (it is CancellationException) throw it
                    _consentResult.value = null
                    _consentError.value = it
                }
            return true
        }
        val recognised = options.isInvitation?.invoke(url) ?: isInvitationLink(url, packageName)
        if (!recognised) return false
        _pendingInvitation.value = url
        return true
    }

    /** [pendingInvitation] decoded, or null. */
    val pendingOrgInvitation: OrgInvitation? get() = _pendingInvitation.value?.let { OrgInvitation.parse(it, packageName) }

    /** Forget [pendingInvitation]: redeemed, or declined. */
    fun dismissPendingInvitation() {
        _pendingInvitation.value = null
    }

    /** Redeem [pendingInvitation] (`POST /invites/{token}/redeem`) and forget it. Answers the organization joined. */
    suspend fun redeemPendingInvitation(): String? {
        val backend =
            _backend.value ?: throw com.calimero.mero.app
                .SignedOutException("redeemPendingInvitation")
        val invitation = pendingOrgInvitation ?: return null
        val joined = backend.joinNamespace(invitation.namespaceId, invitation)
        dismissPendingInvitation()
        didJoinNamespace()
        return joined.namespaceId
    }

    /** After redeeming an invitation: the account is a member at once on 1.0, so screens only re-read. */
    suspend fun didJoinNamespace() {
        _revision.value += 1
    }

    /**
     * Ask the account holder to approve [request] on the node's consent page, opened in a Custom
     * Tab ([ConsentLauncher]). The page sends them back to [callbackUrl]: hand that link to
     * [handle], and read [consentResult] (or [consentError] when the return is not this request's
     * decision). Show the answer's [ConsentStarted.code] in [confirm]
     * before the page opens (answer `false` and it does not): the page shows the same confirmation
     * code. A code that is not the page's throws `CONSENT_CODE_MISMATCH`.
     */
    suspend fun requestConsent(
        context: Context,
        request: ConsentRequest,
        confirm: (suspend (ConsentStarted) -> Boolean)? = null,
    ): ConsentStarted {
        _consentResult.value = null
        _consentError.value = null
        return ConsentLauncher.request(context, client, request, confirm).also { consentRequestId = it.requestId }
    }

    /** Rebuild [backend] from the client's session. A no-op when the account did not change. */
    fun rebuild() {
        val session = client.session
        val next = session?.let { CalimeroAppBackend(client, packageName, it.accountId) { passkey } }
        val current = _backend.value as? CalimeroAppBackend
        if (current?.account == next?.account && (current == null) == (next == null)) return
        _backend.value = next
        _revision.value += 1
    }

    private fun launchSignIn(block: suspend () -> Unit) {
        signing?.cancel()
        _lastError.value = null
        _isSigningIn.value = true
        signing =
            scope.launch {
                try {
                    block()
                } catch (e: CancellationException) {
                    throw e
                } catch (
                    @Suppress("TooGenericExceptionCaught") e: Exception,
                ) {
                    _lastError.value = e
                } finally {
                    _isSigningIn.value = false
                }
            }
    }

    companion object {
        /**
         * A session with its own [CalimeroClient] on [config]'s cluster, its session kept in
         * Keystore-backed encrypted preferences ([EncryptedSessionStore]).
         *
         * @param packageName the app's registry package (`com.example.board`).
         * @param callbackUrl where the consent page sends the person back.
         */
        @Suppress("LongParameterList")
        fun create(
            context: Context,
            packageName: String,
            callbackUrl: String,
            config: MeroAppConfig,
            options: MeroAppSessionOptions = MeroAppSessionOptions(),
            scope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Default),
            store: SessionStore = EncryptedSessionStore(context, "calimero_session_$packageName"),
        ): MeroAppSession {
            val client =
                CalimeroClient(
                    CalimeroClientOptions(
                        baseUrl = config.nodeUrl,
                        nodes = config.nodes,
                        policy = config.policy,
                        httpClient = config.httpClient ?: CalimeroClientOptions.defaultHttpClient(),
                    ),
                )
            return MeroAppSession(client, packageName, callbackUrl, store, options, scope)
        }

        /**
         * Whether [raw] is an invitation to the app with [packageName]: it carries `invitation=…`,
         * and its path names the package. Without a package any invitation link is.
         */
        fun isInvitationLink(
            raw: String,
            packageName: String?,
        ): Boolean {
            val trimmed = raw.trim()
            val query = trimmed.indexOf('?')
            if (query < 0) return false
            val hasToken =
                trimmed
                    .substring(query + 1)
                    .split('&')
                    .any { it.substringBefore('=') == InviteLink.INVITATION_PARAM && it.substringAfter('=', "").isNotEmpty() }
            if (!hasToken) return false
            if (packageName.isNullOrEmpty()) return true
            val path = trimmed.substring(0, query).split('/').map { it.lowercase() }
            return path.contains(packageName.lowercase())
        }
    }
}
