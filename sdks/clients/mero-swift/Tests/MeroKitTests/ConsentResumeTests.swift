import Foundation
import MeroKitTestSupport
import XCTest

@testable import MeroKit

/// ``CalimeroClient/openConsentPage(_:options:)`` keeping the started
/// request's id, and ``CalimeroClient/resumeConsent(_:requestId:)`` on the URL
/// the consent page sends the person back to, against a ``FakeCloudNode``.
/// Port of mero-js `client/consent-resume.test.ts` (`resumeConsent`).
final class ConsentResumeTests: XCTestCase {
    private static let id = "1.AAAAAAAAAAAAAAAAAAAAAA"
    private static let page = "https://node-1.example/consent/\(id)"
    private static let returnTo = "https://kv.apps.example/orgs"

    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    private func client(_ store: any ConsentRequestStore) -> CalimeroClient {
        CalimeroClient(
            CalimeroClientOptions(
                baseUrl: "https://node.test", policy: CloudPolicy(allowMock: true),
                session: MockURLProtocol.makeSession(), retry: CloudRetryOptions(retries: 0), consentStore: store),
            sleep: { _ in })
    }

    private static func back(_ result: String = "approved", _ id: String = id) -> URL {
        URL(string: "\(returnTo)?tab=1&request_id=\(id)&result=\(result)")!
    }

    private static func result(
        _ status: String, _ kind: String = "createOrg", outcome: JSONValue? = nil, requestId: String = id
    ) -> JSONValue {
        var out: [String: JSONValue] = [
            "requestId": .string(requestId), "kind": .string(kind), "status": .string(status),
            "decidedAt": status == "pending" ? .null : 7,
        ]
        if let outcome { out["outcome"] = outcome }
        return .object(out)
    }

    /// A node that starts request `id` and answers `answer` for its result.
    @discardableResult
    private func serve(_ answer: @escaping @Sendable () -> FakeCloudAnswer) -> FakeCloudNode {
        let node = FakeCloudNode { request in
            switch (request.method, request.path) {
            case ("POST", "/consent/requests"):
                return .json([
                    "requestId": .string(Self.id), "url": .string(Self.page), "expiresAt": 1_900_000_000,
                    "code": .string(confirmationCode(Self.id)),
                ])
            case ("GET", "/consent/\(Self.id)/result"): return answer()
            default: return .error(404, "NOT_FOUND")
            }
        }
        node.install()
        return node
    }

    private func refusal(_ body: () async throws -> Any) async -> CalimeroError? {
        do {
            _ = try await body()
            XCTFail("expected a refusal")
            return nil
        } catch let error as CalimeroError {
            return error
        } catch {
            XCTFail("not a CalimeroError: \(error)")
            return nil
        }
    }

    func testOpenConsentPageStoresTheStartedRequestIdAndNothingOfTheRequest() async throws {
        serve { .json(Self.result("pending")) }
        let store = MemoryConsentRequestStore()
        let opened = Sink()
        _ = try await client(store).openConsentPage(
            .acceptInvite(token: "inv-secret", returnTo: Self.returnTo),
            navigate: { opened.append(Data($0.absoluteString.utf8)) })
        XCTAssertEqual(store.load(), Self.id)
        XCTAssertEqual(opened.chunks.map { String(decoding: $0, as: UTF8.self) }, [Self.page])

        // Declined in `confirm`: nothing stored, nothing opened.
        store.clear()
        _ = try await client(store).openConsentPage(
            .acceptInvite(token: "inv-secret", returnTo: Self.returnTo),
            options: OpenConsentOptions(navigate: { _ in XCTFail("opened") }, confirm: { _ in false }))
        XCTAssertNil(store.load())
    }

    func testAnswersThisAppsRequestAsTheNodeDecidedItAndForgetsIt() async throws {
        let node = serve { .json(Self.result("approved", outcome: ["orgId": "org-9"])) }
        let store = MemoryConsentRequestStore(Self.id)
        let r = try await client(store).resumeConsent(Self.back())
        XCTAssertEqual(r?.status, .approved)
        XCTAssertEqual(r?.orgId, "org-9")
        XCTAssertEqual(node.requests.map { "\($0.method) \($0.path)" }, ["GET /consent/\(Self.id)/result"])
        XCTAssertNil(store.load())
    }

    func testTrustsTheNodesStatusOverTheURLs() async throws {
        serve { .json(Self.result("declined", "acceptInvite")) }
        let r = try await client(MemoryConsentRequestStore(Self.id)).resumeConsent(Self.back("approved"))
        XCTAssertEqual(r?.status, .declined)
        XCTAssertEqual(r?.kind, "acceptInvite")
    }

    func testAnswersNilWithoutARequestIdAndAsksNothing() async throws {
        let node = serve { .json(Self.result("approved")) }
        let r = try await client(MemoryConsentRequestStore(Self.id)).resumeConsent(
            URL(string: "\(Self.returnTo)?result=approved")!)
        XCTAssertNil(r)
        XCTAssertEqual(node.requests.count, 0)
    }

    func testRefusesAReturnWhenThisAppStartedNoRequest() async throws {
        let node = serve { .json(Self.result("approved")) }
        let cloud = client(MemoryConsentRequestStore())
        let error = await refusal { try await cloud.resumeConsent(Self.back()) as Any }
        XCTAssertEqual(error?.code, "CONSENT_NOT_STARTED")
        XCTAssertEqual(node.requests.count, 0)
    }

    func testRefusesAReturnForAnotherRequestKeepingTheStoredOne() async throws {
        let node = serve { .json(Self.result("approved")) }
        let store = MemoryConsentRequestStore(Self.id)
        let cloud = client(store)
        let error = await refusal { try await cloud.resumeConsent(Self.back("approved", "1.forged")) as Any }
        XCTAssertEqual(error?.code, "CONSENT_REQUEST_MISMATCH")
        XCTAssertEqual(error?.details, ["requestId": .string(Self.id), "returned": "1.forged"])
        XCTAssertEqual(node.requests.count, 0)
        XCTAssertEqual(store.load(), Self.id)
    }

    func testRefusesAReturnWhileThisRequestIsStillPending() async throws {
        serve { .json(Self.result("pending")) }
        let store = MemoryConsentRequestStore(Self.id)
        let cloud = client(store)
        let error = await refusal { try await cloud.resumeConsent(Self.back()) as Any }
        XCTAssertEqual(error?.code, "CONSENT_NOT_DECIDED")
        XCTAssertEqual(error?.details, ["requestId": .string(Self.id)])
        XCTAssertEqual(store.load(), Self.id)
    }

    func testRefusesAReturnWhoseRequestANewerOneReplaced() async throws {
        let node = serve { .error(404, "NOT_FOUND", message: "no such consent request") }
        let store = MemoryConsentRequestStore(Self.id)
        let cloud = client(store)
        let error = await refusal { try await cloud.resumeConsent(Self.back()) as Any }
        XCTAssertEqual(error?.code, "CONSENT_REPLACED")
        XCTAssertEqual(error?.details, ["requestId": .string(Self.id)])
        XCTAssertEqual(node.requests.map(\.path), ["/consent/\(Self.id)/result"])
        XCTAssertNil(store.load())
    }

    func testPassesOtherRefusalsThroughKeepingTheStoredId() async throws {
        serve { .error(403, "FORBIDDEN", message: "no") }
        let store = MemoryConsentRequestStore(Self.id)
        let cloud = client(store)
        let error = await refusal { try await cloud.resumeConsent(Self.back()) as Any }
        XCTAssertEqual(error?.status, 403)
        XCTAssertEqual(error?.code, "FORBIDDEN")
        XCTAssertEqual(store.load(), Self.id)
    }

    func testRefusesAnAnswerForAnotherRequest() async throws {
        serve { .json(Self.result("approved", outcome: ["orgId": "o"], requestId: "1.other")) }
        let cloud = client(MemoryConsentRequestStore())
        let error = await refusal { try await cloud.resumeConsent(Self.back(), requestId: Self.id) as Any }
        XCTAssertEqual(error?.code, "CONSENT_REQUEST_MISMATCH")
    }

    func testReadsAGivenRequestIdAndKeepsTheTokenOutOfErrors() async throws {
        serve { .json(Self.result("approved", "createInvite", outcome: ["token": "inv-secret"])) }
        let cloud = client(MemoryConsentRequestStore())
        let r = try await cloud.resumeConsent(Self.back(), requestId: Self.id)
        XCTAssertEqual(r?.inviteToken, "inv-secret")
        MockURLProtocol.reset()

        serve { .json(Self.result("pending", "createInvite")) }
        let error = await refusal { try await cloud.resumeConsent(Self.back(), requestId: Self.id) as Any }
        XCTAssertFalse("\(error?.details ?? [:])".contains("inv-secret"))
    }

    func testStripsOnlyTheConsentParameters() {
        XCTAssertEqual(
            withoutConsentReturn(URL(string: "https://a.example/p?x=1&request_id=r&result=approved&y=2#h")!),
            URL(string: "https://a.example/p?x=1&y=2#h")!)
        XCTAssertEqual(
            withoutConsentReturn(URL(string: "https://a.example/p?request_id=r&result=approved")!),
            URL(string: "https://a.example/p")!)
    }

    func testTheUserDefaultsStoreKeepsOnlyTheId() {
        let suite = "network.calimero.mero-swift.tests.\(UUID().uuidString)"
        let store = UserDefaultsConsentRequestStore(suiteName: suite)
        XCTAssertNil(store.load())
        store.save(Self.id)
        XCTAssertEqual(store.load(), Self.id)
        XCTAssertEqual(UserDefaults(suiteName: suite)?.string(forKey: consentStorageKey), Self.id)
        store.clear()
        XCTAssertNil(store.load())
        UserDefaults().removePersistentDomain(forName: suite)
    }
}
