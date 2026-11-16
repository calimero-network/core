import Foundation
import MeroKit
import MeroKitTestSupport
import XCTest

final class ContextContractTests: XCTestCase {
    private let context = String(repeating: "11", count: 32)

    func testBoundContractCallsItsContextWithTheDecodedArgs() async throws {
        let contract = RecordingContextContract { call in
            call.method == "get_post" ? ["id": "p1", "likes": 3] : nil
        }
        let bound = contract.bound(to: context)

        let read = try await bound.query(method: "get_post", argsJSON: Data(#"{"id":"p1"}"#.utf8))
        let wrote = try await bound.execute(method: "like", argsJSON: Data(#"{"id":"p1","up":true}"#.utf8))

        XCTAssertEqual(String(decoding: read, as: UTF8.self), #"{"id":"p1","likes":3}"#)
        XCTAssertEqual(String(decoding: wrote, as: UTF8.self), "null", "a method returning nothing is JSON null")
        XCTAssertEqual(
            contract.calls,
            [
                RecordedContractCall(
                    path: .query, contextId: context, method: "get_post", args: ["id": "p1"],
                    argsJSON: Data(#"{"id":"p1"}"#.utf8)),
                RecordedContractCall(
                    path: .execute, contextId: context, method: "like", args: ["id": "p1", "up": true],
                    argsJSON: Data(#"{"id":"p1","up":true}"#.utf8)),
            ])
    }

    func testBoundContractReadsEmptyArgsAsAnEmptyObject() async throws {
        let contract = RecordingContextContract()
        _ = try await contract.bound(to: context).query(method: "list", argsJSON: Data())
        _ = try await contract.bound(to: context).query(method: "list", argsJSON: Data(" \n".utf8))
        XCTAssertEqual(contract.calls.map(\.args), [[:], [:]])
    }

    func testBoundContractRefusesArgsThatAreNotJSONBeforeCalling() async {
        let contract = RecordingContextContract()
        do {
            _ = try await contract.bound(to: context).execute(method: "post", argsJSON: Data("{nope".utf8))
            XCTFail("expected a refusal")
        } catch AccountError.invalidInput(let message) {
            XCTAssertTrue(message.contains("argsJSON"), message)
        } catch {
            XCTFail("unexpected \(error)")
        }
        XCTAssertTrue(contract.calls.isEmpty)
    }

    func testBoundContractPassesTheContractsFailureThrough() async {
        let contract = RecordingContextContract { _ in
            throw AppError("the app refused the call", details: ["code": "Forbidden"])
        }
        do {
            _ = try await contract.bound(to: context).execute(method: "post", argsJSON: Data("{}".utf8))
            XCTFail("expected the refusal")
        } catch let error as AppError {
            XCTAssertEqual(error.appCode, "Forbidden")
            XCTAssertEqual(error.status, 422)
        } catch {
            XCTFail("unexpected \(error)")
        }
    }

    func testSignedOutContractNamesTheMethod() async {
        for call in [RecordedContractCall.Path.query, .execute] {
            do {
                if call == .query {
                    _ = try await SignedOutContextContract().query(contextId: context, method: "get_feed", args: [:])
                } else {
                    _ = try await SignedOutContextContract().execute(contextId: context, method: "get_feed", args: [:])
                }
                XCTFail("expected SignedOutError")
            } catch let error as SignedOutError {
                let method = error.method
                XCTAssertEqual(method, "get_feed")
            } catch {
                XCTFail("unexpected \(error)")
            }
        }
    }
}
