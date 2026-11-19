package com.calimero.mero.http

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import okhttp3.Call
import okhttp3.Callback
import okhttp3.Request
import okhttp3.Response
import okio.Timeout
import java.io.IOException
import java.util.concurrent.atomic.AtomicBoolean

/**
 * A failure that is not an [IOException] (a typed refusal, a cancelled passkey prompt) on its way
 * through OkHttp's [Callback], which carries only [IOException]s. The generated `ApiClient`
 * (mero-core/src/generated) unwraps it, so a caller of the generated API catches the
 * [com.calimero.mero.CalimeroError] the node meant, as with every hand-written method.
 */
class TypedFailure(
    override val cause: Throwable,
) : IOException(cause.message, cause)

/**
 * The generated API's transport: a [Call.Factory] whose calls go through [send], the client's own
 * pipeline (the node in use, its sealed channel, the credential, `401` refresh, `421` failover,
 * `503` retries of calls and views, `412`/`428` step-ups). [send] answers a successful response or
 * throws; a refusal never comes back as a response.
 */
internal class MeroCallFactory(
    private val scope: CoroutineScope,
    private val send: suspend (Request) -> Response,
) : Call.Factory {
    override fun newCall(request: Request): Call = MeroCall(request, scope, send)
}

private class MeroCall(
    private val original: Request,
    private val scope: CoroutineScope,
    private val send: suspend (Request) -> Response,
) : Call {
    private val executed = AtomicBoolean()

    @Volatile private var canceled = false

    @Volatile private var job: Job? = null

    override fun request(): Request = original

    override fun execute(): Response {
        check(executed.compareAndSet(false, true)) { "Already Executed" }
        if (canceled) throw IOException("Canceled")
        return runBlocking { send(original) }
    }

    override fun enqueue(responseCallback: Callback) {
        check(executed.compareAndSet(false, true)) { "Already Executed" }
        job =
            scope.launch {
                val response =
                    try {
                        if (canceled) throw IOException("Canceled")
                        send(original)
                    } catch (e: CancellationException) {
                        responseCallback.onFailure(this@MeroCall, IOException("Canceled", e))
                        return@launch
                    } catch (e: IOException) {
                        responseCallback.onFailure(this@MeroCall, e)
                        return@launch
                    } catch (
                        @Suppress("TooGenericExceptionCaught") e: Exception,
                    ) {
                        responseCallback.onFailure(this@MeroCall, TypedFailure(e))
                        return@launch
                    }
                responseCallback.onResponse(this@MeroCall, response)
            }
    }

    override fun cancel() {
        canceled = true
        job?.cancel()
    }

    override fun isExecuted(): Boolean = executed.get()

    override fun isCanceled(): Boolean = canceled

    override fun timeout(): Timeout = Timeout.NONE

    override fun clone(): Call = MeroCall(original, scope, send)
}
