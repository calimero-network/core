// Round trips through the kitchen-sink golden client's app types (tests/golden/kitchen-sink.kt):
// sample JSON, as serde writes it for each enum tagging style and for records, decodes to the value
// it means and encodes back to the same JSON. scripts/check-app-abi-clients.py runs it as a unit
// test of the golden-clients module.

package calimero.generated.kitchensink

import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class KitchenSinkRoundTripTest {
    /** [text] decodes to [expected], and [expected] encodes to [text] (compared as JSON elements). */
    private fun <T> roundTrip(
        text: String,
        serializer: KSerializer<T>,
        expected: T,
    ) {
        assertEquals(expected, Json.decodeFromString(serializer, text))
        assertEquals(Json.parseToJsonElement(text), Json.encodeToJsonElement(serializer, expected))
        assertEquals(expected, Json.decodeFromString(serializer, Json.encodeToString(serializer, expected)))
    }

    private fun <T> refused(
        text: String,
        serializer: KSerializer<T>,
    ) {
        assertThrows(text, SerializationException::class.java) { Json.decodeFromString(serializer, text) }
    }

    private val item = KitchenSinkClient.Item(id = ULong.MAX_VALUE, title = "t", tags = listOf("a"), status = KitchenSinkClient.Status.Open)
    private val itemJson = """{"id":"18446744073709551615","title":"t","tags":["a"],"status":"Open"}"""

    @Test
    fun externallyTagged() {
        val s = KitchenSinkClient.Status.serializer()
        roundTrip("\"Open\"", s, KitchenSinkClient.Status.Open)
        roundTrip("""{"Closed":{"at":"7"}}""", s, KitchenSinkClient.Status.Closed(7u))
        roundTrip("""{"Moved":"x"}""", s, KitchenSinkClient.Status.Moved("x"))
        roundTrip("""{"Span":[1,2]}""", s, KitchenSinkClient.Status.Span(1u, 2u))
        assertEquals(KitchenSinkClient.Status.Open, Json.decodeFromString(s, """{"Open":null}"""))
        refused("\"Shut\"", s)
        refused("""{"Span":[1,2,3]}""", s)
        refused("""{"Moved":"x","Open":null}""", s)
    }

    @Test
    fun internallyTagged() {
        val s = KitchenSinkClient.Tagged.serializer()
        roundTrip("""{"type":"draft"}""", s, KitchenSinkClient.Tagged.Draft)
        roundTrip("""{"type":"published","at":"9"}""", s, KitchenSinkClient.Tagged.Published(9u))
        roundTrip(
            """{"type":"wrapped","id":"18446744073709551615","title":"t","tags":["a"],"status":"Open"}""",
            s,
            KitchenSinkClient.Tagged.Wrapped(item),
        )
        val choice = KitchenSinkClient.Choice.serializer(KitchenSinkClient.Item.serializer())
        roundTrip("""{"kind":"one","value":$itemJson}""", choice, KitchenSinkClient.Choice.One(item))
        roundTrip("""{"kind":"zero"}""", choice, KitchenSinkClient.Choice.Zero)
        refused("""{"type":"gone"}""", s)
        refused("""{"at":"9"}""", s)
    }

    @Test
    fun adjacentlyTagged() {
        val s = KitchenSinkClient.Adjacent.serializer()
        roundTrip("""{"t":"Text","c":"hi"}""", s, KitchenSinkClient.Adjacent.Text("hi"))
        roundTrip("""{"t":"Empty"}""", s, KitchenSinkClient.Adjacent.Empty)
        roundTrip("""{"t":"Pair","c":[1,2]}""", s, KitchenSinkClient.Adjacent.Pair(1u, 2u))
        // An absent option is sent as `null`.
        roundTrip("""{"t":"Labeled","c":{"label":"l","at":null}}""", s, KitchenSinkClient.Adjacent.Labeled("l", null))
        roundTrip("""{"t":"Labeled","c":{"label":"l","at":"3"}}""", s, KitchenSinkClient.Adjacent.Labeled("l", 3u))
        refused("""{"t":"Pair","c":"no"}""", s)
    }

    @Test
    fun untagged() {
        val s = KitchenSinkClient.Either.serializer()
        roundTrip("5", s, KitchenSinkClient.Either.Num(5u))
        roundTrip("""{"name":"n"}""", s, KitchenSinkClient.Either.Named("n"))
        roundTrip("""[1,"a"]""", s, KitchenSinkClient.Either.Pair(1u, "a"))
        roundTrip("null", s, KitchenSinkClient.Either.Blank)
        refused("\"text\"", s)
    }

    @Test
    fun structsAndRecursion() {
        roundTrip(itemJson, KitchenSinkClient.Item.serializer(), item)
        roundTrip("\"abc\"", KitchenSinkClient.Id.serializer(), KitchenSinkClient.Id("abc"))
        roundTrip("[1.5,2.5]", KitchenSinkClient.Point.serializer(), KitchenSinkClient.Point(1.5, 2.5))
        roundTrip("null", KitchenSinkClient.Marker.serializer(), KitchenSinkClient.Marker)
        roundTrip(
            """{"items":[$itemJson],"next":null}""",
            KitchenSinkClient.Page.serializer(KitchenSinkClient.Item.serializer()),
            KitchenSinkClient.Page(listOf(item), null),
        )
        roundTrip(
            """{"Add":[{"Lit":"-1"},{"Neg":{"Lit":"9223372036854775807"}}]}""",
            KitchenSinkClient.Expr.serializer(),
            KitchenSinkClient.Expr.Add(KitchenSinkClient.Expr.Lit(-1), KitchenSinkClient.Expr.Neg(KitchenSinkClient.Expr.Lit(Long.MAX_VALUE))),
        )
        val leaf = KitchenSinkClient.Tree(2u, emptyList())
        roundTrip("""{"value":2,"children":[]}""", KitchenSinkClient.Tree.serializer(), leaf)
        roundTrip(
            """{"value":1,"children":[{"value":2,"children":[]}],"parent":{"value":0,"children":[]}}""",
            KitchenSinkClient.Tree.serializer(),
            KitchenSinkClient.Tree(1u, listOf(leaf), KitchenSinkClient.Tree(0u, emptyList())),
        )
        refused("""{"id":"1","title":"t","tags":[]}""", KitchenSinkClient.Item.serializer())
        refused("[1.5]", KitchenSinkClient.Point.serializer())
    }
}
