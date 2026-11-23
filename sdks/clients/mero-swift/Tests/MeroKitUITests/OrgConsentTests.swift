import Foundation
import MeroKit
import MeroKitTestSupport
import XCTest

@testable import MeroKitUI

/// The consent page, answered in-process: sends the person back naming `requestId`.
private struct ReturningPage: WebAuthenticating {
    let requestId: String
    let opened = Box<[URL]>([])

    func authenticate(url: URL, callback: CloudCallback) async throws -> URL {
        opened.update { $0.append(url) }
        return URL(string: "\(callback.url)?request_id=\(requestId)&result=approved")!
    }
}

/// `ServedPageConsent`: the consent page returns through the package's
/// served page (its latest version), and the decision read is this request's.
@MainActor
final class OrgConsentTests: XCTestCase {
    private static let origin = URL(string: "https://chat.apps.test")!
    private static let callback = CloudCallback.scheme("mero-chat", path: "consent")
    nonisolated(unsafe) private var node: FakeCloudNode!

    override func setUp() {
        super.setUp()
        node = FakeCloudNode { request in
            switch request.path {
            case "/consent/requests":
                return .json([
                    "requestId": "r1", "url": "https://node.test/consent/r1", "expiresAt": 1_900_000_000,
                    "code": .string(confirmationCode("r1")),
                ])
            case "/consent/r1/result":
                return .json([
                    "requestId": "r1", "kind": "createInvite", "status": "approved", "decidedAt": 2,
                    "outcome": ["token": "inv_1"],
                ])
            default: return .error(404, "NOT_FOUND")
            }
        }
        // The package's served page, on its own host.
        FakeCloudNode.install([node]) { request in
            guard request.url?.host == Self.origin.host, request.url?.path == "/.well-known/calimero.json" else {
                return .init(status: 404, headers: [:], body: Data())
            }
            return .init(
                status: 200, headers: ["Content-Type": "application/json"],
                body: Data(#"{"package":"com.calimero.chat","versions":["1.0.0","1.2.0"],"deployments":[]}"#.utf8))
        }
    }

    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    private func consent(_ page: ReturningPage) -> ServedPageConsent {
        let client = CalimeroClient(
            CalimeroClientOptions(
                baseUrl: "https://node.test", policy: CloudPolicy(allowMock: true),
                session: MockURLProtocol.makeSession()))
        return ServedPageConsent(
            client: client, origin: Self.origin, callback: Self.callback, opener: page,
            session: MockURLProtocol.makeSession())
    }

    func testReturnsThroughTheLatestServedVersionAndReadsTheDecision() async throws {
        let page = ReturningPage(requestId: "r1")
        let result = try await consent(page).decide {
            .createInvite(org: "org_1", role: .member, teams: ["t_chat"], returnTo: $0)
        }
        XCTAssertEqual(result.inviteToken, "inv_1")
        XCTAssertEqual(page.opened.get, [URL(string: "https://node.test/consent/r1")!])
        let body = try XCTUnwrap(node.requests.first { $0.path == "/consent/requests" }?.json)
        XCTAssertEqual(body["returnTo"], "https://chat.apps.test/1.2.0/?native-consent=1")
        XCTAssertEqual(body["kind"], "createInvite")
        XCTAssertEqual(body["subject"]?["teams"], ["t_chat"])
    }

    func testAReturnForAnotherRequestIsRefused() async throws {
        do {
            _ = try await consent(ReturningPage(requestId: "r2")).decide { .createOrg(name: "Acme", returnTo: $0) }
            XCTFail("a return naming another request is not this decision")
        } catch {
            XCTAssertTrue("\(error)".contains("CONSENT_REQUEST_MISMATCH"), "\(error)")
        }
    }

    func testOnlyAPackageSessionOnCalimeroCloudUsesTheConsentPage() {
        let dev = MeroClient(
            config: MeroProviderConfig(policy: .development, packageName: "com.calimero.chat"),
            store: MemoryCloudSessionStore(), session: MockURLProtocol.makeSession(), passkey: SoftPasskey())
        XCTAssertNil(dev.servedPageConsent(callback: Self.callback), "not signed in")
        dev.connect(nodeURL: "http://127.0.0.1:2501", allowMock: true)
        XCTAssertNil(dev.servedPageConsent(callback: Self.callback), "a development node is called directly")

        let cloud = MeroClient(
            config: MeroProviderConfig(policy: .development, packageName: "com.calimero.chat"),
            store: MemoryCloudSessionStore(), session: MockURLProtocol.makeSession(), passkey: SoftPasskey())
        cloud.connect(nodeURL: "https://node.test", allowMock: false)
        let approver = cloud.servedPageConsent(cluster: "eu-shared-1", callback: Self.callback)
        let served = try? XCTUnwrap(approver as? ServedPageConsent)
        XCTAssertEqual(
            served?.origin.host,
            "\(AppHost.clusterLabel(cluster: "eu-shared-1", package: "com.calimero.chat")).apps.calimero.network")

        let noPackage = MeroClient(
            config: MeroProviderConfig(policy: .development), store: MemoryCloudSessionStore(),
            session: MockURLProtocol.makeSession(), passkey: SoftPasskey())
        noPackage.connect(nodeURL: "https://node.test", allowMock: false)
        XCTAssertNil(noPackage.servedPageConsent(callback: Self.callback), "no package, no served page")
    }
}
