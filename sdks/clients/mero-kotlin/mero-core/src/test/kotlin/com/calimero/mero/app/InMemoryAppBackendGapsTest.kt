package com.calimero.mero.app

import com.calimero.mero.testkit.InMemoryAppBackend
import com.calimero.mero.testkit.RecordingContextContract
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** Names, blobs, leaving and members on the in-memory backend. (Swift `InMemoryAppBackendGapsTests`.) */
class InMemoryAppBackendGapsTest {
    private class Seeded(
        val backend: InMemoryAppBackend,
        val namespace: String,
        val group: String,
        val context: String,
    )

    private suspend fun seeded(): Seeded {
        val backend = InMemoryAppBackend()
        val namespace = backend.foundNamespace("Team").namespaceId
        val group = backend.createSubgroup(namespace, "boards", isPublic = true)
        val context = backend.createContext(group, backend.applicationId, "Roadmap", JsonObject(emptyMap()))
        return Seeded(backend, namespace, group, context)
    }

    private inline fun <reified T : Throwable> assertThrows(block: () -> Unit) {
        try {
            block()
            fail("expected ${T::class.simpleName}")
        } catch (e: Throwable) {
            if (e !is T) throw e
        }
    }

    @Test
    fun `contexts and groups carry their names`() =
        runBlocking {
            val s = seeded()
            val backend = s.backend
            assertEquals(listOf(AppContextEntry(s.context, "Roadmap")), backend.listContextEntries(s.group))
            assertEquals(AppMetadata("Roadmap"), backend.contextMetadata(s.group, s.context))
            assertEquals("boards", backend.groupMetadata(s.group)?.name)
            assertNull(backend.groupMetadata("nope"))

            backend.setContextName(s.group, s.context, "Q3")
            backend.setGroupName(s.group, "planning")
            backend.setGroupName(s.namespace, "Crew")
            assertEquals("Q3", backend.listContextEntries(s.group).first().name)
            assertEquals("planning", backend.listSubgroups(s.namespace).first().name)
            assertEquals("Crew", backend.listNamespaces().first().name)
            assertThrows<IllegalArgumentException> { runBlocking { backend.setGroupName("nope", "x") } }
        }

    @Test
    fun `a blob reads back only through its context`() =
        runBlocking {
            val s = seeded()
            val backend = s.backend
            val blob = backend.uploadBlob(s.context, "hello".toByteArray())
            assertEquals(5L, blob.size)
            assertArrayEquals("hello".toByteArray(), backend.getBlob(blob.blobId, s.context))
            assertArrayEquals("hello".toByteArray(), backend.blobs[blob.blobId])
            assertThrows<IllegalArgumentException> { runBlocking { backend.getBlob(blob.blobId, "other") } }

            backend.hasRelay = false
            assertThrows<SignedOutException> { runBlocking { backend.uploadBlob(s.context, ByteArray(0)) } }
        }

    @Test
    fun `members, roles and names`() =
        runBlocking {
            val s = seeded()
            val backend = s.backend
            val bob = "b0".repeat(32)
            val founder = backend.listMembers(s.namespace)
            assertEquals(listOf(AppMember(backend.account, "Admin")), founder)
            assertTrue(founder[0].isAdmin)

            backend.groupMembers = backend.groupMembers + (s.namespace to founder + AppMember(bob, "Member", "Bob"))
            backend.setMemberName(s.namespace, "Ana")
            backend.setMemberRole(s.namespace, bob, GovernanceMemberRole.ReadOnly)
            assertEquals(
                listOf(AppMember(backend.account, "Admin", "Ana"), AppMember(bob, "ReadOnly", "Bob")),
                backend.listMembers(s.namespace),
            )

            backend.removeMember(s.namespace, bob)
            assertEquals(listOf(backend.account), backend.listMembers(s.namespace).map { it.identity })
            assertThrows<IllegalArgumentException> {
                runBlocking { backend.setMemberRole(s.namespace, bob, GovernanceMemberRole.Admin) }
            }
        }

    @Test
    fun `joining makes the account a member`() =
        runBlocking {
            val backend = InMemoryAppBackend()
            val namespace = "7d".repeat(32)
            backend.joinNamespace(namespace, InMemoryAppBackend.invitation(namespace))
            assertEquals(listOf(AppMember(backend.account, "Member")), backend.listMembers(namespace))
        }

    @Test
    fun `leaving and deleting`() =
        runBlocking {
            val s = seeded()
            val backend = s.backend
            backend.leaveSubgroup(s.group)
            assertFalse("leaving a subgroup leaves its contexts", backend.isMember(s.context))
            assertEquals("an open subgroup stays listed", listOf(s.group), backend.listSubgroups(s.namespace).map { it.groupId })

            backend.joinContext(s.context)
            backend.removeContext(s.group, s.context)
            assertEquals(emptyList<String>(), backend.listContexts(s.group))
            assertFalse(backend.isMember(s.context))

            backend.deleteSubgroup(s.group)
            assertEquals(emptyList<AppSubgroup>(), backend.listSubgroups(s.namespace))

            backend.leaveNamespace(s.namespace)
            assertEquals(emptyList<AppNamespace>(), backend.listNamespaces())
            assertEquals(emptyList<AppMember>(), backend.groupMembers[s.namespace])
            assertTrue(backend.log.contains("leaveNamespace ${s.namespace}"))
        }

    @Test
    fun `a backend of an app's own refuses the new calls by name`() =
        runBlocking {
            try {
                MinimalBackend().listMembers("g")
                fail("expected a refusal")
            } catch (e: UnsupportedOperationException) {
                assertTrue(e.message!!.startsWith("listMembers"))
            }
        }

    @Test
    fun `a bound contract is what a generated client calls`() {
        val calling: ContextContractCalling = RecordingContextContract().bound("ctx")
        assertEquals("ctx", (calling as BoundContextContract).contextId)
    }
}

/** A backend written before the names, blobs and members calls: the defaults keep it compiling. */
private class MinimalBackend : MeroAppBackend {
    override val account = ""
    override val contract: ContextContract = SignedOutContextContract
    override val hasRelay = false
    override val canSubscribe = false

    override suspend fun listNamespaces(): List<AppNamespace> = emptyList()

    override suspend fun listSubgroups(namespaceId: String): List<AppSubgroup> = emptyList()

    override suspend fun listContexts(groupId: String): List<String> = emptyList()

    override suspend fun isMember(contextId: String): Boolean = false

    override suspend fun joinContext(contextId: String) = Unit

    override suspend fun foundNamespace(name: String): AppFoundedNamespace = AppFoundedNamespace("", false)

    override suspend fun createSubgroup(
        namespaceId: String,
        name: String,
        isPublic: Boolean,
    ): String = ""

    override suspend fun createContext(
        groupId: String,
        applicationId: String,
        name: String,
        initArgs: JsonElement,
    ): String = ""

    override suspend fun createInvitation(namespaceId: String): OrgInvitation = InMemoryAppBackend.invitation(namespaceId)

    override suspend fun joinNamespace(
        namespaceId: String,
        invitation: OrgInvitation,
    ): AppJoinedNamespace = AppJoinedNamespace(namespaceId, false, "")

    override fun events(contextId: String): Flow<AppContextEvent> = emptyFlow()
}
