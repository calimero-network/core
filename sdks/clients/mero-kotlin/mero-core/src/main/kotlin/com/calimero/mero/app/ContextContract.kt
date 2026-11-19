package com.calimero.mero.app

import com.calimero.mero.CalimeroClient
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject

// An app instance's contract, as an app on a phone reaches it: a read is a view
// (`POST /apps/{app}/views`), a write a call (`POST /apps/{app}/intents`), both sealed to the
// attested node. Every app makes the same two calls; only the method names differ, and those
// belong to the app (or to the client mero-devtools-js generates for it, over
// [ContextContractCalling]).

/**
 * The two ways an app reaches the contract in an app instance. (Swift `MeroKit.ContextContract`.)
 *
 * [CloudContextContract] is the real one; [SignedOutContextContract] stands in while no one is
 * signed in. [bound] fixes the app instance, for a generated client.
 */
interface ContextContract {
    /** Run a view (`#[app::view]`): the method's return value. */
    suspend fun query(
        contextId: String,
        method: String,
        args: JsonElement,
    ): JsonElement?

    /** Run a mutation: the method's return value. */
    suspend fun execute(
        contextId: String,
        method: String,
        args: JsonElement,
    ): JsonElement?

    /** This contract with [contextId] fixed: what a generated client calls. */
    fun bound(contextId: String): BoundContextContract = BoundContextContract(this, contextId)
}

/**
 * [ContextContract] over a signed-in [CalimeroClient]. (Swift `CloudContextContract`.)
 *
 * - `query` is [CalimeroClient.view] (strong consistency): its `result`.
 * - `execute` is [CalimeroClient.call] (an idempotency key per call, retried on `503`): its
 *   `result`. An app's refusal throws [com.calimero.mero.AppError].
 */
class CloudContextContract(
    val client: CalimeroClient,
) : ContextContract {
    override suspend fun query(
        contextId: String,
        method: String,
        args: JsonElement,
    ): JsonElement? = client.view(contextId, method, args).result.takeUnless { it is JsonNull }

    override suspend fun execute(
        contextId: String,
        method: String,
        args: JsonElement,
    ): JsonElement? = client.call(contextId, method, args).result.takeUnless { it is JsonNull }
}

/** No one is signed in: there is no account to call the contract as. */
class SignedOutException(
    /** The method that was called. */
    val method: String,
) : IllegalStateException("$method needs a signed-in account")

/**
 * A [CalimeroAppBackend] method that needs a passkey step-up (`412 CONFIRM_REQUIRED`: members,
 * invites, leaving) was called while the backend had no passkey to approve it with (mero-compose:
 * `MeroAppSession.passkey` unset). Nothing was sent. (Swift `PasskeyRequired`.)
 */
class PasskeyRequired(
    /** The method that was called. */
    val method: String,
) : IllegalStateException("$method needs a passkey approval; give the backend a passkey")

/** The contract while no one is signed in: every call throws [SignedOutException], naming the method. */
object SignedOutContextContract : ContextContract {
    override suspend fun query(
        contextId: String,
        method: String,
        args: JsonElement,
    ): JsonElement? = throw SignedOutException(method)

    override suspend fun execute(
        contextId: String,
        method: String,
        args: JsonElement,
    ): JsonElement? = throw SignedOutException(method)
}

/**
 * One context's contract, in the shape a generated client calls: JSON in, JSON out, as strings.
 * (Swift `ContextContractCalling`, which takes and returns UTF-8 `Data`.)
 *
 * [BoundContextContract] is the SDK's; [ContextContract.bound] makes one.
 */
interface ContextContractCalling {
    /** Run a view: [argsJson] is the argument object; the result is the method's return value as JSON (`null` when it returns nothing). */
    suspend fun query(
        method: String,
        argsJson: String,
    ): String

    /** Run a mutation: as [query], as a call. */
    suspend fun execute(
        method: String,
        argsJson: String,
    ): String
}

/**
 * A [ContextContract] with its context fixed. (Swift `BoundContextContract`.)
 *
 * Blank [argsJson][query] is read as `{}`, and anything that is not JSON is refused
 * ([IllegalArgumentException]) before a request is made.
 */
class BoundContextContract(
    val contract: ContextContract,
    val contextId: String,
) : ContextContractCalling {
    override suspend fun query(
        method: String,
        argsJson: String,
    ): String = encode(contract.query(contextId, method, args(argsJson)))

    override suspend fun execute(
        method: String,
        argsJson: String,
    ): String = encode(contract.execute(contextId, method, args(argsJson)))

    private fun encode(result: JsonElement?): String = (result ?: JsonNull).toString()

    private fun args(json: String): JsonElement {
        if (json.isBlank()) return JsonObject(emptyMap())
        return try {
            Json.parseToJsonElement(json)
        } catch (e: SerializationException) {
            throw IllegalArgumentException("argsJson is not JSON: ${e.message}", e)
        }
    }
}
