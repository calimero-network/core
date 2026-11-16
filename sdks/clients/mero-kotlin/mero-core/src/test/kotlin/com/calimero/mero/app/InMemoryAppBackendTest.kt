package com.calimero.mero.app

import com.calimero.mero.testkit.InMemoryAppBackend
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.yield
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.IOException

/**
 * mero-testkit's [InMemoryAppBackend], which apps test their view models against, behaves like a
 * relay as far as a view model can tell. (Swift `InMemoryAppBackendTests`.)
 */
class InMemoryAppBackendTest {
    @Test
    fun `founding then creating fills the listings`() =
        runBlocking {
            val backend = InMemoryAppBackend()
            val founded = backend.foundNamespace("Team")
            val group = backend.createSubgroup(founded.namespaceId, "general", isPublic = true)
            val init = buildJsonObject { put("name", "general") }
            val context = backend.createContext(group, backend.applicationId, "general", init)

            val namespaces = backend.listNamespaces()
            assertEquals(listOf(founded.namespaceId), namespaces.map { it.id })
            assertEquals("Team", namespaces.first().name)
            assertEquals(64, founded.namespaceId.length)
            assertTrue(founded.haEnabled)
            assertEquals(listOf(AppSubgroup(group, "general", listOf(context), isPublic = true)), backend.listSubgroups(founded.namespaceId))
            assertEquals(listOf(context), backend.listContexts(group))
            assertTrue("the creator is a member of what it creates", backend.isMember(context))
            assertEquals(setOf(group), backend.publicSubgroups)
            assertEquals(init, backend.initArgs[context])
            assertEquals(
                listOf(
                    "foundNamespace Team",
                    "createSubgroup ${founded.namespaceId} general open",
                    "createContext $group ${backend.applicationId} general",
                    "listNamespaces",
                    "listSubgroups ${founded.namespaceId}",
                    "listContexts $group",
                    "isMember $context",
                ),
                backend.log,
            )
        }

    @Test
    fun `joining gives a relayless account its relay`() =
        runBlocking {
            val backend = InMemoryAppBackend()
            backend.hasRelay = false
            val namespace = "7d".repeat(32)
            try {
                backend.contract.query("c", "get", JsonObject(emptyMap()))
                fail("a relayless contract refuses")
            } catch (_: SignedOutException) {
            }

            val invitation = InMemoryAppBackend.invitation(namespace)
            val joined = backend.joinNamespace(namespace, invitation)

            assertTrue(joined.published)
            assertEquals("https://relay.test", joined.relayUrl)
            assertTrue(backend.hasRelay)
            assertEquals(listOf(namespace), backend.listNamespaces().map { it.id })
            assertEquals(namespace, invitation.namespaceId)
            assertEquals(invitation, OrgInvitation.decode(invitation.encoded()))
        }

    @Test
    fun `joining a context makes the account a member`() =
        runBlocking {
            val backend = InMemoryAppBackend()
            assertFalse(backend.isMember("ctx"))
            backend.joinContext("ctx")
            assertTrue(backend.isMember("ctx"))
        }

    @Test
    fun `failure fails every admin call`() =
        runBlocking {
            val backend = InMemoryAppBackend()
            backend.failure = IOException("offline")
            try {
                backend.listNamespaces()
                fail("expected the failure")
            } catch (e: IOException) {
                assertEquals("offline", e.message)
            }
            assertEquals(listOf("listNamespaces"), backend.log)
        }

    @Test
    fun `emitted events reach their context's stream only`() =
        runBlocking {
            val backend = InMemoryAppBackend()
            val seen = async { backend.events("a").toList() }
            withTimeout(5_000) { while (backend.subscriberCount == 0) yield() }
            backend.emit(AppContextEvent("b", "StateMutation"))
            backend.emit(AppContextEvent("a", "StateMutation"))
            backend.finishEvents()
            assertEquals(listOf(AppContextEvent("a", "StateMutation")), seen.await())
        }

    @Test
    fun `no stream without a subscription`() =
        runBlocking {
            val backend = InMemoryAppBackend()
            backend.canSubscribe = false
            assertEquals(emptyList<AppContextEvent>(), backend.events("a").toList())
        }
}
