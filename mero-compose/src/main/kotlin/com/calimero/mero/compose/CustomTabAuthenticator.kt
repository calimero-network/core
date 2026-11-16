package com.calimero.mero.compose

import android.content.Context
import com.calimero.mero.WebAuthenticating
import com.calimero.mero.WebAuthenticationCancelled
import kotlinx.coroutines.CompletableDeferred
import java.util.concurrent.atomic.AtomicReference

/**
 * [WebAuthenticating] over a Custom Tab (Swift `SystemWebAuthenticator`): opens the page in the
 * system browser and waits for the link it sends the person back with under the callback. The app
 * receives that link as a deep link; hand it to [complete] (or to [MeroAppSession.handle], which
 * does so for the pages it opened). A Custom Tab says nothing when the person closes it: call
 * [cancel] then (say, when the activity resumes without the link), and the wait throws
 * [WebAuthenticationCancelled].
 */
class CustomTabAuthenticator(
    private val context: Context,
) : WebAuthenticating {
    private class Pending(
        val callback: String,
        val answer: CompletableDeferred<String> = CompletableDeferred(),
    )

    private val pending = AtomicReference<Pending?>(null)

    /** Whether a page is open and waiting for its link. */
    val isWaiting: Boolean get() = pending.get() != null

    override suspend fun authenticate(
        url: String,
        callback: String,
    ): String {
        val mine = Pending(callback)
        pending.getAndSet(mine)?.answer?.completeExceptionally(WebAuthenticationCancelled())
        try {
            ConsentLauncher.open(context, url)
            return mine.answer.await()
        } finally {
            pending.compareAndSet(mine, null)
        }
    }

    /** Hand the open page its link: whether [url] is the one it waits for (under its callback). */
    fun complete(url: String): Boolean {
        val open = pending.get() ?: return false
        if (!isUnder(url, open.callback)) return false
        if (!pending.compareAndSet(open, null)) return false
        return open.answer.complete(url)
    }

    /** The person left the page: the wait throws [WebAuthenticationCancelled]. */
    fun cancel() {
        pending.getAndSet(null)?.answer?.completeExceptionally(WebAuthenticationCancelled())
    }

    companion object {
        /** Whether [url] is [callback], or [callback] followed by a query, fragment or path. */
        fun isUnder(
            url: String,
            callback: String,
        ): Boolean {
            val base = callback.substringBefore('?').substringBefore('#').trimEnd('/')
            if (!url.startsWith(base, ignoreCase = false)) return false
            val rest = url.substring(base.length)
            return rest.isEmpty() || rest[0] == '?' || rest[0] == '#' || rest[0] == '/'
        }
    }
}
