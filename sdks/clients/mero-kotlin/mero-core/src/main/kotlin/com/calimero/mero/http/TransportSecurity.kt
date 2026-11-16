package com.calimero.mero.http

import java.net.URI

/**
 * A request was refused before it left the device: it would have carried credentials or tokens
 * over plaintext `http://` / `ws://` to a host that is not local. (Swift `InsecureTransportError`.)
 *
 * @property url the URL refused, without its query or fragment.
 * @property role what it was for: `node`, `cloud`, `relay`, `registry`, `wallet`, `release mirror`.
 */
class InsecureTransportException(
    val url: String,
    val role: String,
) : MeroException(
        "Refusing to reach the $role at $url over plaintext: use https" +
            (if (role == "node") ", or a local address (loopback, link-local, private LAN, .local)" else "") +
            ". For a development rig, set TransportSecurity.allowInsecureHttp.",
    )

/**
 * Which URLs the SDK will speak plaintext `http://` / `ws://` to. (Swift `TransportSecurity`.)
 *
 * - A **node** may be plain http only when its host is local: loopback, link-local
 *   (`169.254/16`, `fe80::/10`), a private LAN address (RFC 1918, `fc00::/7`), or a `.local` /
 *   `.localhost` name. A dev node on your machine or LAN (or the emulator's `10.0.2.2`) keeps
 *   working; a password or token is never sent in the clear to a host across the internet. A
 *   client sealed to an attested TEE ([com.calimero.mero.tee.SealedTransport]) is end-to-end
 *   encrypted, so its node URL is not checked.
 * - The **Cloud** manager, a **relay**, the **registry**, the **wallet** and the **release
 *   mirror** must be https, except on loopback (a dev rig on this machine, or the Android
 *   emulator's host alias `10.0.2.2`).
 *
 * [allowInsecureHttp] lifts both rules. Development only.
 */
object TransportSecurity {
    /**
     * **Development only.** Send plain http/ws to any host. Leave it false in a shipped app: a
     * credential or token sent this way can be read and replayed by anyone on the path.
     */
    @Volatile
    @JvmStatic
    var allowInsecureHttp: Boolean = false

    /** Whether [url] is https/wss, or http/ws to a local host — what a login form should check before it asks for a password. */
    @JvmStatic
    fun isSecureOrLocal(url: String): Boolean {
        val uri = runCatching { URI(url.trim()) }.getOrNull() ?: return false
        return when (uri.scheme?.lowercase()) {
            "https", "wss" -> true
            "http", "ws" -> isLocalHost(uri.host.orEmpty())
            else -> false
        }
    }

    /** Throw [InsecureTransportException] if [url] is plain http/ws to a node host that is not local (unless [allowInsecureHttp]). */
    @JvmStatic
    fun checkNodeUrl(url: String) {
        val uri = plaintext(url) ?: return
        if (!isLocalHost(uri.host.orEmpty())) throw InsecureTransportException(redacted(uri), "node")
    }

    /**
     * Throw [InsecureTransportException] if [url] (a Cloud, relay, registry, wallet or mirror URL)
     * is plain http/ws to anything but loopback (unless [allowInsecureHttp]).
     */
    @JvmStatic
    fun checkServiceUrl(
        url: String,
        role: String,
    ) {
        val uri = plaintext(url) ?: return
        if (!isLoopbackHost(uri.host.orEmpty())) throw InsecureTransportException(redacted(uri), role)
    }

    /** `localhost`, `127.0.0.0/8`, `::1`, and the Android emulator's alias for its host machine, `10.0.2.2`. */
    @JvmStatic
    fun isLoopbackHost(host: String): Boolean {
        val h = normalized(host)
        if (h == "localhost" || h == "::1" || h == "10.0.2.2") return true
        return ipv4(h)?.first() == 127
    }

    /** Loopback, link-local, a private LAN address, or a `.local` / `.localhost` name. */
    @JvmStatic
    fun isLocalHost(host: String): Boolean {
        val h = normalized(host)
        if (h.isEmpty()) return false
        if (isLoopbackHost(h) || h.endsWith(".local") || h.endsWith(".localhost")) return true
        val octets = ipv4(h)
        return if (octets != null) isPrivateIpv4(octets[0], octets[1]) else isPrivateIpv6(h)
    }

    /** [url] parsed, when it is plain http/ws and the rules apply; null otherwise. */
    private fun plaintext(url: String): URI? {
        if (allowInsecureHttp) return null
        val uri = runCatching { URI(url.trim()) }.getOrNull() ?: return null
        val scheme = uri.scheme?.lowercase()
        return uri.takeIf { scheme == "http" || scheme == "ws" }
    }

    /** The URL without its query, fragment or user info, which may carry a token. */
    private fun redacted(uri: URI): String =
        runCatching { URI(uri.scheme, null, uri.host, uri.port, uri.path, null, null).toString() }
            .getOrDefault(uri.host.orEmpty())
}

/** RFC 1918 or link-local (`169.254/16`). */
private fun isPrivateIpv4(
    a: Int,
    b: Int,
): Boolean = a == 10 || (a == 172 && b in 16..31) || (a == 192 && b == 168) || (a == 169 && b == 254)

/** Link-local (`fe80::/10`) or unique local (`fc00::/7`). */
private fun isPrivateIpv6(host: String): Boolean {
    if (!host.contains(':')) return false
    val word = host.substringBefore(':').toIntOrNull(16) ?: return false
    return word and 0xffc0 == 0xfe80 || word and 0xfe00 == 0xfc00
}

private fun normalized(host: String): String {
    var h = host.lowercase().removePrefix("[").removeSuffix("]")
    h = h.substringBefore('%')
    return h.trimEnd('.')
}

private fun ipv4(host: String): List<Int>? {
    val parts = host.split('.')
    if (parts.size != 4) return null
    val octets = parts.map { part -> part.takeIf { it.length in 1..3 && it.all { c -> c in '0'..'9' } }?.toInt()?.takeIf { it <= 255 } }
    return octets.takeIf { all -> all.none { it == null } }?.map { it!! }
}
