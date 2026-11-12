// Round trips through the kitchen-sink golden client's app types (tests/golden/kitchen-sink.swift):
// sample JSON, as serde writes it for each enum tagging style and for records, decodes to the value
// it means and encodes back to the same JSON. scripts/check-app-abi-clients.py runs it with
// `swift test`.

import Foundation
import KitchenSink
import MeroKit
import XCTest

final class KitchenSinkRoundTripTests: XCTestCase {
    typealias Client = KitchenSinkClient

    /// `json` decodes to `expected`, and `expected` encodes to `json` (compared as JSON values).
    private func roundTrip<T: Codable & Equatable>(
        _ json: String, _ expected: T, file: StaticString = #filePath, line: UInt = #line
    ) throws {
        let decoded = try JSONDecoder().decode(T.self, from: Data(json.utf8))
        XCTAssertEqual(decoded, expected, file: file, line: line)
        let encoded = try JSONEncoder().encode(expected)
        let actual = try JSONDecoder().decode(JSONValue.self, from: encoded)
        let wanted = try JSONDecoder().decode(JSONValue.self, from: Data(json.utf8))
        XCTAssertEqual(actual, wanted, String(decoding: encoded, as: UTF8.self), file: file, line: line)
    }

    private func refused<T: Codable>(_ json: String, _ type: T.Type, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertThrowsError(try JSONDecoder().decode(T.self, from: Data(json.utf8)), json, file: file, line: line)
    }

    private let item = Client.Item(id: 18_446_744_073_709_551_615, title: "t", tags: ["a"], status: .open)
    private let itemJSON = #"{"id":"18446744073709551615","title":"t","tags":["a"],"status":"Open"}"#

    func testExternallyTagged() throws {
        try roundTrip(#""Open""#, Client.Status.open)
        try roundTrip(#"{"Closed":{"at":"7"}}"#, Client.Status.closed(at: 7))
        try roundTrip(#"{"Moved":"x"}"#, Client.Status.moved("x"))
        try roundTrip(#"{"Span":[1,2]}"#, Client.Status.span(1, 2))
        XCTAssertEqual(try JSONDecoder().decode(Client.Status.self, from: Data(#"{"Open":null}"#.utf8)), .open)
        refused(#""Shut""#, Client.Status.self)
        refused(#"{"Span":[1,2,3]}"#, Client.Status.self)
        refused(#"{"Moved":"x","Open":null}"#, Client.Status.self)
    }

    func testInternallyTagged() throws {
        try roundTrip(#"{"type":"draft"}"#, Client.Tagged.draft)
        try roundTrip(#"{"type":"published","at":"9"}"#, Client.Tagged.published(at: 9))
        try roundTrip(
            #"{"type":"wrapped","id":"18446744073709551615","title":"t","tags":["a"],"status":"Open"}"#,
            Client.Tagged.wrapped(item)
        )
        try roundTrip(#"{"kind":"one","value":\#(itemJSON)}"#, Client.Choice.one(value: item))
        try roundTrip(#"{"kind":"zero"}"#, Client.Choice<Client.Item>.zero)
        refused(#"{"type":"gone"}"#, Client.Tagged.self)
        refused(#"{"at":"9"}"#, Client.Tagged.self)
    }

    func testAdjacentlyTagged() throws {
        try roundTrip(#"{"t":"Text","c":"hi"}"#, Client.Adjacent.text("hi"))
        try roundTrip(#"{"t":"Empty"}"#, Client.Adjacent.empty)
        try roundTrip(#"{"t":"Pair","c":[1,2]}"#, Client.Adjacent.pair(1, 2))
        // An absent option is sent as `null`.
        try roundTrip(#"{"t":"Labeled","c":{"label":"l","at":null}}"#, Client.Adjacent.labeled(label: "l", at: nil))
        try roundTrip(#"{"t":"Labeled","c":{"label":"l","at":"3"}}"#, Client.Adjacent.labeled(label: "l", at: 3))
        refused(#"{"t":"Pair","c":"no"}"#, Client.Adjacent.self)
    }

    func testUntagged() throws {
        try roundTrip("5", Client.Either.num(5))
        try roundTrip(#"{"name":"n"}"#, Client.Either.named(name: "n"))
        try roundTrip(#"[1,"a"]"#, Client.Either.pair(1, "a"))
        try roundTrip("null", Client.Either.blank)
        refused(#""text""#, Client.Either.self)
    }

    func testStructsAndRecursion() throws {
        try roundTrip(itemJSON, item)
        try roundTrip(#""abc""#, Client.Id("abc"))
        try roundTrip("[1.5,2.5]", Client.Point(1.5, 2.5))
        try roundTrip("null", Client.Marker())
        try roundTrip(#"{"items":[\#(itemJSON)],"next":null}"#, Client.Page(items: [item], next: nil))
        try roundTrip(
            #"{"Add":[{"Lit":"-1"},{"Neg":{"Lit":"9223372036854775807"}}]}"#,
            Client.Expr.add(.lit(-1), .neg(.lit(Int64.max)))
        )
        let leaf = Client.Tree(value: 2, children: [])
        try roundTrip(#"{"value":2,"children":[]}"#, leaf)
        try roundTrip(
            #"{"value":1,"children":[{"value":2,"children":[]}],"parent":{"value":0,"children":[]}}"#,
            Client.Tree(value: 1, children: [leaf], parent: Client.Tree(value: 0, children: []))
        )
        refused(#"{"id":"1","title":"t","tags":[]}"#, Client.Item.self)
        refused("[1.5]", Client.Point.self)
    }
}
