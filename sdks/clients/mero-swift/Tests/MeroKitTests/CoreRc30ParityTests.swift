import Foundation
import MeroKitTestSupport
import XCTest

@testable import MeroKit

/// What core 1.0.0-rc.30 answers that mero-swift now reads as mero-js does:
/// the rollout and passkey refusals, event ids, presence states, and the
/// account recovery ceremony.
final class CoreRc30ParityTests: XCTestCase {
    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    private func client() -> CalimeroClient {
        CalimeroClient(
            CalimeroClientOptions(
                baseUrl: "https://node.test", policy: .development, session: MockURLProtocol.makeSession(),
                retry: CloudRetryOptions(retries: 0)))
    }

    func testTypesTheRefusalsMeroJsTypes() {
        for (status, code, type) in [
            (401, "PASSKEY_UNKNOWN_HERE", "PasskeyUnknownHere"), (409, "RUNTIME_TOO_OLD", "RuntimeTooOld"),
            (400, "NOT_SERVING", "NotServing"), (409, "CLUSTER_NOT_UPGRADED", "ClusterNotUpgraded"),
            (409, "FEATURE_NOT_ACTIVE", "FeatureNotActive"),
        ] {
            let error = errorFrom(status: status, body: ["error": .string(code), "message": "no"])
            XCTAssertEqual(String(describing: Swift.type(of: error)), type, code)
            XCTAssertEqual(error.code, code)
            XCTAssertEqual(error.status, status)
        }
        let feature = errorFrom(
            status: 409,
            body: [
                "error": "FEATURE_NOT_ACTIVE", "message": "later",
                "details": ["feature": "appTeams", "needs": "1.0.0-rc.30", "active": "1.0.0-rc.29"],
            ])
        let active = feature as? FeatureNotActive
        XCTAssertEqual(active?.feature, "appTeams")
        XCTAssertEqual(active?.needs, "1.0.0-rc.30")
        XCTAssertEqual(active?.active, "1.0.0-rc.29")
        let runtime = errorFrom(
            status: 409,
            body: [
                "error": "RUNTIME_TOO_OLD", "details": ["minRuntimeVersion": "0.2.0", "nodes": [["node": 1]]],
            ])
        XCTAssertEqual((runtime as? RuntimeTooOld)?.minRuntimeVersion, "0.2.0")
        XCTAssertEqual((runtime as? RuntimeTooOld)?.nodes.count, 1)
        // An unknown passkey is not a session to refresh.
        XCTAssertFalse(errorFrom(status: 401, body: ["error": "PASSKEY_UNKNOWN_HERE"]) is LoginRequired)
        XCTAssertFalse((errorFrom(status: 401, body: ["error": "PASSKEY_UNKNOWN_HERE"])).message.isEmpty)
    }

    func testReadsAnEventsIndexAndSeqAndPresenceStates() {
        let event = CloudEvent(id: "12:3", kind: "Set", data: .null)
        XCTAssertEqual(event.index, 12)
        XCTAssertEqual(event.seq, 3)
        XCTAssertNil(CloudEvent(id: "", kind: "snapshot", data: .null).index)
        XCTAssertNil(CloudEvent(id: "a:b", kind: "x", data: .null).seq)
        XCTAssertEqual(presenceState(Data([0x01, 0xab, 0xff])), "01abff")
        XCTAssertNil(presenceState(nil))
    }

    func testResumesEventsAfterAnIdSkippingWhatItSaw() async throws {
        let node = FakeCloudNode { _ in
            .events(
                [
                    FakeCloudEvent(id: "3:0", kind: "Posted", data: ["id": "m1"]),
                    FakeCloudEvent(id: "3:1", kind: "Posted", data: ["id": "m2"]),
                    FakeCloudEvent(id: "4:0", kind: "Posted", data: ["id": "m3"]),
                ], holdOpen: true)
        }
        node.install()
        var seen: [CloudEvent] = []
        for try await event in client().events("a1", after: "3:0") {
            seen.append(event)
            if seen.count == 2 { break }
        }
        XCTAssertEqual(seen.map(\.id), ["3:1", "4:0"])
        XCTAssertEqual(node.requests.first?.path, "/apps/a1/events?from=3")

        do {
            for try await _ in client().events("a1", after: "not-an-id") {}
            XCTFail("expected a refusal")
        } catch AccountError.invalidInput {}
    }

    func testStartsAnAccountRecoveryAndTypesAWrongCode() async throws {
        let node = FakeCloudNode { request in
            switch request.path {
            case "/auth/recovery/begin":
                if request.json?["code"] == "bad" {
                    return .error(401, "UNAUTHENTICATED", message: "unknown recovery code")
                }
                return .json([
                    "challengeId": "rc1",
                    "options": [
                        "challenge": "AAAA", "rp": ["id": "localhost", "name": "x"],
                        "user": ["id": "AAAA", "name": "a", "displayName": "a"],
                        "pubKeyCredParams": [["type": "public-key", "alg": -7]],
                    ],
                ])
            case "/auth/recovery/start":
                return .json(["accountId": "acc1", "credentialId": "cred1", "startedAt": 10, "readyAt": 259_210])
            default:
                return .error(404, "NOT_FOUND")
            }
        }
        node.install()
        do {
            _ = try await client().startAccountRecovery(code: "bad", passkey: SoftPasskey())
            XCTFail("expected WrongRecoveryCode")
        } catch is WrongRecoveryCode {}
        let recovery = try await client().startAccountRecovery(code: "good", passkey: SoftPasskey())
        XCTAssertEqual(
            recovery, AccountRecovery(accountId: "acc1", credentialId: "cred1", startedAt: 10, readyAt: 259_210))
        XCTAssertEqual(node.requests.last?.json?["challengeId"], "rc1")
        XCTAssertEqual(node.requests.last?.json?["code"], "good")
    }
}
