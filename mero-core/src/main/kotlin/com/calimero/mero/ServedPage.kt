package com.calimero.mero

import com.calimero.mero.crypto.randomBytes
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import okhttp3.OkHttpClient
import org.bouncycastle.asn1.sec.SECNamedCurves
import org.bouncycastle.crypto.digests.SHA256Digest
import org.bouncycastle.crypto.generators.HKDFBytesGenerator
import org.bouncycastle.crypto.params.ECDomainParameters
import org.bouncycastle.crypto.params.HKDFParameters
import java.math.BigInteger
import java.net.URLDecoder
import java.util.concurrent.atomic.AtomicBoolean
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * Opens a page in the system browser and answers the link it sends the person back with
 * (Swift `WebAuthenticating`). mero-compose's `CustomTabAuthenticator` is a Custom Tab whose
 * callback deep link the app hands back (`MeroAppSession.handle`).
 */
fun interface WebAuthenticating {
    /** Present [url]; answer the link it sent the person back to, under [callback] (fragment included). */
    suspend fun authenticate(
        url: String,
        callback: String,
    ): String
}

/** The person left the page without it sending them back. */
class WebAuthenticationCancelled(
    message: String = "The page was dismissed.",
) : Exception(message)

/** What a package's served page hands a native app after it signed the person in. */
@Serializable
data class ServedPageSession(
    val v: Int = 1,
    val accountId: String,
    val sessionToken: String,
    val expiresAt: Double,
    val refreshToken: String,
    /** The node the page signed in on, and the rest of its cluster. */
    val nodeUrl: String,
    val nodes: List<String> = emptyList(),
) {
    /** The session, as a client uses it. */
    val session: CalimeroSession get() = CalimeroSession(accountId, sessionToken, expiresAt.toLong(), refreshToken)

    override fun toString(): String = "ServedPageSession(accountId=$accountId, nodeUrl=$nodeUrl, nodes=$nodes)"
}

/** Why a served page's answer was refused. */
class ServedPageSignInError(
    message: String,
    cause: Throwable? = null,
) : Exception(message, cause)

/**
 * One native sign-in (or approval) through a package's served page.
 * (== apps `@calimero-apps/passkey/native`; Swift `ServedPageHandoff`.)
 *
 * A native app can't use a `calimero.network` passkey to sign in to the package itself: the
 * assertion would come from an origin that matches no package, so the node would open an unbound
 * session. Instead the app opens its package's served page in the system browser ([url]). The
 * person signs in there as on the web, which binds the session to the package, and the page sends
 * the session back to the app's callback, sealed to this hand-off's key ([open]).
 *
 * Each hand-off makes a fresh P-256 key and [state], and opens one answer only. The box is
 * `base64url(epk ‖ iv ‖ ciphertext)`: ECDH P-256 with `epk`, HKDF-SHA256 (empty salt, info
 * [INFO], 32 bytes), AES-256-GCM with the UTF-8 [state] as additional data.
 */
class ServedPageHandoff(
    /** The private scalar (32 bytes, big-endian). */
    privateKey: ByteArray = newPrivateKey(),
    val state: String = b64url(randomBytes(32)),
) {
    private val d = BigInteger(1, privateKey)
    private val used = AtomicBoolean(false)

    init {
        require(privateKey.size == 32 && d.signum() > 0 && d < DOMAIN.n) { "a P-256 private key" }
    }

    /** The public key the page seals to: base64url of the uncompressed point (65 bytes). */
    val publicKey: String =
        b64url(
            DOMAIN.g
                .multiply(d)
                .normalize()
                .getEncoded(false),
        )

    /**
     * The page to open: `{origin}/{version}/#native-sign-in&state=…&key=…`. The parameters ride in
     * the fragment, so no server sees them.
     */
    fun url(
        origin: String,
        version: String,
    ): String = url(origin, version, FRAGMENT)

    /** `{origin}/{version}/#{fragment}&state=…&key=…`, then [extra] (base64url or rp-id values, so nothing in them needs escaping). */
    fun url(
        origin: String,
        version: String,
        fragment: String,
        extra: List<Pair<String, String>> = emptyList(),
    ): String {
        val more = extra.joinToString("") { (k, v) -> "&$k=$v" }
        return "${origin.trimEnd('/')}/$version/#$fragment&state=$state&key=$publicKey$more"
    }

    /**
     * The session in the page's answer (`callback#state=…&box=…`). Throws [ServedPageSignInError] on
     * another `state`, a box this key can't open, or a second answer.
     */
    @Suppress("ThrowsCount")
    fun open(callback: String): ServedPageSession {
        val plain = openBox(callback, INFO)
        val session =
            try {
                json.decodeFromString(ServedPageSession.serializer(), plain.decodeToString())
            } catch (e: kotlinx.serialization.SerializationException) {
                throw ServedPageSignInError("the sign-in answer could not be opened", e)
            } catch (e: IllegalArgumentException) {
                throw ServedPageSignInError("the sign-in answer could not be opened", e)
            }
        if (session.v != 1) throw ServedPageSignInError("unknown sign-in answer version ${session.v}")
        return session
    }

    /**
     * The assertion in an approval page's answer. Throws [PasskeyCancelled] when the person declined
     * there (`#state=…&cancelled=1`).
     */
    @Suppress("ThrowsCount")
    fun openApproval(callback: String): AssertionJSON {
        if (value("cancelled", callback) == "1" && value("state", callback) == state) throw PasskeyCancelled("the approval was declined")
        val plain = openBox(callback, APPROVE_INFO)
        val answer =
            try {
                json.decodeFromString(Approval.serializer(), plain.decodeToString())
            } catch (e: kotlinx.serialization.SerializationException) {
                throw ServedPageSignInError("the approval answer could not be opened", e)
            } catch (e: IllegalArgumentException) {
                throw ServedPageSignInError("the approval answer could not be opened", e)
            }
        if (answer.v != 1) throw ServedPageSignInError("the approval answer could not be opened")
        return answer.assertion
    }

    @Serializable
    private data class Approval(
        val v: Int,
        val assertion: AssertionJSON,
    )

    /** The plaintext of `callback#state=…&box=…`, opened once, under HKDF [info]. */
    @Suppress("ThrowsCount")
    private fun openBox(
        callback: String,
        info: String,
    ): ByteArray {
        val got = value("state", callback)
        val box = value("box", callback)
        if (got == null || box == null) throw ServedPageSignInError("the page sent nothing back")
        if (got != state) throw ServedPageSignInError("the answer is not for this request")
        if (!used.compareAndSet(false, true)) throw ServedPageSignInError("this request was already answered")
        val bytes =
            try {
                fromB64url(box)
            } catch (e: IllegalArgumentException) {
                throw ServedPageSignInError("the answer is malformed", e)
            }
        if (bytes.size <= POINT + IV + TAG) throw ServedPageSignInError("the answer is malformed")
        @Suppress("TooGenericExceptionCaught")
        return try {
            val epk = DOMAIN.curve.decodePoint(bytes.copyOfRange(0, POINT))
            val shared = epk.multiply(d).normalize()
            require(!shared.isInfinity)
            val ikm = shared.affineXCoord.encoded
            val key = ByteArray(32)
            HKDFBytesGenerator(SHA256Digest()).apply { init(HKDFParameters(ikm, null, info.encodeToByteArray())) }.generateBytes(key, 0, 32)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, SecretKeySpec(key, "AES"), GCMParameterSpec(TAG * 8, bytes, POINT, IV))
            cipher.updateAAD(state.encodeToByteArray())
            cipher.doFinal(bytes, POINT + IV, bytes.size - POINT - IV)
        } catch (e: Exception) {
            throw ServedPageSignInError("the answer could not be opened", e)
        }
    }

    companion object {
        /** A sign-in's HKDF info. */
        const val INFO: String = "calimero.native-sign-in.v1"

        /** The fragment that marks a native sign-in on the page. */
        const val FRAGMENT: String = "native-sign-in"

        /** An approval's HKDF info ([ServedPagePasskey]). */
        const val APPROVE_INFO: String = "calimero.native-approve.v1"

        /** An approval's fragment ([ServedPagePasskey]). */
        const val APPROVE_FRAGMENT: String = "native-approve"

        /**
         * The served page as a consent page's `returnTo`: it passes the answer on to the app
         * (`callback?request_id=…&result=…`). Same as [com.calimero.mero.consentReturnTo].
         */
        fun consentReturnTo(
            origin: String,
            version: String,
        ): String = com.calimero.mero.consentReturnTo(origin, version)

        private const val POINT = 65
        private const val IV = 12
        private const val TAG = 16

        private val DOMAIN: ECDomainParameters =
            SECNamedCurves.getByName("secp256r1").let { ECDomainParameters(it.curve, it.g, it.n, it.h) }

        private val json = Json { ignoreUnknownKeys = true }

        private fun newPrivateKey(): ByteArray {
            while (true) {
                val bytes = randomBytes(32)
                val d = BigInteger(1, bytes)
                if (d.signum() > 0 && d < DOMAIN.n) return bytes
            }
        }

        /** The value of [name] in [callback]'s fragment. */
        private fun value(
            name: String,
            callback: String,
        ): String? {
            val fragment = callback.substringAfter('#', "")
            return fragment
                .split('&')
                .firstOrNull { it.substringBefore('=') == name && '=' in it }
                ?.substringAfter('=')
                ?.let { URLDecoder.decode(it, "UTF-8") }
        }
    }
}

/**
 * The account's passkey for the package's own step-ups (`412 CONFIRM_REQUIRED` over one of the
 * package's own app instances), asked on the app's served page.
 * (== apps `@calimero-apps/passkey/native`, `#native-approve`; Swift `ServedPagePasskey`.)
 *
 * Organization actions are not approved here. A session from the served page is bound to the
 * package, and the node refuses it organization, team, member and invite routes and any `org:` /
 * `account:` step-up: the served page is the publisher's code, and a passkey prompt does not show
 * what it signs. Ask for those on the node's consent page instead
 * ([com.calimero.mero.app.ServedPageConsent], or [CalimeroClient.requestConsent] with
 * [ConsentRequest.CreateOrg], [ConsentRequest.CreateInvite], [ConsentRequest.AcceptInvite], … and
 * [consentReturnTo] as `returnTo`).
 *
 * The node accepts the package's step-ups only from the package's own origin, so [get] opens the
 * served page in [opener], the person approves there with their passkey, and the assertion comes
 * back sealed to a key made for this one approval.
 */
class ServedPagePasskey(
    /** The package's frontend origin ([AppHost.origin]). */
    val origin: String,
    /** The app's own link the page sends the approval back to (`mero-chat://approved`). */
    val callback: String,
    val opener: WebAuthenticating,
    val httpClient: OkHttpClient = CalimeroClientOptions.defaultHttpClient(),
) : PasskeyProvider {
    /** Approvals only: a new passkey is made on the served page's sign-up. */
    override suspend fun create(options: kotlinx.serialization.json.JsonObject): RegistrationJSON =
        throw ServedPageSignInError("a new passkey is made on the sign-in page")

    override suspend fun get(
        challenge: ByteArray,
        rpId: String,
        allowCredentials: List<String>?,
    ): AssertionJSON {
        val version = AppHost.latestVersion(origin, httpClient)
        val handoff = ServedPageHandoff()
        val extra = mutableListOf("challenge" to b64url(challenge), "rpId" to rpId)
        if (!allowCredentials.isNullOrEmpty()) extra += "allow" to allowCredentials.joinToString(",")
        val url = handoff.url(origin, version, ServedPageHandoff.APPROVE_FRAGMENT, extra)
        val answer =
            try {
                opener.authenticate(url, callback)
            } catch (e: WebAuthenticationCancelled) {
                throw PasskeyCancelled(cause = e)
            }
        return handoff.openApproval(answer)
    }
}

/**
 * Sign in through the package's served page ([ServedPageHandoff]): [opener] shows the page, the
 * person signs in there with their passkey, and the page sends back a session bound to
 * [packageName], sealed to a key made for this sign-in only. This client then uses that session
 * ([CalimeroClient.useSession]); keep the answer's session as any other. (Swift
 * `MeroClient.signInWithServedPage`.)
 *
 * @param callback the app's own link the page sends the session back to (`mero-chat://signed-in`).
 * @param cluster the Calimero Cloud cluster to sign in on (its own host for the package); null:
 *   the package's primary cluster.
 * @throws WebAuthenticationCancelled when the person left the page.
 */
suspend fun CalimeroClient.signInWithServedPage(
    packageName: String,
    callback: String,
    opener: WebAuthenticating,
    cluster: String? = null,
    appsDomain: String = AppHost.DEFAULT_DOMAIN,
): ServedPageSession {
    val origin = AppHost.origin(packageName, cluster, appsDomain)
    val version = AppHost.latestVersion(origin, options.httpClient)
    val handoff = ServedPageHandoff()
    val signedIn = handoff.open(opener.authenticate(handoff.url(origin, version), callback))
    useSession(signedIn.session)
    return signedIn
}
