import CryptoKit
import Foundation
import MeroKitTestSupport
import XCTest

@testable import MeroKit

/// A public link's visitor: the vector core's client (`calimero_client::visitor`),
/// mero-js (`client/visitor.test.ts`), mero-py and mero-kotlin check too.
final class VisitorTests: XCTestCase {
    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    private static let link = "d4205f1ac7bb3fe43278131b6e621ebf952f0a5f51a38505327572d863c46c42"
    private static let key = "fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618"
    /// Core's (deterministic) signature in the shared bearer
    /// `vis1.pl_token.<key>.1000.<signature>`.
    private static let coresSignature =
        "9b5205e4deb4b534a612c3b03be773b60aea16192f2fb6d99496d194f2dd2591"
        + "73173731e56a46ec7b37cc3d2d2482fe7be5d70c0fadda54c22f5ea2b8fa4e09"

    func testNamesTheLinkAndTheVisitorAndSignsTheCredentialCoreExpects() throws {
        XCTAssertEqual(try publicLinkKey("pl_token"), Self.link)
        let visitor = try Visitor(token: "pl_token", seed: Data(repeating: 9, count: 32))
        XCTAssertEqual(visitor.link, Self.link)
        XCTAssertEqual(visitor.publicKey, Self.key)
        XCTAssertEqual(
            String(decoding: visitor.message(at: 1000), as: UTF8.self),
            "calimero.visitor.v1\0\(Self.link)\0\(Self.key)\01000")

        // CryptoKit's Ed25519 signatures are randomized, so check the bearer's
        // shape and that its signature verifies, rather than pinning core's bytes.
        let bearer = try visitor.bearer(at: 1000)
        let prefix = "vis1.pl_token.\(Self.key).1000."
        XCTAssertTrue(bearer.hasPrefix(prefix), bearer)
        let signature = try Hex.decode(String(bearer.dropFirst(prefix.count)), label: "signature", bytes: 64)
        let publicKey = try Curve25519.Signing.PublicKey(
            rawRepresentation: try Hex.decode(Self.key, label: "key", bytes: 32))
        XCTAssertTrue(publicKey.isValidSignature(signature, for: visitor.message(at: 1000)))
        // Core's own signature over the same bytes verifies too.
        let cores = try Hex.decode(Self.coresSignature, label: "signature", bytes: 64)
        XCTAssertTrue(publicKey.isValidSignature(cores, for: visitor.message(at: 1000)))
    }

    func testGivesTwoSeedsTwoKeysAndOneLink() throws {
        let one = try Visitor(token: "pl_token", seed: Data(repeating: 1, count: 32))
        let two = try Visitor(token: "pl_token", seed: Data(repeating: 2, count: 32))
        XCTAssertNotEqual(one.publicKey, two.publicKey)
        XCTAssertEqual(one.link, two.link)
        XCTAssertEqual(newVisitorSeed().count, 32)
        XCTAssertThrowsError(try Visitor(token: "pl_token", seed: Data(repeating: 1, count: 31)))
    }

    func testTheClientSignsEveryRequestAsTheVisitorAndNeverRefreshes() async throws {
        let node = FakeCloudNode { _ in .error(401, "UNAUTHENTICATED") }
        node.install()
        let cloud = CalimeroClient(
            CalimeroClientOptions(
                baseUrl: "https://node.test", policy: CloudPolicy(allowMock: true),
                session: MockURLProtocol.makeSession(), retry: CloudRetryOptions(retries: 0)),
            sleep: { _ in })
        await cloud.useSession(
            CalimeroSession(accountId: "a", sessionToken: "v2.x", expiresAt: 0, refreshToken: "rt_x"))
        let visitor = try await cloud.useVisitor(link: "pl_token", seed: Data(repeating: 9, count: 32))
        let session = await cloud.session
        XCTAssertNil(session)
        do {
            _ = try await cloud.view("a1", "get")
            XCTFail("expected LoginRequired")
        } catch is LoginRequired {}
        XCTAssertEqual(node.requests.map(\.path), ["/apps/a1/views"])
        let bearer = try XCTUnwrap(node.requests.first?.bearer)
        XCTAssertTrue(bearer.hasPrefix("vis1.pl_token.\(visitor.publicKey)."), bearer)

        await cloud.useApiKey("cal_key")
        _ = try? await cloud.view("a1", "get")
        XCTAssertEqual(node.requests.last?.bearer, "cal_key")
    }

    func testSigningOutDropsTheVisitorEvenWhenTheNodeRefuses() async throws {
        // A visitor has no session, so the node refuses the logout; the client must still forget it.
        let node = FakeCloudNode { _ in .error(403, "VISITOR_ONLY") }
        node.install()
        let cloud = CalimeroClient(
            CalimeroClientOptions(
                baseUrl: "https://node.test", policy: CloudPolicy(allowMock: true),
                session: MockURLProtocol.makeSession(), retry: CloudRetryOptions(retries: 0)),
            sleep: { _ in })
        _ = try await cloud.useVisitor(link: "pl_token", seed: Data(repeating: 9, count: 32))
        do {
            try await cloud.signOut()
            XCTFail("expected the node's refusal")
        } catch {}
        let before = node.requests.count
        _ = try? await cloud.view("a1", "get")
        XCTAssertTrue(node.requests.dropFirst(before).allSatisfy { $0.bearer == nil })
    }
}
