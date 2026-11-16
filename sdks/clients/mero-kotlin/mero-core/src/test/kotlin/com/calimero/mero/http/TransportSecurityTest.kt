package com.calimero.mero.http

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Plain http only to local hosts (SEC-06). (Swift `TransportSecurityTests`.) */
class TransportSecurityTest {
    @After
    fun tearDown() {
        TransportSecurity.allowInsecureHttp = false
    }

    @Test
    fun `local hosts`() {
        for (host in listOf(
            "localhost", "127.0.0.1", "127.8.9.10", "::1", "[::1]", "10.0.2.2", "10.1.2.3", "172.16.0.1",
            "172.31.255.255", "192.168.1.5", "169.254.10.20", "fe80::1", "[fe80::1%en0]", "fd12:3456::1",
            "merod.local", "node.localhost", "LOCALHOST",
        )) {
            assertTrue(host, TransportSecurity.isLocalHost(host))
        }
        for (host in listOf(
            "node.example", "8.8.8.8", "172.32.0.1", "172.15.0.1", "192.169.1.1", "11.0.0.1", "2001:db8::1",
            "local", "evil-local.com", "1.2.3", "", "127.0.0.1.example.com",
        )) {
            assertFalse(host, TransportSecurity.isLocalHost(host))
        }
    }

    @Test
    fun `loopback hosts`() {
        for (host in listOf("localhost", "127.0.0.1", "::1", "[::1]", "10.0.2.2")) assertTrue(host, TransportSecurity.isLoopbackHost(host))
        for (host in listOf("192.168.1.5", "10.0.0.1", "merod.local", "node.example")) assertFalse(host, TransportSecurity.isLoopbackHost(host))
    }

    @Test
    fun `is secure or local`() {
        assertTrue(TransportSecurity.isSecureOrLocal("https://node.example"))
        assertTrue(TransportSecurity.isSecureOrLocal("wss://node.example/ws"))
        assertTrue(TransportSecurity.isSecureOrLocal("http://localhost:2428"))
        assertTrue(TransportSecurity.isSecureOrLocal("http://192.168.1.5:2428"))
        assertFalse(TransportSecurity.isSecureOrLocal("http://node.example"))
        assertFalse(TransportSecurity.isSecureOrLocal("ws://node.example"))
        assertFalse(TransportSecurity.isSecureOrLocal("ftp://localhost"))
        assertFalse(TransportSecurity.isSecureOrLocal("not a url"))
    }

    @Test
    fun `a node may be plain http only when local`() {
        TransportSecurity.checkNodeUrl("https://node.example")
        TransportSecurity.checkNodeUrl("http://localhost:2428")
        TransportSecurity.checkNodeUrl("http://192.168.1.5:2428")
        TransportSecurity.checkNodeUrl("http://10.0.2.2:2428")
        val refused = runCatching { TransportSecurity.checkNodeUrl("http://node.example/auth?t=secret") }.exceptionOrNull()
        assertTrue("$refused", refused is InsecureTransportException)
        refused as InsecureTransportException
        assertEquals("node", refused.role)
        assertEquals("the query is not echoed", "http://node.example/auth", refused.url)
        assertTrue(runCatching { TransportSecurity.checkNodeUrl("ws://node.example/ws") }.isFailure)
    }

    @Test
    fun `a service must be https unless loopback`() {
        TransportSecurity.checkServiceUrl("https://cloud.example", "cloud")
        TransportSecurity.checkServiceUrl("http://127.0.0.1:8080", "cloud")
        TransportSecurity.checkServiceUrl("http://10.0.2.2:8080", "relay")
        assertTrue(runCatching { TransportSecurity.checkServiceUrl("http://192.168.1.5", "relay") }.isFailure)
        assertTrue(runCatching { TransportSecurity.checkServiceUrl("http://cloud.example", "cloud") }.isFailure)
    }

    @Test
    fun `the development opt-in lifts both rules`() {
        TransportSecurity.allowInsecureHttp = true
        TransportSecurity.checkNodeUrl("http://node.example")
        TransportSecurity.checkServiceUrl("http://cloud.example", "cloud")
    }
}
