import Foundation
import MeroKit
import MeroKitTestSupport
import XCTest

@testable import MeroKitUI

/// A consent page answered in-process: sends the person back naming
/// `requestId`, or is dismissed (`nil`).
private struct ReturningBrowser: WebAuthenticating {
    let requestId: String?
    let opened = Box<[URL]>([])

    func authenticate(url: URL, callback: CloudCallback) async throws -> URL {
        opened.update { $0.append(url) }
        guard let requestId else { throw WebAuthenticationCancelled() }
        return URL(string: "\(callback.url)?request_id=\(requestId)&result=approved")!
    }
}

/// `ConsentLauncher.decide`: the sealed result for the request this app
/// started, the checks on the page's return, and the browser fallback where
/// the system has no https callback.
@MainActor
final class ConsentLauncherTests: XCTestCase {
    private static let callback = CloudCallback.https(host: "chat.example", path: "/consent")
    nonisolated(unsafe) private var node: FakeCloudNode!
    nonisolated(unsafe) private var result: Box<JSONValue>!

    override func setUp() {
        super.setUp()
        let result = Box<JSONValue>([
            "requestId": "r1", "kind": "createOrg", "status": "approved", "decidedAt": 2, "outcome": ["orgId": "o9"],
        ])
        self.result = result
        node = FakeCloudNode { request in
            switch request.path {
            case "/consent/requests":
                return .json([
                    "requestId": "r1", "url": "https://node.test/consent/r1", "expiresAt": 1_900_000_000,
                    "code": .string(confirmationCode("r1")),
                ])
            case "/consent/r1/result": return .json(result.get)
            default: return .error(404, "NOT_FOUND")
            }
        }
        node.install()
    }

    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    private func client() -> CalimeroClient {
        CalimeroClient(
            CalimeroClientOptions(
                baseUrl: "https://node.test", policy: CloudPolicy(allowMock: true),
                session: MockURLProtocol.makeSession()))
    }

    private static func createOrg(_ returnTo: String = ConsentLauncherTests.callback.url) -> ConsentRequest {
        .createOrg(name: "Acme", returnTo: returnTo)
    }

    func testRunsThePageInTheAuthSessionAndReadsTheOutcome() async throws {
        let browser = ReturningBrowser(requestId: "r1")
        let (started, result) = try await ConsentLauncher.decide(
            client: client(), request: Self.createOrg(), callback: Self.callback, opener: browser,
            openInBrowser: { _ in
                XCTFail("the auth session takes an https callback here")
                return false
            },
            httpsCallbackSupported: true)
        XCTAssertEqual(started.requestId, "r1")
        XCTAssertEqual(result.orgId, "o9")
        XCTAssertEqual(browser.opened.get, [URL(string: "https://node.test/consent/r1")!])
        let body = try XCTUnwrap(node.requests.first { $0.path == "/consent/requests" }?.json)
        XCTAssertEqual(body["returnTo"], "https://chat.example/consent")
    }

    func testAReturnForAnotherRequestIsRefused() async throws {
        do {
            _ = try await ConsentLauncher.decide(
                client: client(), request: Self.createOrg(), callback: Self.callback,
                opener: ReturningBrowser(requestId: "r2"), httpsCallbackSupported: true)
            XCTFail("expected CONSENT_REQUEST_MISMATCH")
        } catch let error as CalimeroError {
            XCTAssertEqual(error.code, "CONSENT_REQUEST_MISMATCH")
        }
    }

    func testAReturnThatLeftThisRequestPendingIsRefused() async throws {
        result.update { $0 = ["requestId": "r1", "kind": "createOrg", "status": "pending", "decidedAt": .null] }
        do {
            _ = try await ConsentLauncher.decide(
                client: client(), request: Self.createOrg(), callback: Self.callback,
                opener: ReturningBrowser(requestId: "r1"), httpsCallbackSupported: true)
            XCTFail("expected CONSENT_NOT_DECIDED")
        } catch let error as CalimeroError {
            XCTAssertEqual(error.code, "CONSENT_NOT_DECIDED")
        }
    }

    func testAReturnAfterThisRequestWasReplacedIsRefused() async throws {
        let node = FakeCloudNode { request in
            guard request.path == "/consent/requests" else { return .error(404, "NOT_FOUND") }
            return .json([
                "requestId": "r1", "url": "https://node.test/consent/r1", "expiresAt": 1_900_000_000,
                "code": .string(confirmationCode("r1")),
            ])
        }
        node.install()
        do {
            _ = try await ConsentLauncher.decide(
                client: client(), request: Self.createOrg(), callback: Self.callback,
                opener: ReturningBrowser(requestId: "r1"), httpsCallbackSupported: true)
            XCTFail("expected CONSENT_REPLACED")
        } catch let error as CalimeroError {
            XCTAssertEqual(error.code, "CONSENT_REPLACED")
        }
    }

    func testADismissedSheetAnswersTheResultAsItStands() async throws {
        result.update { $0 = ["requestId": "r1", "kind": "createOrg", "status": "pending", "decidedAt": .null] }
        let (_, result) = try await ConsentLauncher.decide(
            client: client(), request: Self.createOrg(), callback: Self.callback,
            opener: ReturningBrowser(requestId: nil), httpsCallbackSupported: true)
        XCTAssertEqual(result.status, .pending)
    }

    func testFallsBackToTheBrowserAndPollsOnAnOlderSystem() async throws {
        let browser = ReturningBrowser(requestId: "r1")
        let opened = Box<[URL]>([])
        let (_, result) = try await ConsentLauncher.decide(
            client: client(), request: Self.createOrg(), callback: Self.callback, opener: browser,
            openInBrowser: { url in
                opened.update { $0.append(url) }
                return true
            },
            httpsCallbackSupported: false)
        XCTAssertEqual(result.orgId, "o9")
        XCTAssertEqual(opened.get, [URL(string: "https://node.test/consent/r1")!])
        XCTAssertTrue(browser.opened.get.isEmpty, "no auth session without an https callback")
    }

    func testTheServedPageCanBeTheReturnTo() async throws {
        let session = MeroClient(
            config: MeroProviderConfig(nodeURL: "https://node.test", policy: .development),
            store: MemoryCloudSessionStore(), session: MockURLProtocol.makeSession(), passkey: SoftPasskey())
        let served = "https://chat.apps.example/1.0.0/?native-consent=1"
        let result = await session.requestConsent(
            { Self.createOrg($0) }, callback: .scheme("merochat", path: "consent"), returnTo: served,
            opener: ReturningBrowser(requestId: "r1"))
        XCTAssertEqual(result?.orgId, "o9")
        let body = try XCTUnwrap(node.requests.first { $0.path == "/consent/requests" }?.json)
        XCTAssertEqual(body["returnTo"], .string(served))
    }
}
