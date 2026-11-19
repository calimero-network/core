package com.calimero.mero.app

import com.calimero.mero.testkit.RecordedContractCall
import com.calimero.mero.testkit.RecordingContextContract
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** [ContextContract.bound], the shape a generated client calls, and the signed-out contract. (Swift `ContextContractTests`.) */
class ContextContractTest {
    private val context = "11".repeat(32)

    @Test
    fun `a bound contract calls its context with the decoded args`() =
        runBlocking {
            val contract =
                RecordingContextContract { call ->
                    if (call.method == "get_post") {
                        buildJsonObject {
                            put("id", "p1")
                            put("likes", 3)
                        }
                    } else {
                        null
                    }
                }
            val bound = contract.bound(context)

            val read = bound.query("get_post", """{"id":"p1"}""")
            val wrote = bound.execute("like", """{"id":"p1","up":true}""")

            assertEquals("""{"id":"p1","likes":3}""", read)
            assertEquals("a method returning nothing is JSON null", "null", wrote)
            assertEquals(
                listOf(
                    RecordedContractCall(RecordedContractCall.Path.QUERY, context, "get_post", buildJsonObject { put("id", "p1") }),
                    RecordedContractCall(
                        RecordedContractCall.Path.EXECUTE,
                        context,
                        "like",
                        buildJsonObject {
                            put("id", "p1")
                            put("up", true)
                        },
                    ),
                ),
                contract.calls,
            )
        }

    @Test
    fun `u64s round-trip exactly through the bound adapter`() =
        runBlocking {
            val sent = """{ "max": 18446744073709551615, "at": 1761650000123456789 }"""
            val exact = """{"max":18446744073709551615,"at":1761650000123456789}"""
            val contract = RecordingContextContract { call -> call.args }

            assertEquals(exact, contract.bound(context).query("echo", sent))
            assertEquals(exact, contract.bound(context).execute("echo", sent))
            val args =
                contract.calls
                    .first()
                    .args.jsonObject
            assertEquals("18446744073709551615", args["max"]!!.jsonPrimitive.content)
            assertEquals("1761650000123456789", args["at"]!!.jsonPrimitive.content)
        }

    @Test
    fun `a bound contract reads blank args as an empty object`() =
        runBlocking {
            val contract = RecordingContextContract()
            contract.bound(context).query("list", "")
            contract.bound(context).query("list", " \n")
            assertEquals(listOf(JsonObject(emptyMap()), JsonObject(emptyMap())), contract.calls.map { it.args })
        }

    @Test
    fun `a bound contract refuses args that are not JSON before calling`() =
        runBlocking {
            val contract = RecordingContextContract()
            try {
                contract.bound(context).execute("post", "{nope")
                fail("expected a refusal")
            } catch (e: IllegalArgumentException) {
                assertTrue(e.message, e.message!!.contains("argsJson"))
            }
            assertTrue(contract.calls.isEmpty())
        }

    @Test
    fun `a bound contract passes the contract's failure through`() =
        runBlocking {
            val contract = RecordingContextContract { throw IllegalStateException("refused") }
            try {
                contract.bound(context).execute("post", "{}")
                fail("expected the refusal")
            } catch (e: IllegalStateException) {
                assertEquals("refused", e.message)
            }
        }

    @Test
    fun `the signed-out contract names the method`() =
        runBlocking {
            for (query in listOf(true, false)) {
                try {
                    if (query) {
                        SignedOutContextContract.query(context, "get_feed", JsonObject(emptyMap()))
                    } else {
                        SignedOutContextContract.execute(context, "get_feed", JsonPrimitive(1))
                    }
                    fail("expected SignedOutException")
                } catch (e: SignedOutException) {
                    assertEquals("get_feed", e.method)
                }
            }
        }
}
