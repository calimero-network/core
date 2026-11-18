package com.calimero.mero.sample.chat

import com.calimero.mero.testkit.InMemoryAppBackend
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** The chat on the app-facing layer, against mero-testkit's in-memory backend (no cluster). */
class ChatServiceTest {
    @Test
    fun `creating a space makes an org, an open general team and a channel on it`() =
        runBlocking {
            val backend = InMemoryAppBackend()
            val chat = ChatService(backend, "ada")
            chat.loadSpaces()
            assertEquals(SpacesLoad.LOADED, chat.spacesLoad)
            assertTrue(chat.spaces.isEmpty())

            chat.createSpace("Acme")
            val space = chat.spaces.single()
            assertEquals("Acme", space.name)
            chat.loadChannels(space)
            assertEquals(listOf("general"), chat.channels.map { it.name })
            assertTrue(backend.log.any { it.startsWith("createSubgroup ${space.id} general open") })
            assertTrue(backend.recording.calls("set_profile").isNotEmpty())
        }

    @Test
    fun `a failed listing is FAILED, never an empty one`() =
        runBlocking {
            val backend = InMemoryAppBackend()
            backend.failure = IllegalStateException("offline")
            val chat = ChatService(backend)
            chat.loadSpaces()
            assertEquals(SpacesLoad.FAILED, chat.spacesLoad)
            assertTrue(chat.statusIsError)
        }

    @Test
    fun `an invite link round-trips into a join`() =
        runBlocking {
            val owner = InMemoryAppBackend()
            val chat = ChatService(owner, "ada")
            chat.createSpace("Acme")
            val link = chat.makeInvite(chat.spaces.single())!!
            assertTrue(link, link.contains(ChatService.PACKAGE_NAME))

            val guest = InMemoryAppBackend(account = "cd".repeat(32))
            val guestChat = ChatService(guest, "bob")
            assertTrue(guestChat.joinSpace(link))
            assertEquals(listOf(chat.spaces.single().id), guestChat.spaces.map { it.id })
        }
}
