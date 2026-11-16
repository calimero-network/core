package com.calimero.mero.invite

/**
 * Shareable invitation links, matching `@calimero-network/mero-platform`.
 *
 * A link carries an [com.calimero.mero.app.OrgInvitation] (`base58(deflate({invitation, namespaceId, …}))`).
 * Historically a native app was a client of a node on a computer, so the *link* is
 * not something the phone opens — it is something the phone hands to a person,
 * who opens it on the machine with the desktop app. There the launcher resolves
 * the slug, installs the app if it is missing, and joins the namespace. What the
 * phone needs is only two things: build the link, and accept one somebody pastes
 * back in.
 *
 * That is why there is no intent-filter or App Links setup here. Nothing about
 * this requires Android to route a link to the app.
 */
object InviteLink {
    /**
     * Where shareable links point. Always HTTPS: an HTTPS link opens the web
     * build directly and hands off to the desktop launcher on a machine that has
     * it, whereas `calimero://` is a device-local transport that does not survive
     * being pasted into a chat window.
     */
    const val DEFAULT_HOST = "https://links.calimero.network"

    /** The intent verb an invitation carries. */
    const val JOIN_ACTION = "join"

    /** The query parameter the payload travels in. */
    const val INVITATION_PARAM = "invitation"

    /**
     * Build a canonical shareable link.
     *
     * [slug] is the app's package id (e.g. `com.calimero.mero-tag`). The desktop
     * launcher matches it against `Application.package`, so it must equal the id
     * the app is published under — not a display name.
     */
    fun create(
        slug: String,
        action: String = JOIN_ACTION,
        params: Map<String, String> = emptyMap(),
        host: String = DEFAULT_HOST,
    ): String {
        val base = host.trimEnd('/')
        val path = "${encode(slug)}/${encode(action)}"
        if (params.isEmpty()) return "$base/$path"
        // Sorted so a given invitation always produces the same string.
        val query =
            params.toSortedMap().entries.joinToString("&") { (k, v) ->
                "${encode(k)}=${encode(v)}"
            }
        return "$base/$path?$query"
    }

    /** The shareable link for an invitation token. */
    fun invitation(
        token: String,
        slug: String,
        host: String = DEFAULT_HOST,
    ): String {
        val params = mapOf(INVITATION_PARAM to token)
        return create(slug = slug, params = params, host = host)
    }

    /**
     * Pull the invitation token out of whatever a person pasted: a shareable
     * HTTPS link, a `calimero://` link, or the bare token.
     *
     * The query is split by hand rather than with `android.net.Uri` or
     * `java.net.URI`, for two reasons: this stays unit-testable on the JVM with
     * no device, and host parsing on a non-special scheme mangles a dotted slug
     * like `com.calimero.mero-tag`.
     *
     * [expectedSlug], when given, makes a link carrying a *different* app's slug return null
     * instead of its token. Pass your own package id: another app's invitation decodes perfectly
     * well, since the payload shape is shared, and redeeming it would join a namespace belonging to
     * that app. This mirrors the JS `invitationFromRaw`, which rejects a foreign slug for the same
     * reason. Null (the default) accepts any slug. A *bare token* carries no slug and is always
     * accepted, the same latitude the web apps give a pasted code. (Swift
     * `InviteLink.token(fromPasted:expectedSlug:)`.)
     *
     * Returns null for blank input, or for a link whose slug was rejected — an unrecognized string
     * is handed back unchanged, so a bare token still works and the codec gets the final say.
     */
    @JvmOverloads
    fun tokenFromPasted(
        input: String,
        expectedSlug: String? = null,
    ): String? {
        val trimmed = input.trim()
        if (trimmed.isEmpty()) return null

        val lower = trimmed.lowercase()
        val isHttp = lower.startsWith("http://")
        val isHttps = lower.startsWith("https://")
        val isScheme = lower.startsWith("calimero://")
        if (!isHttp && !isHttps && !isScheme) return trimmed
        if (expectedSlug != null && !slugMatches(trimmed, expectedSlug)) return null
        return invitationParam(trimmed)
    }

    /** The link's `invitation` parameter, percent-decoded; the link itself when it has none. */
    private fun invitationParam(trimmed: String): String {
        val queryStart = trimmed.indexOf('?')
        if (queryStart < 0) return trimmed

        // Expressed as a lookup rather than a loop: a loop here needs both a
        // `continue` for the other parameters and a `return` for the hit, which
        // detekt flags (LoopWithTooManyJumpStatements) and which reads worse.
        // A pair with no '=' yields the whole string from `substringBefore`,
        // which cannot equal the parameter name, so it is skipped for free.
        val query = trimmed.substring(queryStart + 1)
        val pairs = query.split("&")
        val match = pairs.firstOrNull { it.substringBefore('=') == INVITATION_PARAM }
        if (match == null) return trimmed
        return decodePercent(match.substringAfter('=', ""))
    }

    /**
     * Does this link's `/<slug>/` path segment match? Compared as a path segment rather than a
     * substring so `com.calimero.mero` cannot match `com.calimero.mero-ar`; case-insensitive,
     * because a link that has been through a mail client may not come back the way it left.
     */
    private fun slugMatches(
        link: String,
        expected: String,
    ): Boolean {
        val segments = link.substringBefore('?').split('/').map { it.lowercase() }
        return expected.lowercase() in segments
    }

    /** Percent-encode everything a token or slug may contain. */
    private fun encode(value: String): String {
        val safe = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~"
        val out = StringBuilder(value.length)
        for (byte in value.toByteArray(Charsets.UTF_8)) {
            val ch = byte.toInt().toChar()
            if (safe.indexOf(ch) >= 0) {
                out.append(ch)
            } else {
                out.append('%')
                out.append("%02X".format(byte.toInt() and 0xff))
            }
        }
        return out.toString()
    }

    private fun decodePercent(value: String): String {
        if (!value.contains('%') && !value.contains('+')) return value
        val bytes = java.io.ByteArrayOutputStream(value.length)
        var i = 0
        while (i < value.length) {
            val ch = value[i]
            when {
                ch == '%' && i + 2 < value.length -> {
                    val hex = value.substring(i + 1, i + 3).toIntOrNull(16)
                    if (hex == null) {
                        bytes.write(ch.code)
                        i++
                    } else {
                        bytes.write(hex)
                        i += 3
                    }
                }
                else -> {
                    for (b in ch.toString().toByteArray(Charsets.UTF_8)) {
                        bytes.write(b.toInt())
                    }
                    i++
                }
            }
        }
        return bytes.toByteArray().toString(Charsets.UTF_8)
    }
}
