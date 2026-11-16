import Foundation
import MeroKit
import MeroKitTestSupport
import XCTest

@testable import MeroKitUI

/// A consent page answered in-process: approves and sends the person back.
private struct ApprovingBrowser: WebAuthenticating {
    let opened = Box<[URL]>([])

    func authenticate(url: URL, callback: CloudCallback) async throws -> URL {
        opened.update { $0.append(url) }
        return URL(string: "\(callback.url)?request_id=r1&result=approved")!
    }
}

@MainActor
final class MeroAppSessionTests: XCTestCase {
    private static let package = "com.calimero.chat"
    nonisolated(unsafe) private var cluster: FakeCloud!
    nonisolated(unsafe) private var node: FakeCloudNode!

    override func setUp() {
        super.setUp()
        cluster = FakeCloud()
        let cluster = self.cluster!
        node = FakeCloudNode { request in
            switch request.path {
            case "/consent/requests":
                return .json([
                    "requestId": "r1", "url": "https://node.test/consent/r1", "expiresAt": 1,
                    "code": .string(confirmationCode("r1")),
                ])
            case "/consent/r1/result":
                return .json(["requestId": "r1", "kind": "upgrade", "status": "approved", "decidedAt": 2])
            default: return cluster.handle(request)
            }
        }
        node.install()
    }

    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    private func makeSession(store: MemoryCloudSessionStore = MemoryCloudSessionStore()) -> MeroAppSession {
        let client = MeroClient(
            config: MeroProviderConfig(
                nodeURL: "https://node.test", policy: .development, packageName: Self.package),
            store: store, session: MockURLProtocol.makeSession(), passkey: SoftPasskey())
        return MeroAppSession(client: client, callback: .scheme("merochat", path: "consent"))
    }

    func testSignsInWithAPasskeyAndBuildsTheCloudBackend() async throws {
        let store = MemoryCloudSessionStore()
        let session = makeSession(store: store)
        XCTAssertFalse(session.isSignedIn)
        XCTAssertNil(session.backend)
        XCTAssertTrue(session.contract is SignedOutContextContract)

        await session.signIn()
        XCTAssertNil(session.client.errorMessage)
        XCTAssertTrue(session.isSignedIn)
        XCTAssertEqual(session.account, FakeCloud.me)
        XCTAssertEqual(session.applicationId, Self.package)
        XCTAssertTrue(session.backend is CalimeroAppBackend)
        XCTAssertTrue(session.hasRelay)
        XCTAssertTrue(session.contract is CloudContextContract)
        // The observer persists the session asynchronously.
        for _ in 0..<50 where store.load() == nil { try await Task.sleep(nanoseconds: 10_000_000) }
        XCTAssertEqual(store.load()?.session.refreshToken, "rt_s")
        XCTAssertEqual(store.load()?.nodeURL, "https://node.test")

        await session.signOut()
        XCTAssertFalse(session.isSignedIn)
        XCTAssertNil(session.backend)
        XCTAssertNil(store.load())
    }

    func testRestoresAStoredSessionByRefreshingIt() async throws {
        let stored = StoredCloudSession(
            nodeURL: "https://node.test", allowMock: true,
            session: CalimeroSession(
                accountId: FakeCloud.me, sessionToken: "v2.old", expiresAt: 0, refreshToken: "rt_old"))
        let session = makeSession(store: MemoryCloudSessionStore(stored))
        await session.restore()
        XCTAssertTrue(session.isSignedIn)
        XCTAssertNotNil(session.backend)
        XCTAssertEqual(node.requests.last?.path, "/auth/refresh")
        XCTAssertEqual(node.requests.last?.json?["refreshToken"], "rt_old")
    }

    func testHoldsAnInvitationLinkAndRedeemsItAfterSignIn() async throws {
        let session = makeSession()
        await session.signIn()
        let backend = try XCTUnwrap(session.backend)
        let org = try await backend.foundNamespace(name: "Team").namespaceId
        let link = try await session.createInvitationLink(namespaceId: org, contextId: "app9")
        XCTAssertTrue(link.hasPrefix("https://links.calimero.network/\(Self.package)/join?invitation="))
        let payload = try XCTUnwrap(OrgInvitation.parse(link, expectedSlug: Self.package))
        XCTAssertEqual(payload, OrgInvitation(namespaceId: org, token: "inv_\(org)", contextId: "app9", name: "Team"))
        XCTAssertNil(OrgInvitation.parse(link, expectedSlug: "com.other.app"))
        let json = try XCTUnwrap(InviteCodec.decode(token: try payload.encoded()))
        XCTAssertTrue(json.contains(#""invitation":"inv_"#), json)
        XCTAssertNil(
            OrgInvitation.decode(
                try InviteCodec.encode(["invitation": ["groupId": "x"], "namespaceId": "o"] as JSONValue)))

        let handled = await session.handle(URL(string: link)!)
        XCTAssertTrue(handled)
        XCTAssertEqual(session.pendingInvitation, link)
        let foreign = await session.handle(URL(string: "https://links.calimero.network/com.other/join?invitation=x")!)
        XCTAssertFalse(foreign)

        XCTAssertEqual(session.pendingOrgInvitation?.namespaceId, org)
        let joined = try await session.redeemPendingInvitation()
        XCTAssertEqual(joined, org)
        XCTAssertNil(session.pendingInvitation)
        XCTAssertEqual(node.requests.last?.path, "/invites/inv_\(org)/redeem")
    }

    func testOpensTheConsentPageInTheBrowserAndReadsTheDecision() async throws {
        let session = makeSession()
        await session.signIn()
        let browser = ApprovingBrowser()
        let result = await session.client.requestConsent(
            { upgradeConsent(app: "a1", toVersion: "1.1.0", returnTo: $0) }, callback: session.callback, opener: browser
        )
        XCTAssertEqual(result?.status, .approved)
        XCTAssertEqual(browser.opened.get, [URL(string: "https://node.test/consent/r1")!])
        let request = try XCTUnwrap(node.requests.first { $0.path == "/consent/requests" }?.json)
        XCTAssertEqual(request["returnTo"], "merochat://consent")
        let returned = await session.handle(URL(string: "merochat://consent?request_id=r1&result=approved")!)
        XCTAssertTrue(returned)
    }

    func testRecognisesThisAppsInvitationLinksOnly() {
        let link = "https://links.calimero.network/com.calimero.chat/join?invitation=abc"
        XCTAssertTrue(MeroAppSession.isInvitationLink(link, packageName: "com.calimero.chat"))
        XCTAssertFalse(MeroAppSession.isInvitationLink(link, packageName: "com.calimero.chat2"))
        XCTAssertTrue(MeroAppSession.isInvitationLink(link, packageName: nil))
        XCTAssertFalse(MeroAppSession.isInvitationLink("https://links.calimero.network/x/join", packageName: nil))
    }
}

/// A thread-safe box.
final class Box<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: T

    init(_ value: T) {
        self.value = value
    }

    var get: T {
        lock.lock()
        defer { lock.unlock() }
        return value
    }

    func update(_ body: (inout T) -> Void) {
        lock.lock()
        body(&value)
        lock.unlock()
    }
}
