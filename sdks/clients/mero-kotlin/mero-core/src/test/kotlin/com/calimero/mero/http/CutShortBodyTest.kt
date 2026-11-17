package com.calimero.mero.http

import com.calimero.mero.DirectoryError
import com.calimero.mero.DirectoryFailure
import com.calimero.mero.readDirectory
import kotlinx.coroutines.runBlocking
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.mockwebserver.SocketPolicy
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * A body the network cuts short is a transport failure the caller is told of in its own terms,
 * never a bare IOException: the client re-reads its node directory on its own scope, where one
 * escaping would crash the app.
 */
class CutShortBodyTest {
    private lateinit var server: MockWebServer

    @Before
    fun setUp() {
        server = MockWebServer()
        server.start()
    }

    @After
    fun tearDown() {
        server.shutdown()
    }

    private fun cutShort() =
        server.enqueue(
            MockResponse()
                .setHeader("content-type", "application/json")
                .setBody("{\"nodes\":[" + "{\"url\":\"https://n.example\",\"weight\":1},".repeat(4096) + "]}")
                .setSocketPolicy(SocketPolicy.DISCONNECT_DURING_RESPONSE_BODY),
        )

    @Test
    fun `a plain request whose body is cut short fails as a network error`() {
        cutShort()
        val error = runCatching { runBlocking { PlainJsonHttp(role = null).execute("GET", server.url("/x").toString()) } }.exceptionOrNull()
        assertTrue("$error", error is NetworkException)
    }

    @Test
    fun `a directory whose body is cut short is unreachable`() {
        cutShort()
        val error = runCatching { runBlocking { readDirectory(server.url("/clusters").toString(), "c1") } }.exceptionOrNull()
        assertTrue("$error", error is DirectoryError)
        assertEquals(DirectoryFailure.Unreachable, (error as DirectoryError).reason)
    }
}
