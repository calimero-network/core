import Foundation
import MeroAPI
import MeroKitTestSupport
import XCTest

@testable import MeroKit

/// A thread-safe box for what a fake node saw.
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

/// Port of mero-js `cloud-v2/client.test.ts`'s journeys, against a
/// ``FakeCloudNode`` that runs the real sealed channel (Noise NK, sealed
/// frames) and mock attestation.
final class CalimeroClientTests: XCTestCase {
    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    private func client(
        _ base: String = "https://node.test", nodes: [String] = [], allowMock: Bool = true
    ) -> CalimeroClient {
        CalimeroClient(
            CalimeroClientOptions(
                baseUrl: base, nodes: nodes, policy: CloudPolicy(allowMock: allowMock),
                session: MockURLProtocol.makeSession(), retry: CloudRetryOptions(retries: 3)),
            sleep: { _ in })
    }

    private static let session: JSONValue = [
        "accountId": "acc_1", "sessionToken": "v2.one", "expiresAt": 1_900_000_000, "refreshToken": "rt_one",
    ]

    private static func passkeyCeremonies(_ request: FakeCloudRequest) -> FakeCloudAnswer? {
        switch request.path {
        case "/auth/passkey/register/begin":
            return .json([
                "challengeId": "ch1",
                "options": [
                    "challenge": .string(b64url(Data(repeating: 7, count: 32))),
                    "rp": ["id": "localhost", "name": "x"],
                    "user": ["id": .string(b64url(Data([1, 2, 3]))), "name": "ada", "displayName": "ada"],
                ],
            ])
        case "/auth/passkey/login/begin":
            return .json([
                "challengeId": "ch2",
                "options": ["challenge": .string(b64url(Data(repeating: 9, count: 32))), "rpId": "localhost"],
            ])
        case "/auth/passkey/register/finish", "/auth/passkey/login/finish": return .json(session)
        default: return nil
        }
    }

    func testSignsUpWithAPasskeyOverTheSealedChannelAndSendsTheSessionAfter() async throws {
        let node = FakeCloudNode { request in
            Self.passkeyCeremonies(request) ?? .json(["accountId": "acc_1", "orgs": []])
        }
        node.install()
        let cloud = client()
        let session = try await cloud.signUp(SoftPasskey(), name: "ada")
        XCTAssertEqual(session.accountId, "acc_1")
        let me = AccountInfo(json: try await cloud.answerJSON { try await cloud.auth.me() })
        XCTAssertEqual(me.accountId, "acc_1")

        XCTAssertEqual(node.attests, 1)
        XCTAssertEqual(node.handshakes, 1)
        XCTAssertEqual(
            node.requests.map(\.path), ["/auth/passkey/register/begin", "/auth/passkey/register/finish", "/auth/me"])
        let finish = try XCTUnwrap(node.requests[1].json)
        XCTAssertEqual(finish["challengeId"], "ch1")
        XCTAssertNotNil(finish["registration"]?["attestationObject"]?.stringValue)
        XCTAssertNil(node.requests[0].bearer)
        XCTAssertEqual(node.requests[2].bearer, "v2.one")
    }

    func testSignsInWithTheAssertionOverTheNodesChallenge() async throws {
        let node = FakeCloudNode { Self.passkeyCeremonies($0) ?? .error(404, "NOT_FOUND") }
        node.install()
        let passkey = SoftPasskey()
        try await client().signIn(passkey)
        let assertion = try XCTUnwrap(node.requests[1].json?["assertion"])
        XCTAssertEqual(assertion["credentialId"]?.stringValue, passkey.credentialId)
        let clientData = try JSONDecoder().decode(
            JSONValue.self, from: try fromB64url(assertion["clientDataJson"]?.stringValue ?? ""))
        XCTAssertEqual(clientData["challenge"]?.stringValue, b64url(Data(repeating: 9, count: 32)))
        XCTAssertEqual(clientData["type"], "webauthn.get")
    }

    func testRefusesAMockQuoteUnlessTheCallerAllowsMock() async throws {
        let node = FakeCloudNode { _ in .json([:]) }
        node.install()
        do {
            _ = try await client(allowMock: false).auth.me()
            XCTFail("a mock quote must be refused")
        } catch let error as TeeTrustError {
            XCTAssertTrue(error.message.contains("mock quote refused"), error.message)
        }
        XCTAssertEqual(node.handshakes, 0, "nothing is sent to a node that did not attest")
    }

    func testCallsAndViewsAnAppAndRetriesA503WithTheSameIdempotencyKey() async throws {
        let attempts = Counter()
        let node = FakeCloudNode { request in
            if request.path == "/apps/a1/intents" {
                attempts.increment()
                if attempts.count < 3 { return .error(503, "TIMEOUT") }
                return .json(["index": 7, "result": ["ok": true], "events": [["kind": "Set", "data": "a"]]])
            }
            return .json(["index": 7, "result": "1"])
        }
        node.install()
        let cloud = client()
        let call = try await cloud.call("a1", "set", ["key": "a"])
        XCTAssertEqual(call.index, 7)
        XCTAssertEqual(call.result?["ok"], true)
        XCTAssertEqual(call.events.first?.kind, "Set")
        let keys = node.requests.filter { $0.path == "/apps/a1/intents" }.compactMap {
            $0.json?["idempotencyKey"]?.stringValue
        }
        XCTAssertEqual(keys.count, 3)
        XCTAssertEqual(Set(keys).count, 1, "every retry carries the same key")
        XCTAssertTrue(keys[0].hasPrefix("node:c-"))

        let view = try await cloud.view("a1", "get", ["key": "a"])
        XCTAssertEqual(view.result, "1")
        let body = try XCTUnwrap(node.requests.last?.json)
        XCTAssertEqual(body["consistency"], "strong")
        XCTAssertEqual(body["method"], "get")
    }

    func testGivesUpOnA503AfterItsRetriesAsUnavailable() async throws {
        let node = FakeCloudNode { _ in .error(503, "MIGRATING", message: "moving") }
        node.install()
        do {
            _ = try await client().call("a1", "set")
            XCTFail("expected unavailable")
        } catch let error as UnavailableError {
            XCTAssertEqual(error.reason, "migrating")
            XCTAssertEqual(error.committed, .no)
        }
        XCTAssertEqual(node.requests.count, 4, "the first attempt and 3 retries")
    }

    func testMapsTheNodesRefusalsToTypedErrors() {
        func error(_ status: Int, _ code: String, _ details: JSONValue? = nil) -> CalimeroError {
            var body: [String: JSONValue] = ["error": .string(code), "message": "m"]
            if let details { body["details"] = details }
            return errorFrom(status: status, body: .object(body))
        }
        XCTAssertTrue(error(401, "UNAUTHENTICATED") is LoginRequired)
        XCTAssertTrue(error(421, "WRONG_NODE") is WrongNode)
        XCTAssertEqual((error(421, "NOT_LEADER", ["leaderUrl": "https://n2"]) as? NotLeader)?.leaderUrl, "https://n2")
        XCTAssertTrue(error(404, "NOT_FOUND") is NotFound)
        XCTAssertTrue(error(412, "CONFIRM_REQUIRED") is ConfirmRequired)
        XCTAssertTrue(error(413, "ARGS_TOO_LARGE") is ArgsTooLarge)
        XCTAssertTrue(error(409, "IDEMPOTENCY_MISMATCH") is IdempotencyMismatch)
        XCTAssertTrue(error(409, "CONSENT_STALE") is ConsentStale)
        XCTAssertEqual(
            (error(409, "RELEASE_DEADLINE", ["release": "2.3.110"]) as? ReleaseDeadlineError)?.release, "2.3.110")
        XCTAssertEqual((error(422, "TRAP", ["kind": "OutOfFuel"]) as? TrapError)?.kind, "OutOfFuel")
        let app =
            error(422, "APP_ERROR", ["index": 1, "error": ["code": "Taken", "fields": ["name": "x"]]]) as? AppError
        XCTAssertEqual(app?.appCode, "Taken")
        XCTAssertEqual(app?.fields["name"], "x")
        let flat = error(422, "APP_ERROR", ["code": "Forbidden", "fields": ["0": "positional"]]) as? AppError
        XCTAssertEqual(flat?.appCode, "Forbidden")
        let unavailable = errorFrom(status: 503, body: ["error": "UNAVAILABLE"], retryAfter: "2") as? UnavailableError
        XCTAssertEqual(unavailable?.committed, .unknown)
        XCTAssertEqual(unavailable?.retryAfter, 2)
        let other = error(409, "CONFLICT")
        XCTAssertFalse(other is IdempotencyMismatch)
        XCTAssertEqual(other.code, "CONFLICT")
    }

    func testRefreshesOnceForConcurrentRequestsAfterA401AndRepeatsThem() async throws {
        let refreshes = Counter()
        let node = FakeCloudNode { request in
            if request.path == "/auth/refresh" {
                refreshes.increment()
                return .json([
                    "accountId": "acc_1", "sessionToken": "v2.two", "expiresAt": 1_900_000_000,
                    "refreshToken": "rt_two",
                ])
            }
            if request.bearer == "v2.one" { return .error(401, "UNAUTHENTICATED") }
            return .json(["index": 1, "result": .string(request.bearer ?? "")])
        }
        node.install()
        let cloud = client()
        let seen = Box<[CalimeroSession?]>([])
        await cloud.onSessionChange { session in seen.update { $0.append(session) } }
        await cloud.useSession(
            CalimeroSession(accountId: "acc_1", sessionToken: "v2.one", expiresAt: 0, refreshToken: "rt_one"))
        async let first = cloud.view("a1", "get")
        async let second = cloud.view("a1", "get")
        let results = try await [first, second]
        XCTAssertEqual(results.map(\.result), ["v2.two", "v2.two"])
        XCTAssertEqual(refreshes.count, 1, "a refresh token is used once")
        XCTAssertEqual(node.requests.first { $0.path == "/auth/refresh" }?.json?["refreshToken"], "rt_one")
        let current = await cloud.session
        XCTAssertEqual(current?.refreshToken, "rt_two")
        XCTAssertEqual(seen.get.last??.refreshToken, "rt_two")
    }

    func testARefusedRefreshSurfacesAsLoginRequired() async throws {
        let node = FakeCloudNode { _ in .error(401, "UNAUTHENTICATED", message: "expired") }
        node.install()
        let cloud = client()
        await cloud.useSession(
            CalimeroSession(accountId: "a", sessionToken: "v2.x", expiresAt: 0, refreshToken: "rt_x"))
        do {
            _ = try await cloud.answerJSON { try await cloud.auth.me() }
            XCTFail("expected LoginRequired")
        } catch is LoginRequired {}
        XCTAssertEqual(node.requests.map(\.path), ["/auth/me", "/auth/refresh"])
    }

    func testMovesToTheNextNodeOnWrongNodeAndRepeatsTheRequestThere() async throws {
        let first = FakeCloudNode(baseURL: "https://n1.test") { _ in .error(421, "WRONG_NODE") }
        let second = FakeCloudNode(baseURL: "https://n2.test") { _ in .json(["index": 2, "result": "there"]) }
        FakeCloudNode.install([first, second])
        let cloud = client("https://n1.test", nodes: ["https://n2.test/"])
        let answer = try await cloud.view("a1", "get")
        XCTAssertEqual(answer.result, "there")
        let now = await cloud.nodeUrl
        XCTAssertEqual(now, "https://n2.test")
        XCTAssertEqual(second.attests, 1, "the next node is attested before anything is sent to it")
    }

    func testAWrongNodeWithNoOtherNodeReachesTheCaller() async throws {
        FakeCloudNode { _ in .error(421, "WRONG_NODE") }.install()
        do {
            let cloud = client()
            _ = try await cloud.answerJSON { try await cloud.auth.me() }
            XCTFail("expected WrongNode")
        } catch is WrongNode {}
    }

    func testFollowsNotLeaderToALeaderItKnows() async throws {
        let follower = FakeCloudNode(baseURL: "https://n1.test") { _ in
            .error(421, "NOT_LEADER", details: ["leader": 2, "leaderUrl": "https://n2.test"])
        }
        let leader = FakeCloudNode(baseURL: "https://n2.test") { _ in .json(["status": "ok"]) }
        FakeCloudNode.install([follower, leader])
        let answer = try await client("https://n1.test", nodes: ["https://n2.test"]).post("ops/nodes", [:])
        XCTAssertEqual(answer["status"], "ok")
        XCTAssertEqual(leader.requests.count, 1)
    }

    func testAnswersAConfirmRequiredWithThePasskeyAndSendsTheCallOnce() async throws {
        let node = FakeCloudNode { request in
            if request.json?["confirm"] == nil {
                return .error(
                    412, "CONFIRM_REQUIRED",
                    details: [
                        "challengeId": "c9", "challenge": .string(b64url(Data(repeating: 3, count: 32))),
                        "rpId": "localhost",
                    ])
            }
            return .json(["index": 4, "result": "paid"])
        }
        node.install()
        let answer = try await client().callConfirmed("a1", "pay", ["amount": 5], passkey: SoftPasskey())
        XCTAssertEqual(answer.result, "paid")
        let confirmed = try XCTUnwrap(node.requests.last?.json)
        XCTAssertEqual(confirmed["confirm"]?["challengeId"], "c9")
        XCTAssertEqual(confirmed["idempotencyKey"], node.requests.first?.json?["idempotencyKey"])
    }

    func testADeclinedConfirmationIsConfirmCancelled() async throws {
        struct Declining: PasskeyProvider {
            func create(options: JSONValue) async throws -> RegistrationJSON { throw PasskeyCancelled() }
            func get(challenge: Data, rpId: String, allowCredentials: [String]?) async throws -> AssertionJSON {
                throw PasskeyCancelled()
            }
        }
        FakeCloudNode { _ in
            .error(412, "CONFIRM_REQUIRED", details: ["challengeId": "c", "challenge": "AAAA", "rpId": "localhost"])
        }.install()
        do {
            _ = try await client().orgs.delete(org: "o1", passkey: Declining())
            XCTFail("expected ConfirmCancelled")
        } catch is ConfirmCancelled {}
    }

    func testStreamsAnAppsEventsFromTheSealedEventStream() async throws {
        let node = FakeCloudNode { _ in
            .events(
                [
                    FakeCloudEvent(id: "3:0", kind: "Posted", data: ["id": "m1"]),
                    FakeCloudEvent(id: "3:1", kind: "Posted", data: ["id": "m2"]),
                ], holdOpen: true)
        }
        node.install()
        var seen: [CloudEvent] = []
        for try await event in client().events("a1", from: 3) {
            seen.append(event)
            if seen.count == 2 { break }
        }
        XCTAssertEqual(seen.map(\.id), ["3:0", "3:1"])
        XCTAssertEqual(seen.map(\.kind), ["Posted", "Posted"])
        XCTAssertEqual(seen.last?.data["id"], "m2")
        XCTAssertEqual(node.requests.first?.path, "/apps/a1/events?from=3")
    }

    func testOrgsTeamsInvitesAndConsent() async throws {
        let node = FakeCloudNode { request in
            switch (request.method, request.path) {
            case ("POST", "/orgs"): return .json(["orgId": "o1"])
            case ("POST", "/orgs/o1/teams"): return .json(["teamId": "t1"])
            case ("POST", "/orgs/o1/invites"): return .json(["token": "inv_x"])
            case ("POST", "/invites/inv_x/redeem"): return .json(["orgId": "o1"])
            case ("POST", "/consent/requests"):
                return .json([
                    "requestId": "r1", "url": "https://node.test/consent/r1", "expiresAt": 1, "code": "GBS-YKH",
                ])
            case ("GET", "/consent/r1/result"):
                return .json(["requestId": "r1", "kind": "upgrade", "status": "approved", "decidedAt": 2])
            case ("POST", "/apps"): return .json(["appId": "a1", "contextId": "c"])
            default: return .json([:])
            }
        }
        node.install()
        let cloud = client()
        let org = try await cloud.orgs.create(name: "Acme").orgId
        XCTAssertEqual(org, "o1")
        let team = try await cloud.orgs.teamCreate(org: "o1", name: "General", join: .everyone).teamId
        XCTAssertEqual(team, "t1")
        let app = try await cloud.answerJSON {
            try await cloud.apps.create(
                name: "chat", package: "com.calimero.chat", org: "o1", access: AppAccess.team("t1").generated)
        }["appId"]
        XCTAssertEqual(app, "a1")
        let created = try XCTUnwrap(node.requests.last?.json)
        XCTAssertEqual(created["package"], "com.calimero.chat")
        XCTAssertEqual(created["access"], ["kind": "team", "team": "t1"])
        let token = try await cloud.orgs.inviteCreate(org: "o1", role: .member, passkey: SoftPasskey()).token
        XCTAssertEqual(token, "inv_x")
        _ = try await cloud.answerJSON { try await cloud.orgs.inviteRedeem(token: "inv_x") }
        let started = try await cloud.openConsentPage(
            upgradeConsent(app: "a1", toVersion: "1.1.0", returnTo: "chat://consent")
        ) { url in
            XCTAssertEqual(url.absoluteString, "https://node.test/consent/r1")
        }
        XCTAssertEqual(started.code, "GBS-YKH", "the confirmation code the app shows before opening the page")
        let consentBody = try XCTUnwrap(node.requests.first { $0.path == "/consent/requests" }?.json)
        XCTAssertEqual(consentBody["kind"], "upgrade")
        XCTAssertEqual(consentBody["subject"]?["toVersion"], "1.1.0")
        let result = try await cloud.waitForConsent(started.requestId)
        XCTAssertEqual(result.status, .approved)
        let back = consentReturn(URL(string: "chat://consent?request_id=r1&result=approved")!)
        XCTAssertEqual(back?.requestId, "r1")
        XCTAssertEqual(back?.approved, true)
    }

    func testTheContractKeepsEveryNumberExactlyAsWritten() async throws {
        let exact = #"{"at":1761650000123456789,"max":18446744073709551615}"#
        let node = FakeCloudNode { request in
            let args = String(decoding: request.body, as: UTF8.self)
            let start = args.range(of: #""args":"#)!.upperBound
            let end =
                args.range(of: #","idempotencyKey""#)?.lowerBound ?? args.range(of: #","consistency""#)!.lowerBound
            return FakeCloudAnswer(
                status: 200, headers: [("content-type", "application/json")],
                chunks: [Data(#"{"index":1,"result":\#(args[start..<end]),"events":[]}"#.utf8)])
        }
        node.install()
        let bound = CloudContextContract(client: client()).bound(to: "a1")
        let sent = #"{ "max": 18446744073709551615, "at": 1761650000123456789 }"#
        let called = try await bound.execute(method: "stamp", argsJSON: Data(sent.utf8))
        XCTAssertEqual(String(decoding: called, as: UTF8.self), exact)
        let viewed = try await bound.query(method: "stamps", argsJSON: Data(sent.utf8))
        XCTAssertEqual(String(decoding: viewed, as: UTF8.self), exact)
    }

    func testCanonicalSpellingMatchesCanonicalJSONBesideTheIntegers() throws {
        let input = #" { "b" : 1, "a" : [1.50, -0, 1e3, "xé\n\/", true, null, {}] } "#
        let literal = String(decoding: try LiteralJSON.canonical(Data(input.utf8)), as: UTF8.self)
        XCTAssertEqual(literal, #"{"a":[1.5,0,1000,"xé\n/",true,null,{}],"b":1}"#)
    }

    // MARK: - core rc.7+ parity (mero-js #317–#323)

    private static let stepUpChallenge: JSONValue = [
        "challengeId": "su1", "challenge": .string(b64url(Data(repeating: 5, count: 32))), "rpId": "localhost",
    ]

    /// A node that asks for a step-up on every route until the body carries `confirm`.
    private static func stepUpNode(_ answer: JSONValue = [:]) -> FakeCloudNode {
        FakeCloudNode { request in
            if request.path == "/auth/passkeys/add/begin" {
                return .json([
                    "challengeId": "add1",
                    "options": [
                        "challenge": .string(b64url(Data(repeating: 1, count: 32))),
                        "rp": ["id": "localhost", "name": "x"],
                        "user": ["id": .string(b64url(Data([4]))), "name": "ada", "displayName": "ada"],
                    ],
                ])
            }
            if request.json?["confirm"] == nil {
                return .error(412, "CONFIRM_REQUIRED", details: stepUpChallenge)
            }
            return .json(answer)
        }
    }

    func testMembershipsCredentialsAndInvitesAreApprovedWithAPasskey() async throws {
        let node = Self.stepUpNode(["token": "inv_1", "credentialId": "cred2", "keyId": "k1", "key": "cal_x"])
        node.install()
        let cloud = client()
        let passkey = SoftPasskey()
        _ = try await cloud.orgs.setMember(org: "o1", account: "acc_2", role: .admin, passkey: passkey)
        _ = try await cloud.orgs.removeMember(org: "o1", account: "acc_2", passkey: passkey)
        _ = try await cloud.orgs.capabilities(org: "o1", account: "acc_2", capabilities: 1, passkey: passkey)
        _ = try await cloud.orgs.teamSetMember(org: "o1", team: "t1", account: "acc_2", role: .lead, passkey: passkey)
        _ = try await cloud.orgs.teamRemoveMember(org: "o1", team: "t1", account: "acc_2", passkey: passkey)
        let token = try await cloud.orgs.inviteCreate(
            org: "o1", role: .member, teams: ["t1"], uses: 2, passkey: passkey)
        XCTAssertEqual(token.token, "inv_1")
        let key = try await cloud.auth.apikeyCreate(label: "ci", readOnly: true, passkey: passkey)
        XCTAssertEqual(key.key, "cal_x")
        _ = try await cloud.auth.passkeyRemove(cred: "cred1", passkey: passkey)
        let added = try await cloud.addPasskey(SoftPasskey(), approver: passkey)
        XCTAssertEqual(added, "cred2")

        // Each route: refused once with 412, then the same body with `confirm`.
        let routes = node.requests.filter { $0.path != "/auth/passkeys/add/begin" }
        XCTAssertEqual(routes.count, 18)
        for pair in stride(from: 0, to: routes.count, by: 2) {
            let (first, second) = (routes[pair], routes[pair + 1])
            XCTAssertEqual(first.path, second.path)
            XCTAssertEqual(first.method, second.method)
            XCTAssertNil(first.json?["confirm"])
            XCTAssertEqual(second.json?["confirm"]?["challengeId"], "su1", second.path)
        }
        XCTAssertEqual(
            Set(routes.map { "\($0.method) \($0.path)" }),
            [
                "POST /orgs/o1/members", "DELETE /orgs/o1/members/acc_2", "POST /orgs/o1/capabilities",
                "POST /orgs/o1/teams/t1/members", "DELETE /orgs/o1/teams/t1/members/acc_2", "POST /orgs/o1/invites",
                "POST /auth/apikeys", "DELETE /auth/passkeys/cred1", "POST /auth/passkeys/add/finish",
            ])
        let invite = try XCTUnwrap(routes.last { $0.path == "/orgs/o1/invites" }?.json)
        XCTAssertEqual(invite["role"], "member")
        XCTAssertEqual(invite["teams"], ["t1"])
        XCTAssertEqual(invite["uses"], 2)
        let finish = try XCTUnwrap(routes.last?.json)
        XCTAssertEqual(finish["challengeId"], "add1", "the new passkey's registration rides with the approval")
        XCTAssertNotNil(finish["registration"])
    }

    func testRedeemingAnInviteNeedsNoApproval() async throws {
        let node = FakeCloudNode { _ in .json(["orgId": "o1"]) }
        node.install()
        let cloud = client()
        let answer = try await cloud.answerJSON { try await cloud.orgs.inviteRedeem(token: "inv_1") }
        XCTAssertEqual(answer["orgId"], "o1")
        XCTAssertEqual(node.requests.count, 1)
    }

    func testAsksTheDirectoryForAClustersNodesAndOrdersThemByZone() async throws {
        let first = FakeCloudNode(baseURL: "https://a.test") { _ in .json(["accountId": "acc"]) }
        let second = FakeCloudNode(baseURL: "https://b.test") { _ in .json(["accountId": "acc"]) }
        let listings = Counter()
        FakeCloudNode.install([first, second]) { request in
            listings.increment()
            XCTAssertEqual(request.url?.path, "/v2/eu-1")
            let body = #"""
                {"cluster":"eu-1","refreshAfterSeconds":30,"nodes":[
                {"id":"1","url":"https://b.test/","zone":"far","weight":1},
                {"id":"2","url":"https://a.test","zone":"near","weight":3},
                {"id":"3","url":"ftp://c.test","zone":"near","weight":1},
                {"id":"4","url":"https://d.test","zone":"near","weight":0}]}
                """#
            return .init(status: 200, headers: ["content-type": "application/json"], body: Data(body.utf8))
        }
        let cloud = try await CalimeroClient.connect(
            CalimeroConnectOptions(
                cluster: "eu-1", policy: .development, directory: "https://dir.test/v2", zone: "near",
                session: MockURLProtocol.makeSession(), random: { 0.99 }))
        let nodes = await cloud.nodes
        XCTAssertEqual(nodes, ["https://a.test", "https://b.test"], "zone first; bad URLs and weights dropped")
        let me = AccountInfo(json: try await cloud.answerJSON { try await cloud.auth.me() })
        XCTAssertEqual(me.accountId, "acc")
        XCTAssertEqual(first.requests.count, 1)
        XCTAssertEqual(listings.count, 1)
    }

    func testDefaultsToTheCalimeroCloudDirectory() {
        XCTAssertEqual(defaultDirectory, "https://manager.cloud.calimero.network/api/directory")
    }

    func testTheDirectorysFailuresAreDirectoryErrors() async throws {
        MockURLProtocol.setHandler { request in
            if request.url?.path.hasSuffix("/gone") == true { return .init(status: 404, headers: [:], body: Data()) }
            if request.url?.path.hasSuffix("/empty") == true {
                return .init(
                    status: 200, headers: [:], body: Data(#"{"nodes":[{"url":"https://x.test","weight":0}]}"#.utf8))
            }
            return .init(status: 200, headers: [:], body: Data("<html>".utf8))
        }
        for (cluster, reason) in [("gone", DirectoryFailure.notFound), ("empty", .empty), ("html", .badResponse)] {
            do {
                _ = try await readDirectory(
                    "https://dir.test", cluster: cluster, session: MockURLProtocol.makeSession())
                XCTFail("expected a DirectoryError for \(cluster)")
            } catch let error as DirectoryError {
                XCTAssertEqual(error.reason, reason)
            }
        }
    }

    func testWeightedOrderFollowsTheRoll() {
        let nodes = [
            DirectoryNode(url: "https://a", zone: "z1", weight: 1),
            DirectoryNode(url: "https://b", zone: "z2", weight: 3),
        ]
        XCTAssertEqual(orderNodes(nodes, zone: nil, random: { 0 }), ["https://a", "https://b"])
        XCTAssertEqual(orderNodes(nodes, zone: nil, random: { 0.5 }), ["https://b", "https://a"])
        XCTAssertEqual(orderNodes(nodes, zone: "z2", random: { 0 }), ["https://b", "https://a"])
    }

    func testACallFailsOverToTheNextNodeWithTheSameKeyOnceItsRetriesRunOut() async throws {
        let down = FakeCloudNode(baseURL: "https://n1.test") { _ in .error(503, "UNAVAILABLE") }
        let up = FakeCloudNode(baseURL: "https://n2.test") { _ in .json(["index": 3, "result": "ok"]) }
        FakeCloudNode.install([down, up])
        let cloud = client("https://n1.test", nodes: ["https://n2.test"])
        let answer = try await cloud.call("a1", "set", ["k": 1])
        XCTAssertEqual(answer.result, "ok")
        XCTAssertEqual(down.requests.count, 4, "the first attempt and 3 retries on the first node")
        XCTAssertEqual(up.requests.count, 1)
        let keys = (down.requests + up.requests).compactMap { $0.json?["idempotencyKey"]?.stringValue }
        XCTAssertEqual(Set(keys).count, 1, "the cluster answers the key once, whichever node it reaches")
        let now = await cloud.nodeUrl
        XCTAssertEqual(now, "https://n2.test")
    }

    func testRefreshMovesThroughTheNodesOnWrongNode() async throws {
        let wrong = FakeCloudNode(baseURL: "https://n1.test") { _ in .error(421, "WRONG_NODE") }
        let right = FakeCloudNode(baseURL: "https://n2.test") { _ in
            .json(["accountId": "a", "sessionToken": "v2.new", "expiresAt": 1, "refreshToken": "rt_new"])
        }
        FakeCloudNode.install([wrong, right])
        let cloud = client("https://n1.test", nodes: ["https://n2.test"])
        await cloud.useSession(
            CalimeroSession(accountId: "a", sessionToken: "v2.x", expiresAt: 0, refreshToken: "rt_x"))
        let session = try await cloud.refresh()
        XCTAssertEqual(session.refreshToken, "rt_new")
        XCTAssertEqual(right.requests.first?.json?["refreshToken"], "rt_x", "the same token, not yet spent")
    }

    func testBlobsCarryTheirContentTypeAndAGrantedDownloadNeedsNoSession() async throws {
        let node = FakeCloudNode { request in
            if request.method == "POST" {
                return .json(["blobId": "B1", "id": "aa", "size": .number(Double(request.body.count))])
            }
            return FakeCloudAnswer(status: 200, headers: [("content-type", "image/png")], chunks: [Data([1, 2, 3])])
        }
        node.install()
        let cloud = client()
        // core documents the upload as `application/octet-stream` beside `*/*` and stores the
        // type the request names: the caller's `contentType`, else `application/octet-stream`.
        let uploaded = try await cloud.answer(as: BlobInfo.self) {
            try await cloud.apps.blobUpload(app: "a1", body: Data([1, 2, 3]), contentType: "image/png")
        }
        XCTAssertEqual(uploaded.size, 3)
        XCTAssertEqual(node.requests[0].headers["content-type"], "image/png")
        _ = try await cloud.answerJSON { try await cloud.apps.blobUpload(app: "a1", body: Data([4])) }
        XCTAssertEqual(node.requests[1].headers["content-type"], "application/octet-stream")
        _ = try await cloud.answerJSON {
            try await cloud.apps.blobUpload(app: "a1", body: Data([5]), contentType: nil)
        }
        XCTAssertEqual(node.requests[2].headers["content-type"], "application/octet-stream")
        let content = try await cloud.blobContent("a1", "B1")
        XCTAssertEqual(content, BlobContent(data: Data([1, 2, 3]), contentType: "image/png"))
        let granted = try await cloud.apps.blobDownload(app: "a1", id: "B1", grant: "g+/=1")
        XCTAssertEqual(granted, Data([1, 2, 3]))
        XCTAssertEqual(node.requests.last?.path, "/apps/a1/blobs/B1?grant=g%2B%2F%3D1")
        XCTAssertNil(node.requests.last?.bearer)
    }

    func testXcallsDeploymentsAndDeployers() async throws {
        let node = FakeCloudNode { request in
            switch (request.method, request.path) {
            case ("GET", "/apps/a1/xcalls"):
                return .json([
                    "pending": 2,
                    "deadLetters": [
                        ["seq": 1, "app": "a2", "method": "m", "reason": "trap", "atMs": 1_700_000_000_000]
                    ],
                ])
            case ("GET", "/deployments"):
                return .json([
                    "deployments": [
                        [
                            "id": "dep1", "name": "Shop", "owner": ["org": "o1"], "createdBy": "acc",
                            "createdAt": "2026-01-01T00:00:00Z",
                            "apps": [
                                [
                                    "app": "shop", "package": "com.x", "version": "1.0.0", "addedBy": "acc",
                                    "addedAt": "2026-01-01T00:00:00Z",
                                ]
                            ],
                        ]
                    ]
                ])
            case ("GET", "/deployments/dep1"):
                return .json([
                    "id": "dep1", "name": "Shop", "owner": ["kind": "org", "name": "Acme"],
                    "apps": [["app": "shop", "package": "com.x", "version": "1.0.0"]],
                ])
            case ("GET", "/deployments/dep1/orgs"):
                return .json([
                    "deployment": "dep1", "outsideOrgs": 1,
                    "orgs": [
                        [
                            "orgId": "o2", "name": .null, "instances": 3, "firstCreatedAt": "2026-01-02T00:00:00Z",
                            "apps": [["app": "shop", "version": "1.0.0"]],
                        ]
                    ],
                ])
            case ("GET", "/deployers"):
                return .json([
                    "deployments": "granted", "deployers": "owner", "owner": ["account": "acc"],
                    "grants": [
                        [
                            "key": "org:o1", "deployer": ["org": "o1"], "until": .null, "untilSecs": .null,
                            "grantedAt": "2026-01-01T00:00:00Z", "grantedBy": "acc",
                        ]
                    ],
                ])
            case ("POST", "/apps"): return .json(["appId": "a9"])
            default:
                if request.json?["confirm"] == nil, request.method != "GET" {
                    return .error(412, "CONFIRM_REQUIRED", details: Self.stepUpChallenge)
                }
                return .json(["id": "dep1"])
            }
        }
        node.install()
        let cloud = client()
        let passkey = SoftPasskey()
        let xcalls = try await cloud.answer(as: AppXcalls.self) { try await cloud.apps.xcalls(app: "a1") }
        XCTAssertEqual(xcalls.pending, 2)
        XCTAssertEqual(xcalls.deadLetters.first?.app, "a2")

        let created = try await cloud.answerJSON {
            try await cloud.deployments.create(name: "Shop", owner: Deployer.org("o1").generated, passkey: passkey)
        }
        let id = try XCTUnwrap(created["id"]?.stringValue)
        XCTAssertEqual(id, "dep1")
        _ = try await cloud.answerJSON {
            try await cloud.deployments.offer(
                id: id, app: "shop", package: "com.x", version: "1.0.0", passkey: passkey)
        }
        _ = try await cloud.answerJSON {
            try await cloud.deployments.withdraw(id: id, app: "shop", passkey: passkey)
        }
        _ = try await cloud.answerJSON { try await cloud.deployments.delete(id: id, passkey: passkey) }
        _ = try await cloud.answerJSON {
            try await cloud.deployments.deployerGrant(
                deployer: Deployer.account("acc_2").generated, until: "1970-01-01T00:00:00Z", passkey: passkey)
        }
        _ = try await cloud.answerJSON {
            try await cloud.deployments.deployerRevoke(key: Deployer.org("o1").key, passkey: passkey)
        }
        let confirmed = node.requests.filter { $0.json?["confirm"] != nil }.map { "\($0.method) \($0.path)" }
        XCTAssertEqual(
            confirmed,
            [
                "POST /deployments", "POST /deployments/dep1/apps", "DELETE /deployments/dep1/apps/shop",
                "DELETE /deployments/dep1", "POST /deployers",
                // The generated client percent-encodes a path parameter's `:` (the node decodes it).
                "DELETE /deployers/org%3Ao1",
            ])
        let create = try XCTUnwrap(node.requests.last { $0.path == "/deployments" }?.json)
        XCTAssertEqual(create["owner"], ["org": "o1"])
        let grant = try XCTUnwrap(node.requests.last { $0.path == "/deployers" }?.json)
        XCTAssertEqual(grant["deployer"], ["account": "acc_2"])
        XCTAssertEqual(grant["until"], "1970-01-01T00:00:00Z")

        let mine = try await cloud.answer(as: DeploymentListing.self) { try await cloud.deployments.list() }
            .deployments
        XCTAssertEqual(mine.first?.owner, .org("o1"))
        let shown = try await cloud.answer(as: PublicDeployment.self) { try await cloud.deployments.get(id: "dep1") }
        XCTAssertEqual(shown.owner.name, "Acme")
        let orgs = try await cloud.answer(as: DeploymentOrgs.self) { try await cloud.deployments.orgs(id: "dep1") }
        XCTAssertEqual(orgs.orgs.first?.instances, 3)
        XCTAssertEqual(orgs.outsideOrgs, 1)
        let deployers = try await cloud.answer(as: DeployersInfo.self) { try await cloud.deployments.deployerList() }
        XCTAssertEqual(deployers.deployments, .granted)
        XCTAssertEqual(deployers.grants.first?.deployer, .org("o1"))
        XCTAssertEqual(deployerKey(.account("a")), "account:a")

        _ = try await cloud.answerJSON { try await cloud.apps.create(package: "com.x", deployment: "dep1") }
        XCTAssertEqual(node.requests.last?.json?["deployment"], "dep1")
        XCTAssertEqual(pageDeployment(URL(string: "https://x.test/1.0.0/?deployment=dep1")), "dep1")
        XCTAssertNil(pageDeployment(URL(string: "https://x.test/1.0.0/")))
    }

    func testTheConfirmationCodeIsCoresAndAMismatchIsRefused() async throws {
        XCTAssertEqual(confirmationCode("1.AAAAAAAAAAAAAAAAAAAAAA"), "R02-9P2")
        XCTAssertEqual(confirmationCode("2.q7Xb3kR0fLzM8nW1yVt5cA"), "54E-429")
        let node = FakeCloudNode { _ in
            .json(["requestId": "r1", "url": "https://node.test/consent/r1", "expiresAt": 1, "code": "AAA-AAA"])
        }
        node.install()
        let opened = Counter()
        do {
            _ = try await client().openConsentPage(
                releaseConsent(.account, version: "2.3.110", returnTo: "x://c")
            ) { _ in opened.increment() }
            XCTFail("expected CONSENT_CODE_MISMATCH")
        } catch let error as CalimeroError {
            XCTAssertEqual(error.code, "CONSENT_CODE_MISMATCH")
            XCTAssertEqual(error.details["expected"], "GBS-YKH")
        }
        XCTAssertEqual(opened.count, 0, "the page is not opened")
    }

    func testADeclinedConsentConfirmationDoesNotOpenThePage() async throws {
        FakeCloudNode { _ in
            .json(["requestId": "r1", "url": "https://node.test/consent/r1", "expiresAt": 1, "code": "GBS-YKH"])
        }.install()
        let opened = Counter()
        let shown = Box<String?>(nil)
        let started = try await client().openConsentPage(
            releaseConsent(.account, version: "2.3.110", returnTo: "x://c"),
            options: OpenConsentOptions(
                navigate: { _ in opened.increment() },
                confirm: { started in
                    shown.update { $0 = started.code }
                    return false
                }))
        XCTAssertEqual(started.requestId, "r1")
        XCTAssertEqual(shown.get, "GBS-YKH")
        XCTAssertEqual(opened.count, 0)
    }
}

/// `GET /deployments`' answer, read as ``DeploymentInfo``s.
private struct DeploymentListing: Decodable {
    let deployments: [DeploymentInfo]
}
