import CryptoKit
import Foundation
import MeroKit
import MeroKitTestSupport
import XCTest

@testable import MeroKitUI

/// The served page, answered in-process: seals a session to the key in the URL
/// exactly as `@calimero-apps/passkey/native` does.
private struct SealingPage: WebAuthenticating {
    let session: ServedPageSession
    let opened = Box<[URL]>([])

    func authenticate(url: URL, callback: CloudCallback) async throws -> URL {
        opened.update { $0.append(url) }
        var parts = URLComponents()
        parts.percentEncodedQuery = String(url.fragment!.dropFirst("native-sign-in&".count))
        let state = parts.queryItems!.first { $0.name == "state" }!.value!
        let keyParam = parts.queryItems!.first { $0.name == "key" }!.value!
        let key = try P256.KeyAgreement.PublicKey(x963Representation: fromB64url(keyParam))
        let mine = P256.KeyAgreement.PrivateKey()
        let aes = try mine.sharedSecretFromKeyAgreement(with: key).hkdfDerivedSymmetricKey(
            using: SHA256.self, salt: Data(), sharedInfo: Data(ServedPageHandoff.info.utf8), outputByteCount: 32)
        let sealed = try AES.GCM.seal(JSONEncoder().encode(session), using: aes, authenticating: Data(state.utf8))
        let box = b64url(mine.publicKey.x963Representation + sealed.combined!)
        return URL(string: "\(callback.url)#state=\(state)&box=\(box)")!
    }
}

private struct DismissedSheet: WebAuthenticating {
    func authenticate(url: URL, callback: CloudCallback) async throws -> URL { throw WebAuthenticationCancelled() }
}

@MainActor
final class ServedPageSignInTests: XCTestCase {
    static let package = "com.calimero.chat"
    static let signedIn = ServedPageSession(
        v: 1, accountId: "acc_1", sessionToken: "v2.claims.sig", expiresAt: 1_900_000_000, refreshToken: "rt_1",
        nodeUrl: "https://n1.test", nodes: ["https://n2.test"])

    override func setUp() {
        super.setUp()
        MockURLProtocol.setHandler { request in
            guard request.url?.path == "/.well-known/calimero.json" else {
                return .init(status: 404, headers: [:], body: Data())
            }
            return .init(
                status: 200, headers: ["Content-Type": "application/json"],
                body: Data(
                    #"{"package":"com.calimero.chat","versions":["1.0.0","1.2.0"],"deployments":[],"api":[]}"#.utf8))
        }
    }

    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    private func client(_ store: MemoryCloudSessionStore) -> MeroClient {
        MeroClient(
            config: MeroProviderConfig(policy: .development, packageName: Self.package), store: store,
            session: MockURLProtocol.makeSession(), passkey: SoftPasskey())
    }

    func testSignsInThroughThePackagesPageAndStoresTheSession() async throws {
        let store = MemoryCloudSessionStore()
        let client = client(store)
        let page = SealingPage(session: Self.signedIn)

        let ok = await client.signInWithServedPage(callback: .scheme("mero-chat", path: "signed-in"), opener: page)

        XCTAssertTrue(ok, client.errorMessage ?? "")
        let opened = try XCTUnwrap(page.opened.get.first)
        XCTAssertEqual(opened.host, "\(AppHost.label(package: Self.package)).apps.calimero.network")
        XCTAssertEqual(opened.path, "/1.2.0")
        XCTAssertEqual(client.account, "acc_1")
        XCTAssertEqual(client.nodeURL, "https://n1.test")
        XCTAssertEqual(store.load()?.session.refreshToken, "rt_1")
        XCTAssertEqual(store.load()?.nodes, ["https://n2.test"])
        let cloudSession = await client.cloud?.session
        XCTAssertEqual(cloudSession?.sessionToken, "v2.claims.sig")
    }

    func testOpensTheNamedClustersHost() async throws {
        let page = SealingPage(session: Self.signedIn)
        _ = await client(MemoryCloudSessionStore()).signInWithServedPage(
            cluster: "eu-shared-1", callback: .scheme("mero-chat", path: "signed-in"), opener: page)
        XCTAssertEqual(
            page.opened.get.first?.host,
            "\(AppHost.clusterLabel(cluster: "eu-shared-1", package: Self.package)).apps.calimero.network")
    }

    func testADismissedSheetIsNotAnError() async {
        let client = client(MemoryCloudSessionStore())
        let ok = await client.signInWithServedPage(
            callback: .scheme("mero-chat", path: "signed-in"), opener: DismissedSheet())
        XCTAssertFalse(ok)
        XCTAssertNil(client.errorMessage)
        XCTAssertFalse(client.isAuthenticated)
    }
}
