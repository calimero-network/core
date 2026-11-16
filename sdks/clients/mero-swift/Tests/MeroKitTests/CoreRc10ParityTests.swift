import CryptoKit
import Foundation
import MeroAPI
import MeroKitTestSupport
import XCTest

@testable import MeroKit

/// core rc.8–rc.10 parity (mero-js #326–#331): sign-up proofs and policy,
/// confirmed key revocation, upgrade keys, package origins, organization
/// recovery and sealed files.
final class CoreRc10ParityTests: XCTestCase {
    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    private func client() -> CalimeroClient {
        CalimeroClient(
            CalimeroClientOptions(
                baseUrl: "https://node.test", policy: CloudPolicy(allowMock: true),
                session: MockURLProtocol.makeSession(), retry: CloudRetryOptions(retries: 0)),
            sleep: { _ in })
    }

    private static let stepUp: JSONValue = [
        "challengeId": "su1", "challenge": .string(b64url(Data(repeating: 5, count: 32))), "rpId": "localhost",
    ]

    private static let session: JSONValue = [
        "accountId": "acc_1", "sessionToken": "v2.one", "expiresAt": 1_900_000_000, "refreshToken": "rt_one",
    ]

    private static let registerBegin: JSONValue = [
        "challengeId": "ch1",
        "options": [
            "challenge": .string(b64url(Data(repeating: 7, count: 32))),
            "rp": ["id": "localhost", "name": "x"],
            "user": ["id": .string(b64url(Data([1, 2, 3]))), "name": "ada", "displayName": "ada"],
        ],
    ]

    func testASignUpAnswersTheNewPasskeysProofAndCarriesTheInviteOnEveryHalf() async throws {
        let node = FakeCloudNode { request in
            switch request.path {
            case "/auth/passkey/register/begin": return .json(Self.registerBegin)
            case "/auth/passkey/register/finish":
                if request.json?["confirm"] == nil { return .error(412, "CONFIRM_REQUIRED", details: Self.stepUp) }
                return .json(Self.session)
            default: return .error(404, "NOT_FOUND")
            }
        }
        node.install()
        let cloud = client()
        let session = try await cloud.signUp(SoftPasskey(), name: "bob", invite: "inv_1")
        XCTAssertEqual(session.accountId, "acc_1")
        XCTAssertEqual(
            node.requests.map(\.path),
            ["/auth/passkey/register/begin", "/auth/passkey/register/finish", "/auth/passkey/register/finish"])
        XCTAssertEqual(node.requests[0].json?["invite"], "inv_1")
        XCTAssertEqual(node.requests[0].json?["name"], "bob")
        XCTAssertNil(node.requests[0].json?["origin"], "a native app names no origin")
        XCTAssertEqual(node.requests[1].json?["invite"], "inv_1")
        XCTAssertNil(node.requests[1].json?["confirm"])
        let retry = try XCTUnwrap(node.requests[2].json)
        XCTAssertEqual(retry["invite"], "inv_1")
        XCTAssertEqual(retry["challengeId"], "ch1")
        XCTAssertNotNil(retry["registration"])
        XCTAssertEqual(retry["confirm"]?["challengeId"], "su1")
        let current = await cloud.session
        XCTAssertEqual(current?.sessionToken, "v2.one")
    }

    func testReadsTheSignUpPolicyAndTypesItsRefusals() async throws {
        let node = FakeCloudNode { request in
            if request.path == "/sign-up" {
                return .json(["policy": "invite", "orgInvites": true, "signUpInvites": true, "inviteRequired": true])
            }
            return .error(
                403, "SIGN_UP_REFUSED", message: "sign-up refused: this cluster takes new accounts by invite only")
        }
        node.install()
        let cloud = client()
        let policy = try await cloud.answer(as: SignUpPolicy.self) { try await cloud.deployments.signUpPublic() }
        XCTAssertEqual(
            policy, SignUpPolicy(policy: .invite, orgInvites: true, signUpInvites: true, inviteRequired: true))
        XCTAssertNil(node.requests[0].bearer)
        do {
            try await cloud.signUp(SoftPasskey(), name: "bob")
            XCTFail("expected SignUpRefused")
        } catch let error as SignUpRefused {
            XCTAssertEqual(error.reason, .inviteOnly)
        }
        XCTAssertEqual(node.requests.count, 2, "refused at begin, before any passkey is made")

        for (message, reason) in [
            ("sign-up refused: the invite has expired", SignUpRefusal.expiredInvite),
            ("sign-up refused: unknown invite", .unknownInvite),
            ("sign-up refused: the invite's organization is gone", .orgGone),
            (
                "sign-up refused: this cluster takes only accounts its operator or owner pre-authorized with a sign-up invite",
                .closed
            ),
            ("something else", .other),
        ] {
            let error = errorFrom(status: 403, body: ["error": "SIGN_UP_REFUSED", "message": .string(message)])
            XCTAssertEqual((error as? SignUpRefused)?.reason, reason, message)
        }
    }

    func testAnUnregisteredOriginNamesTheOriginTheCeremonySent() async throws {
        let node = FakeCloudNode { _ in .error(403, "ORIGIN_NOT_REGISTERED", message: "this origin is not one") }
        node.install()
        do {
            try await client().signIn(SoftPasskey(), origin: "https://chat.example.com")
            XCTFail("expected OriginNotRegistered")
        } catch let error as OriginNotRegistered {
            XCTAssertEqual(error.origin, "https://chat.example.com")
            XCTAssertEqual(error.status, 403)
        }
        XCTAssertEqual(node.requests[0].json?["origin"], "https://chat.example.com")
    }

    func testSignInTakesTheRelyingPartyFromRpWhenTheNodeAnswersIt() async throws {
        let node = FakeCloudNode { request in
            switch request.path {
            case "/auth/passkey/login/begin":
                return .json([
                    "challengeId": "ch2",
                    "options": [
                        "challenge": .string(b64url(Data(repeating: 9, count: 32))), "rp": ["id": "localhost"],
                    ],
                ])
            default: return .json(Self.session)
            }
        }
        node.install()
        try await client().signIn(SoftPasskey(), origin: "https://chat.example.com")
        XCTAssertEqual(node.requests.map(\.path), ["/auth/passkey/login/begin", "/auth/passkey/login/finish"])
    }

    func testMeAnswersThePackageAndOriginASessionIsBoundTo() async throws {
        FakeCloudNode { _ in
            .json(["accountId": "acc_1", "via": "session", "app": "com.example.chat", "origin": "https://c.example"])
        }.install()
        let cloud = client()
        let me = AccountInfo(json: try await cloud.answerJSON { try await cloud.auth.me() })
        XCTAssertEqual(me.via, "session")
        XCTAssertEqual(me.app, "com.example.chat")
        XCTAssertEqual(me.origin, "https://c.example")
    }

    func testRevokingAKeyIsApprovedAndAnUpgradeKeyNamesItsApp() async throws {
        let node = FakeCloudNode { request in
            if request.json?["confirm"] == nil { return .error(412, "CONFIRM_REQUIRED", details: Self.stepUp) }
            return .json(["keyId": "k1", "key": "cal_up"])
        }
        node.install()
        let cloud = client()
        let passkey = SoftPasskey()
        _ = try await cloud.answerJSON { try await cloud.auth.apikeyRevoke(key: "k0", passkey: passkey) }
        let key = try await cloud.auth.apikeyCreate(
            label: "ci", ttlSecs: 86_400, upgrades: .init(app: "app1"), passkey: passkey)
        XCTAssertEqual(key.key, "cal_up")
        XCTAssertEqual(
            node.requests.map { "\($0.method) \($0.path)" },
            [
                "DELETE /auth/apikeys/k0", "DELETE /auth/apikeys/k0", "POST /auth/apikeys", "POST /auth/apikeys",
            ])
        XCTAssertEqual(node.requests[1].json?["confirm"]?["challengeId"], "su1")
        let body = try XCTUnwrap(node.requests[3].json)
        XCTAssertEqual(body["upgrades"], ["app": "app1"])
        XCTAssertEqual(body["label"], "ci")
    }

    func testRegistersListsAndRemovesAPackagesOrigins() async throws {
        let row: JSONValue = [
            "package": "com.example.chat", "origin": "https://chat.example.com", "rpId": "chat.example.com",
            "addedBy": "acc_1", "addedAt": "2026-11-05T00:00:00Z",
        ]
        let node = FakeCloudNode { request in
            if request.method == "GET" { return .json(["package": "com.example.chat", "origins": [row]]) }
            if request.json?["confirm"] == nil { return .error(412, "CONFIRM_REQUIRED", details: Self.stepUp) }
            if request.method == "DELETE" {
                return .json(["package": "com.example.chat", "origin": "https://chat.example.com", "removed": true])
            }
            return .json(row)
        }
        node.install()
        let cloud = client()
        let passkey = SoftPasskey()
        let added = try await cloud.answer(as: PackageOrigin.self) {
            try await cloud.packages.originsAdd(
                package: "com.example.chat", origin: "https://chat.example.com", rpId: "chat.example.com",
                passkey: passkey)
        }
        XCTAssertEqual(added.rpId, "chat.example.com")
        XCTAssertEqual(added.package, "com.example.chat")
        let listed = try await cloud.answer(as: OriginListing.self) {
            try await cloud.packages.originsList(package: "com.example.chat")
        }.origins
        XCTAssertEqual(listed, [added])
        _ = try await cloud.answerJSON {
            try await cloud.packages.originsRemove(
                package: "com.example.chat", origin: "https://chat.example.com", passkey: passkey)
        }

        let routes = node.requests.map { "\($0.method) \($0.path)" }
        XCTAssertEqual(
            routes,
            [
                "POST /package-origins/com.example.chat", "POST /package-origins/com.example.chat",
                "GET /package-origins/com.example.chat",
                "DELETE /package-origins/com.example.chat", "DELETE /package-origins/com.example.chat",
            ])
        XCTAssertEqual(node.requests[1].json?["origin"], "https://chat.example.com")
        XCTAssertEqual(node.requests[1].json?["rpId"], "chat.example.com")
        XCTAssertEqual(node.requests[4].json?["origin"], "https://chat.example.com")
        XCTAssertEqual(node.requests[4].json?["confirm"]?["challengeId"], "su1")

        for (status, code, type) in [
            (400, "BAD_ORIGIN", "BadOrigin"), (400, "BAD_RP_ID", "BadOrigin"), (409, "ORIGIN_TAKEN", "OriginTaken"),
            (429, "QUOTA_EXCEEDED", "QuotaExceeded"),
        ] {
            let error = errorFrom(status: status, body: ["error": .string(code), "message": "no"])
            XCTAssertEqual(String(describing: Swift.type(of: error)), type, code)
            XCTAssertEqual(error.code, code)
        }
    }

    func testRecoversAnOrganizationWithItsSeed() async throws {
        let seed = RecoverySeed()
        let node = FakeCloudNode { request in
            switch request.path {
            case "/orgs": return .json(["orgId": "o1"])
            case "/auth/me":
                return .json([
                    "accountId": "acc_new", "name": "new", "passkeys": [], "orgs": [], "via": "session",
                    "passkeyId": .null, "approvedReleases": [], "outboundApprovals": [:],
                ])
            case "/orgs/o1/recovery/start": return .json(["startedAt": 100, "readyAt": 259_300])
            case "/orgs/o1/recovery/finish":
                if request.json?["confirm"] == nil { return .error(412, "CONFIRM_REQUIRED", details: Self.stepUp) }
                return .error(409, "RECOVERY_NOT_READY", message: "not yet", details: ["readyAt": 259_300])
            default:
                if request.json?["confirm"] == nil { return .error(412, "CONFIRM_REQUIRED", details: Self.stepUp) }
                return .json([:])
            }
        }
        node.install()
        let cloud = client()
        let passkey = SoftPasskey()
        let org = try await cloud.orgs.create(name: "Acme", recoveryKey: seed.publicKey).orgId
        XCTAssertEqual(org, "o1")
        XCTAssertEqual(node.requests[0].json?["recoveryKey"], .string(seed.publicKey))
        try await cloud.setRecoveryKey("o1", publicKey: nil, passkey: passkey)
        XCTAssertEqual(node.requests[2].json?["publicKey"], .null)
        XCTAssertEqual(node.requests[2].json?["confirm"]?["challengeId"], "su1")

        let times = try await cloud.startRecovery("o1", seed: seed)
        XCTAssertEqual(times, RecoveryTimes(startedAt: 100, readyAt: 259_300))
        let start = try XCTUnwrap(node.requests.last?.json)
        let nonce = try XCTUnwrap(start["nonce"]?.stringValue)
        let signature = try fromB64url(try XCTUnwrap(start["signature"]?.stringValue))
        let publicKey = try Curve25519.Signing.PublicKey(rawRepresentation: try fromB64url(seed.publicKey))
        XCTAssertTrue(
            publicKey.isValidSignature(
                signature, for: RecoverySeed.message(org: "o1", account: "acc_new", nonce: nonce)),
            "the seed signs the organization, this account and the nonce")

        _ = try await cloud.answerJSON { try await cloud.orgs.recoveryCancel(org: "o1", passkey: passkey) }
        do {
            _ = try await cloud.orgs.recoveryFinish(org: "o1", passkey: passkey)
            XCTFail("expected RecoveryNotReady")
        } catch let error as RecoveryNotReady {
            XCTAssertEqual(error.readyAt, 259_300)
        }
    }

    func testAnOrganizationShowsItsRecoveryUnderWay() {
        let org = OrgInfo(json: [
            "orgId": "o1", "name": "Acme",
            "recovery": [
                "publicKey": "pk", "setAt": 5, "pending": ["account": "acc_2", "startedAt": 10, "readyAt": 20],
            ],
        ])
        XCTAssertEqual(
            org.recovery,
            OrgRecovery(
                publicKey: "pk", setAt: 5, pending: PendingRecovery(account: "acc_2", startedAt: 10, readyAt: 20)))
        XCTAssertNil(OrgInfo(json: ["orgId": "o2"]).recovery)
        XCTAssertNil(
            OrgInfo(json: ["orgId": "o3", "recovery": ["publicKey": "pk", "setAt": 1, "pending": nil]]).recovery?
                .pending)
    }

    func testTheRecoverySeedMatchesCoresPinnedBytes() throws {
        // Pinned in core: `calimero_identity::recovery::the_wire_bytes_are_pinned_for_the_clients`.
        let seed = try RecoverySeed(bytes: Data(repeating: 1, count: 32))
        XCTAssertEqual(
            String(decoding: RecoverySeed.message(org: "o", account: "a", nonce: "n"), as: UTF8.self),
            "calimero.org-recovery.v1\0o\0a\0n")
        XCTAssertEqual(seed.publicKey, "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w")
        // CryptoKit's Ed25519 signatures are randomized, so check it verifies
        // rather than pinning core's (deterministic) bytes.
        let signature = try fromB64url(try seed.sign(org: "o", account: "a", nonce: "n"))
        let publicKey = try Curve25519.Signing.PublicKey(rawRepresentation: try fromB64url(seed.publicKey))
        XCTAssertTrue(
            publicKey.isValidSignature(signature, for: RecoverySeed.message(org: "o", account: "a", nonce: "n")))
        // Core's own signature over the same bytes verifies too.
        let cores = try fromB64url(
            "bbbHMaH2SC50zQADskIDS5xg8WxZFrtxdasJzhzCOMZh-aBrUxeDKro2_UUIyU4PzH5LmsS1gQRQmVfx3E1eDw")
        XCTAssertTrue(publicKey.isValidSignature(cores, for: RecoverySeed.message(org: "o", account: "a", nonce: "n")))

        XCTAssertThrowsError(try seed.sign(org: "o", account: "a", nonce: ""))
        XCTAssertThrowsError(try seed.sign(org: "o", account: "a", nonce: String(repeating: "n", count: 65)))

        let s = try RecoverySeed(bytes: Data((0..<32).map { UInt8(($0 * 7) & 0xff) }))
        XCTAssertEqual(s.text.count, 71)
        XCTAssertEqual(try RecoverySeed(text: s.text), s)
        XCTAssertEqual(try RecoverySeed(text: s.text.uppercased().replacingOccurrences(of: "-", with: " ")), s)
        XCTAssertThrowsError(try RecoverySeed(text: String(s.text.dropLast())))
        XCTAssertThrowsError(try RecoverySeed(text: s.text.replacingOccurrences(of: "0", with: "g")))
        XCTAssertThrowsError(try RecoverySeed(bytes: Data(count: 31)))
    }

    func testUploadsASealedFileForAComponentApp() async throws {
        let node = FakeCloudNode { _ in
            .json([
                "blobId": "2Ab", "id": "ab", "size": 5, "sha256": "e3b0", "sealed": true, "index": 7,
            ])
        }
        node.install()
        let file = try await client().uploadFile("app1", Data("hello".utf8))
        XCTAssertEqual(file, BlobInfo(blobId: "2Ab", id: "ab", size: 5, sha256: "e3b0", sealed: true, index: 7))
        XCTAssertEqual(node.requests[0].method, "POST")
        XCTAssertEqual(node.requests[0].path, "/apps/app1/blobs?sealed=true")
        XCTAssertEqual(node.requests[0].body, Data("hello".utf8))
    }
}

/// `GET /package-origins/{package}`'s answer, read as ``PackageOrigin``s.
private struct OriginListing: Decodable {
    let origins: [PackageOrigin]
}
