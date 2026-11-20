import Foundation
import MeroAPI
import MeroKit
import XCTest

/// The node API end to end against a `merod local` cluster (mock
/// attestation): sessions and passkeys, step-ups, organizations, teams, apps,
/// events, presence, blobs and packages. The Swift side of mero-js's
/// `tests/cluster/client.test.ts`.
///
/// Skipped unless `MERO_E2E_NODE_URL` names the cluster's first node.
/// `MERO_E2E_NODES` (comma-separated) names the others, for the cases that
/// sign in on a second node; without it they use the first node only.
/// `MERO_E2E_BUNDLE` is a kv-store `.wasm` (core's `apps/kv-store`, as
/// `kv-store-test-fixture.mpk` carries it) for the cases that run an app, and
/// `MERO_E2E_MPK` a signed kv-store `.mpk` for the package cases.
final class ClusterE2ETests: XCTestCase {
    private static let env = ProcessInfo.processInfo.environment

    private var urls: [String] {
        get throws {
            guard let first = Self.env["MERO_E2E_NODE_URL"], !first.isEmpty else {
                throw XCTSkip("MERO_E2E_NODE_URL is not set")
            }
            let others = (Self.env["MERO_E2E_NODES"] ?? "").split(separator: ",").map(String.init).filter {
                !$0.isEmpty
            }
            return [first] + others
        }
    }

    private func client(_ i: Int, allowMock: Bool = true) throws -> CalimeroClient {
        let urls = try urls
        return CalimeroClient(
            CalimeroClientOptions(baseUrl: urls[i % urls.count], policy: CloudPolicy(allowMock: allowMock)))
    }

    /// A new account on node `i`, its client and its passkey.
    private func person(_ i: Int, _ name: String) async throws -> (CalimeroClient, SoftPasskey) {
        let c = try client(i)
        let passkey = SoftPasskey()
        try await c.signUp(passkey, name: name)
        return (c, passkey)
    }

    private func me(_ c: CalimeroClient) async throws -> JSONValue {
        try JSONValue(encoding: try await c.auth.me())
    }

    /// The kv-store bundle with a custom section of its own, so its hash is
    /// this suite's: a raw upload's hash names a package release's WASM only
    /// for whoever may see that release, so the fixture package's own bytes,
    /// once installed, are not a bundle another account may name.
    private func bundle() throws -> Data {
        guard let path = Self.env["MERO_E2E_BUNDLE"], !path.isEmpty else {
            throw XCTSkip("MERO_E2E_BUNDLE is not set")
        }
        var wasm = try Data(contentsOf: URL(fileURLWithPath: path))
        let name = Data("mero-swift-e2e".utf8)
        let payload = Self.leb128(name.count) + name + Data("raw".utf8)
        wasm.append(0)
        wasm.append(Self.leb128(payload.count))
        wasm.append(payload)
        return wasm
    }

    private static func leb128(_ value: Int) -> Data {
        var out = Data()
        var rest = value
        repeat {
            var byte = UInt8(rest & 0x7f)
            rest >>= 7
            if rest != 0 { byte |= 0x80 }
            out.append(byte)
        } while rest != 0
        return out
    }

    private func kvApp(_ c: CalimeroClient, _ name: String, org: String? = nil) async throws -> String {
        let hash = try await c.packages.bundlePut(body: try bundle()).hash
        return try await c.apps.create(name: name, bundle: hash, org: org).appId
    }

    /// `body`'s error, failing when it succeeds.
    private func refusal(
        _ body: () async throws -> Void, file: StaticString = #filePath, line: UInt = #line
    ) async -> Error? {
        do {
            try await body()
            XCTFail("expected a refusal", file: file, line: line)
            return nil
        } catch {
            return error
        }
    }

    private func pause(_ seconds: Double = 0.3) async {
        try? await Task.sleep(nanoseconds: UInt64(seconds * 1e9))
    }

    // MARK: - Trust and sessions

    func testRefusesAMockQuoteUnlessAllowed() async throws {
        let c = try client(0, allowMock: false)
        let error = await refusal { _ = try await c.auth.loginBegin() }
        XCTAssertTrue("\(error.map { "\($0)" } ?? "")".contains("mock"), "\(String(describing: error))")
    }

    func testSignsUpSignsInElsewhereRefreshesAndSignsOutEverywhere() async throws {
        let (alice, passkey) = try await person(0, "alice-session")
        let mine = try await me(alice)
        XCTAssertEqual(mine["via"]?.stringValue, "session")
        let laptop = try client(1)
        let session = try await laptop.signIn(passkey)
        XCTAssertEqual(session.accountId, mine["accountId"]?.stringValue)

        // A refresh answers a session for the same account. A young refresh
        // token is kept (core rc.30, `refresh_rotate_after`), an old one rotated.
        let current = await alice.session
        let before = try XCTUnwrap(current)
        let fresh = try await alice.refresh()
        XCTAssertEqual(fresh.accountId, before.accountId)
        XCTAssertGreaterThanOrEqual(fresh.expiresAt, before.expiresAt)
        let still = try await me(alice)
        XCTAssertEqual(still["accountId"], mine["accountId"])

        try await alice.signOutEverywhere()
        let error = await refusal { _ = try await laptop.auth.me() }
        XCTAssertEqual((error as? CalimeroError)?.status, 401, "\(String(describing: error))")
    }

    func testAPasskeyNoAccountHasIsUnknownHere() async throws {
        let c = try client(0)
        let error = await refusal { _ = try await c.signIn(SoftPasskey()) }
        XCTAssertTrue(error is PasskeyUnknownHere, "\(String(describing: error))")
    }

    func testListsSessionsAndEndsOne() async throws {
        let (laptop, passkey) = try await person(0, "alice-sessions")
        let mine = try await me(laptop)
        let phone = try client(1)
        try await phone.signIn(passkey)

        let sessions = try JSONValue(encoding: try await laptop.auth.sessionsList())["sessions"]?.arrayValue ?? []
        XCTAssertEqual(sessions.count, 2)
        let own = sessions.first { $0["current"]?.boolValue == true }
        let other = sessions.first { $0["current"]?.boolValue != true }
        XCTAssertNotNil(own)
        for s in sessions { XCTAssertEqual(s["passkey"]?.stringValue, mine["passkeyId"]?.stringValue) }

        let otherId = try XCTUnwrap(other?["session"]?.stringValue)
        _ = try await laptop.auth.sessionRevoke(session: otherId)
        let ended = await refusal { _ = try await phone.auth.me() }
        XCTAssertTrue(ended is LoginRequired, "\(String(describing: ended))")
        let again = await refusal { _ = try await laptop.auth.sessionRevoke(session: otherId) }
        XCTAssertTrue(again is NotFound, "\(String(describing: again))")
    }

    func testListsTheAccountsActivity() async throws {
        let (laptop, passkey) = try await person(0, "alice-activity")
        let mine = try await me(laptop)
        var activity = try JSONValue(encoding: try await laptop.auth.activityList())["activity"]?.arrayValue ?? []
        XCTAssertEqual(activity.map { $0["kind"]?.stringValue }, ["signIn"])
        _ = try await laptop.auth.apikeyCreate(label: "ci", passkey: passkey)
        activity = try JSONValue(encoding: try await laptop.auth.activityList())["activity"]?.arrayValue ?? []
        XCTAssertEqual(activity.map { $0["kind"]?.stringValue }, ["approval", "signIn"])
        XCTAssertEqual(activity.first?["method"]?.stringValue, "__create_api_key")
        XCTAssertEqual(activity.first?["passkey"]?.stringValue, mine["passkeyId"]?.stringValue)
    }

    func testStartsAnAccountRecoveryWithACodeAndTheOldPasskeyCancelsIt() async throws {
        let (alice, passkey) = try await person(0, "alice-recovery")
        let made = try JSONValue(encoding: try await alice.auth.recoveryCreate(passkey: passkey))
        let code = try XCTUnwrap(made["code"]?.stringValue, "\(made)")

        let lost = try client(1)
        let wrong = await refusal {
            _ = try await lost.startAccountRecovery(code: "not-a-code", passkey: SoftPasskey())
        }
        XCTAssertTrue(wrong is WrongRecoveryCode, "\(String(describing: wrong))")
        let fresh = SoftPasskey()
        let recovery = try await lost.startAccountRecovery(code: code, passkey: fresh)
        let aliceMe = try await me(alice)
        XCTAssertEqual(recovery.accountId, aliceMe["accountId"]?.stringValue)
        XCTAssertGreaterThan(recovery.readyAt, recovery.startedAt)
        let early = await refusal { _ = try await lost.finishAccountRecovery(fresh, accountId: recovery.accountId) }
        XCTAssertTrue(early is RecoveryNotReady, "\(String(describing: early))")

        _ = try await alice.auth.recoveryCancel(passkey: passkey)
        let cancelled = await refusal { _ = try await lost.finishAccountRecovery(fresh, accountId: recovery.accountId) }
        XCTAssertTrue(cancelled is CalimeroError, "\(String(describing: cancelled))")
    }

    // MARK: - Passkeys, API keys and step-ups

    func testAddsAndRemovesAPasskeyWithTheOldOnesApproval() async throws {
        let (alice, passkey) = try await person(0, "alice-passkeys")
        let second = SoftPasskey()
        // The new passkey cannot approve its own addition.
        let own = await refusal { _ = try await alice.addPasskey(second, approver: second) }
        XCTAssertTrue(own is CalimeroError, "\(String(describing: own))")
        let cred = try await alice.addPasskey(second, approver: passkey)
        let viaSecond = try client(2)
        try await viaSecond.signIn(second)
        let aliceId = try await me(alice)["accountId"]
        let secondId = try await me(viaSecond)["accountId"]
        XCTAssertEqual(secondId, aliceId)

        let asked = await refusal { _ = try await alice.auth.passkeyRemove(cred: cred) }
        XCTAssertTrue(asked is ConfirmRequired, "\(String(describing: asked))")
        _ = try await alice.auth.passkeyRemove(cred: cred, passkey: passkey)
        let gone = await refusal { _ = try await self.client(2).signIn(second) }
        XCTAssertTrue(gone is CalimeroError, "\(String(describing: gone))")
    }

    func testAPIKeysNeedAStepUpAndAReadOnlyKeyNeverWrites() async throws {
        let (alice, passkey) = try await person(0, "alice-keys")
        let asked = await refusal { _ = try await alice.auth.apikeyCreate(label: "ci") }
        let required = try XCTUnwrap(asked as? ConfirmRequired, "\(String(describing: asked))")
        XCTAssertNotNil(required.details["challenge"] ?? required.details["challengeId"])
        let created = try JSONValue(encoding: try await alice.auth.apikeyCreate(label: "ci", passkey: passkey))
        let keyId = try XCTUnwrap(created["keyId"]?.stringValue)
        var keys = try JSONValue(encoding: try await alice.auth.apikeyList())["keys"]?.arrayValue ?? []
        XCTAssertTrue(keys.contains { $0["keyId"]?.stringValue == keyId })
        _ = try await alice.auth.apikeyRevoke(key: keyId, passkey: passkey)
        keys = try JSONValue(encoding: try await alice.auth.apikeyList())["keys"]?.arrayValue ?? []
        XCTAssertFalse(keys.contains { $0["keyId"]?.stringValue == keyId })

        // The client-wide step-up answers a 412 for a request that names no passkey.
        await alice.useStepUp(passkey)
        let app = try await kvApp(alice, "keys-kv")
        let readOnly = try JSONValue(
            encoding: try await alice.auth.apikeyCreate(apps: [app], readOnly: true))
        await alice.useStepUp(nil)
        let agent = try client(1)
        await agent.useApiKey(try XCTUnwrap(readOnly["key"]?.stringValue))
        _ = try await agent.view(app, "len")
        let denied = await refusal { _ = try await agent.call(app, "set", ["key": "x", "value": "y"]) }
        XCTAssertEqual((denied as? CalimeroError)?.code, "FORBIDDEN", "\(String(describing: denied))")
    }

    // MARK: - Apps

    func testCallsViewsAndReplaysByIdempotencyKey() async throws {
        let (alice, passkey) = try await person(0, "alice-kv")
        let app = try await kvApp(alice, "kv")
        let first = try await alice.call(app, "set", ["key": "a", "value": "1"], idempotencyKey: "k1")
        XCTAssertGreaterThan(first.index, 0)
        let replay = try await alice.call(app, "set", ["key": "a", "value": "other"], idempotencyKey: "k1")
        XCTAssertEqual(replay.index, first.index)

        // The same account on another node reads it.
        let laptop = try client(1)
        try await laptop.signIn(passkey)
        let strong = try await laptop.view(app, "get", ["key": "a"])
        XCTAssertEqual(strong.result, "1")
        let stale = try await laptop.view(app, "get", ["key": "a"], stale: true, minIndex: first.index)
        XCTAssertEqual(stale.result, "1")

        // Another account cannot reach an owner-only app.
        let (bob, _) = try await person(2, "bob-kv")
        let hidden = await refusal { _ = try await bob.call(app, "set", ["key": "x", "value": "y"]) }
        XCTAssertTrue(hidden is NotFound, "\(String(describing: hidden))")
    }

    func testEventsReplayAndResumeAfterAnEventId() async throws {
        let (alice, _) = try await person(0, "alice-events")
        let app = try await kvApp(alice, "events-kv")
        for value in ["1", "2", "3"] { _ = try await alice.call(app, "set", ["key": "k", "value": .string(value)]) }

        var seen: [CloudEvent] = []
        for try await event in alice.events(app) {
            seen.append(event)
            if seen.count == 3 { break }
        }
        XCTAssertEqual(seen.count, 3)
        for event in seen {
            XCTAssertNotNil(event.index, "id \(event.id) is <index>:<seq>")
            XCTAssertNotNil(event.seq)
        }

        // Resuming after the second event's id answers the third, and nothing before it.
        var resumed: [CloudEvent] = []
        for try await event in alice.events(app, after: seen[1].id) {
            resumed.append(event)
            break
        }
        XCTAssertEqual(resumed.first?.id, seen[2].id)
    }

    func testPresenceAndBlobs() async throws {
        let (alice, _) = try await person(0, "alice-presence")
        let app = try await kvApp(alice, "presence-kv")
        _ = try await alice.apps.presencePublish(app: app, seq: 1, state: presenceState(Data([1, 2, 3])))
        let present = try JSONValue(encoding: try await alice.apps.presenceGet(app: app))["present"]?.arrayValue ?? []
        XCTAssertTrue(present.contains { $0["state"]?.stringValue == "010203" }, "\(present)")
        for try await event in alice.presenceStream(app) {
            XCTAssertEqual(event.kind, "snapshot")
            break
        }
        _ = try await alice.apps.presencePublish(app: app, seq: 2, state: .some(presenceState(nil)))

        let data = Data("hello blob".utf8)
        let up = try JSONValue(encoding: try await alice.apps.blobUpload(app: app, body: data))
        let blobId = try XCTUnwrap(up["blobId"]?.stringValue ?? up["id"]?.stringValue)
        let content = try await alice.blobContent(app, blobId)
        XCTAssertEqual(content.data, data)
        let blobs = try await alice.listBlobs(app)
        XCTAssertTrue(blobs.contains { $0.id == up["id"]?.stringValue })
        _ = try await alice.apps.blobDelete(app: app, id: blobId)
        let gone = await refusal { _ = try await alice.blobInfo(app, blobId) }
        XCTAssertEqual((gone as? CalimeroError)?.status, 404, "\(String(describing: gone))")
    }

    func testAliasesAccessAndTransfer() async throws {
        let (alice, passkey) = try await person(0, "alice-access")
        let (bob, _) = try await person(1, "bob-access")
        let bobMe = try await me(bob)
        let bobId = try XCTUnwrap(bobMe["accountId"]?.stringValue)
        // Owner-only, in an organization made with it: a named account reaches it only as a
        // member of the organization, and an app goes only to one, so bob joins it; joining
        // alone still leaves him out.
        let hash = try await alice.packages.bundlePut(body: try bundle()).hash
        let app = try await alice.apps.create(
            name: "shared", bundle: hash, grants: []
        ).appId
        let made = try JSONValue(encoding: try await alice.apps.get(app: app))
        let shared = try XCTUnwrap(made["org"]?.stringValue, "\(made)")
        let token = try await alice.orgs.inviteCreate(org: shared, role: .member, passkey: passkey).token
        _ = try await bob.orgs.inviteRedeem(token: token)
        _ = try await alice.auth.aliasSet(alias: "notes", app: app)
        let alias = try JSONValue(encoding: try await alice.auth.aliasGet(alias: "notes"))
        XCTAssertEqual(alias["appId"]?.stringValue, app)
        _ = try await alice.auth.aliasDelete(alias: "notes")

        let hidden = await refusal { _ = try await bob.call(app, "set", ["key": "x", "value": "y"]) }
        XCTAssertTrue(hidden is NotFound, "\(String(describing: hidden))")
        _ = try await alice.apps.access(
            app: app, grants: [.account(.init(kind: .account, account: bobId))])
        await pause()
        _ = try await bob.call(app, "set", ["key": "x", "value": "y"])

        let asked = await refusal { _ = try await alice.apps.transfer(app: app, to: bobId) }
        XCTAssertTrue(asked is ConfirmRequired, "\(String(describing: asked))")
        _ = try await alice.apps.transfer(app: app, to: bobId, passkey: passkey)
        let transferred = try JSONValue(encoding: try await bob.apps.get(app: app))
        XCTAssertEqual(transferred["owner"]?.stringValue, bobId)
    }

    // MARK: - Organizations and teams

    func testOrganizationsGateAppsAndRemovalCutsAccessOff() async throws {
        let (alice, passkey) = try await person(0, "alice-org")
        let (bob, _) = try await person(1, "bob-org")
        let bobMe = try await me(bob)
        let bobId = try XCTUnwrap(bobMe["accountId"]?.stringValue)
        let org = try await alice.orgs.create(name: "Acme").orgId
        let team = try await alice.orgs.teamCreate(org: org, name: "eng", join: .invite).teamId
        let token = try await alice.orgs.inviteCreate(org: org, role: .member, teams: [team], passkey: passkey).token
        _ = try await bob.orgs.inviteRedeem(token: token)

        let app = try await kvApp(alice, "team-kv", org: org)
        _ = try await bob.call(app, "set", ["key": "from", "value": "bob"])
        // The org's app teams (one per package; a raw bundle has none) decode.
        let appTeams = try JSONValue(encoding: try await alice.orgs.appTeamList(org: org))
        XCTAssertNotNil(appTeams["appTeams"]?.arrayValue, "\(appTeams)")

        let asked = await refusal { _ = try await alice.orgs.removeMember(org: org, account: bobId) }
        XCTAssertTrue(asked is ConfirmRequired, "\(String(describing: asked))")
        _ = try await alice.orgs.removeMember(org: org, account: bobId, passkey: passkey)
        await pause()
        let cut = await refusal { _ = try await bob.call(app, "set", ["key": "x", "value": "y"]) }
        XCTAssertTrue(cut is NotFound, "\(String(describing: cut))")
    }

    func testTeamsMembersAndATeamDeleteThatAsksForThePasskey() async throws {
        let (alice, passkey) = try await person(0, "alice-teams")
        let (bob, _) = try await person(1, "bob-teams")
        let bobMe = try await me(bob)
        let bobId = try XCTUnwrap(bobMe["accountId"]?.stringValue)
        let org = try await alice.orgs.create(name: "Initech").orgId
        _ = try await alice.orgs.setMember(org: org, account: bobId, role: .member, passkey: passkey)
        let team = try await alice.orgs.teamCreate(org: org, name: "ops", join: .invite).teamId
        _ = try await alice.orgs.teamSetMember(org: org, team: team, account: bobId, role: .readOnly, passkey: passkey)
        try await alice.updateTeam(org, team, name: "platform", metadata: ["colour": "blue"])
        let renamed = try JSONValue(encoding: try await alice.orgs.teamGet(org: org, team: team))
        XCTAssertEqual(renamed["name"]?.stringValue, "platform")
        _ = try await alice.orgs.capabilities(org: org, account: bobId, capabilities: 0, passkey: passkey)
        _ = try await alice.orgs.teamRemoveMember(org: org, team: team, account: bobId, passkey: passkey)
        _ = try await alice.orgs.teamDelete(org: org, team: team)

        // A team whose sub-team holds an app goes with the passkey; the ask names the app.
        let eng = try await alice.orgs.teamCreate(org: org, name: "eng", join: .invite).teamId
        let backend = try await alice.orgs.teamCreate(org: org, name: "backend", parent: eng, join: .invite).teamId
        let hash = try await alice.packages.bundlePut(body: try bundle()).hash
        let teamApp = try await alice.apps.create(
            name: "eng-kv", bundle: hash, org: org, grants: [.team(.init(kind: .team, team: backend))]
        ).appId
        let asked = await refusal { _ = try await alice.orgs.teamDelete(org: org, team: eng) }
        let required = try XCTUnwrap(asked as? ConfirmRequired, "\(String(describing: asked))")
        XCTAssertEqual(required.details["apps"]?.arrayValue?.compactMap(\.stringValue), [teamApp])
        _ = try await alice.orgs.teamDelete(org: org, team: eng, passkey: passkey)
        await pause()
        let gone = await refusal { _ = try await alice.apps.get(app: teamApp) }
        XCTAssertEqual((gone as? CalimeroError)?.status, 404, "\(String(describing: gone))")
    }

    func testShowsAdminsTheirMembersLockoutRisk() async throws {
        let (alice, passkey) = try await person(0, "alice-security")
        let (bob, _) = try await person(1, "bob-security")
        let org = try await alice.orgs.create(name: "Lockout Co").orgId
        let token = try await alice.orgs.inviteCreate(org: org, role: .member, passkey: passkey).token
        _ = try await bob.orgs.inviteRedeem(token: token)
        let bobMe = try await me(bob)
        let bobId = try XCTUnwrap(bobMe["accountId"]?.stringValue)
        let members =
            try JSONValue(encoding: try await alice.orgs.members(org: org, detail: .security))["members"]?.arrayValue
            ?? []
        XCTAssertEqual(members.count, 2)
        let b = try XCTUnwrap(members.first { $0["account"]?.stringValue == bobId })
        XCTAssertEqual(b["role"]?.stringValue, "member")
        XCTAssertEqual(b["passkeys"]?.doubleValue, 1)
        let refused = await refusal { _ = try await bob.orgs.members(org: org, detail: .security) }
        XCTAssertEqual((refused as? CalimeroError)?.status, 403, "\(String(describing: refused))")
    }

    func testDeletesAnOrgWithTheOwnersPasskey() async throws {
        let (alice, passkey) = try await person(0, "alice-doomed")
        let org = try await alice.orgs.create(name: "doomed").orgId
        let asked = await refusal { _ = try await alice.orgs.delete(org: org) }
        XCTAssertTrue(asked is ConfirmRequired, "\(String(describing: asked))")
        _ = try await alice.orgs.delete(org: org, passkey: passkey)
        let gone = await refusal { _ = try await alice.orgs.get(org: org) }
        XCTAssertEqual((gone as? CalimeroError)?.status, 404, "\(String(describing: gone))")
    }

    func testMovesAnAppWithItsPeopleIntoAnotherOrganization() async throws {
        let (alice, passkey) = try await person(0, "alice-move")
        let (bob, _) = try await person(1, "bob-move")
        let bobMe = try await me(bob)
        let bobId = try XCTUnwrap(bobMe["accountId"]?.stringValue)
        let src = try await alice.orgs.create(name: "move-src").orgId
        let dst = try await alice.orgs.create(name: "move-dst").orgId
        _ = try await alice.orgs.setMember(org: src, account: bobId, role: .member, passkey: passkey)
        let app = try await kvApp(alice, "moving", org: src)
        _ = try await alice.call(app, "set", ["key": "a", "value": "1"])
        let moved = try JSONValue(
            encoding: try await alice.apps.move(app: app, toOrg: dst, bringMembers: true, passkey: passkey))
        XCTAssertEqual(moved["org"]?.stringValue, dst)
        XCTAssertEqual(moved["added"]?.arrayValue?.compactMap(\.stringValue), [bobId])
        let bobRead = try await bob.view(app, "get", ["key": "a"])
        XCTAssertEqual(bobRead.result, "1")
        let asked = await refusal { _ = try await alice.apps.move(app: app, toOrg: src) }
        XCTAssertTrue(asked is ConfirmRequired, "\(String(describing: asked))")
    }

    // MARK: - Consent

    func testAsksForConsentAndReadsThePendingResult() async throws {
        let (alice, _) = try await person(0, "alice-consent")
        let org = try await alice.orgs.create(name: "Consent Co").orgId
        let started = try await alice.requestConsent(
            .renameOrg(org: org, name: "Consented Co", returnTo: "http://localhost:3000/done"))
        XCTAssertFalse(started.requestId.isEmpty)
        XCTAssertNotNil(URL(string: started.url))
        let result = try await alice.consentResult(started.requestId)
        XCTAssertEqual(result.status, .pending)
        XCTAssertEqual(result.kind, "renameOrg")
    }

    /// openConsentPage keeps the request it started; a return while it is
    /// still pending is refused, and a newer request replaces it on the node.
    func testResumesAConsentRequestFromThePagesReturn() async throws {
        let urls = try urls
        let store = MemoryConsentRequestStore()
        let alice = CalimeroClient(
            CalimeroClientOptions(baseUrl: urls[0], policy: CloudPolicy(allowMock: true), consentStore: store))
        try await alice.signUp(SoftPasskey(), name: "alice-resume")
        let org = try await alice.orgs.create(name: "Resume Co").orgId
        let started = try await alice.openConsentPage(
            .renameOrg(org: org, name: "Resumed Co", returnTo: "http://localhost:3000/done"), navigate: { _ in })
        XCTAssertEqual(store.load(), started.requestId)
        let back = try XCTUnwrap(
            URL(string: "http://localhost:3000/done?request_id=\(started.requestId)&result=approved"))
        let pending = await refusal { _ = try await alice.resumeConsent(back) }
        XCTAssertEqual((pending as? CalimeroError)?.code, "CONSENT_NOT_DECIDED", "\(String(describing: pending))")
        XCTAssertEqual(store.load(), started.requestId)
        let none = try await alice.resumeConsent(try XCTUnwrap(URL(string: "http://localhost:3000/done")))
        XCTAssertNil(none)

        // One undecided request per account per node: a newer one replaces it.
        _ = try await alice.requestConsent(
            .renameOrg(org: org, name: "Newer Co", returnTo: "http://localhost:3000/done"))
        let replaced = await refusal { _ = try await alice.resumeConsent(back) }
        XCTAssertEqual((replaced as? CalimeroError)?.code, "CONSENT_REPLACED", "\(String(describing: replaced))")
        XCTAssertNil(store.load())
    }

    // MARK: - Packages and origins

    /// Installs the signed fixture package, runs an app from it, then
    /// registers an origin for it (its first installer manages it), signs a
    /// person up from that origin and removes it again.
    func testInstallsASignedPackageAndManagesItsOrigins() async throws {
        guard let path = Self.env["MERO_E2E_MPK"], !path.isEmpty else { throw XCTSkip("MERO_E2E_MPK is not set") }
        let (alice, passkey) = try await person(0, "alice-packages")
        let unsigned = await refusal { _ = try await alice.packages.upload(body: Data([1, 2, 3])) }
        XCTAssertEqual((unsigned as? CalimeroError)?.code, "BAD_PACKAGE", "\(String(describing: unsigned))")
        let installed = try JSONValue(
            encoding: try await alice.packages.upload(body: try Data(contentsOf: URL(fileURLWithPath: path))))
        let package = try XCTUnwrap(installed["package"]?.stringValue)
        let version = try XCTUnwrap(installed["version"]?.stringValue)
        let app = try await alice.apps.create(name: "pkg-kv", package: package, version: version).appId
        _ = try await alice.call(app, "set", ["key": "a", "value": "1"])
        let read = try await alice.view(app, "get", ["key": "a"])
        XCTAssertEqual(read.result, "1")
        let info = try JSONValue(encoding: try await alice.packages.get(package: package))
        XCTAssertEqual(info["latest"]?.stringValue, version)

        let site = "https://chat.mero-swift.test"
        let asked = await refusal { _ = try await alice.packages.originsAdd(package: package, origin: site) }
        if let refused = asked as? CalimeroError, refused.code == "FORBIDDEN" {
            throw XCTSkip("another account installed \(package) first on this cluster; run on a fresh one")
        }
        XCTAssertTrue(asked is ConfirmRequired, "\(String(describing: asked))")
        _ = try await alice.packages.originsAdd(package: package, origin: site, passkey: passkey)
        let bad = await refusal {
            _ = try await alice.packages.originsAdd(package: package, origin: "not an origin", passkey: passkey)
        }
        XCTAssertTrue(bad is BadOrigin, "\(String(describing: bad))")

        let carol = try client(1)
        let carolPasskey = SoftPasskey(rpId: "chat.mero-swift.test", origin: site)
        try await carol.signUp(carolPasskey, name: "carol", origin: site)
        let carolMe = try await me(carol)
        XCTAssertEqual(carolMe["origin"]?.stringValue, site)

        let stranger = await refusal {
            _ = try await self.client(0).signIn(
                SoftPasskey(rpId: "stranger.mero-swift.test", origin: "https://stranger.mero-swift.test"),
                origin: "https://stranger.mero-swift.test")
        }
        XCTAssertEqual((stranger as? OriginNotRegistered)?.origin, "https://stranger.mero-swift.test")

        _ = try await alice.packages.originsRemove(package: package, origin: site, passkey: passkey)
        await pause()
        let ended = await refusal { _ = try await carol.auth.me() }
        XCTAssertTrue(ended is LoginRequired, "\(String(describing: ended))")
    }

    // MARK: - Sealed files and receipts

    func testUploadsAMultiPartSealedFileAndExportsItBackByteForByte() async throws {
        let (alice, _) = try await person(0, "alice-files")
        let app = try await kvApp(alice, "sealed")
        var generator = SystemRandomNumberGenerator()
        let data = Data(
            (0..<(CalimeroClient.filePartBytes + 3_000_000)).map { _ in UInt8.random(in: 0...255, using: &generator) })
        let up = try await alice.uploadFile(app, data)
        XCTAssertEqual(up.size, data.count)
        XCTAssertEqual(up.sha256, Hex.encode(sha256(data)))

        let info = try JSONValue(encoding: try await alice.apps.fileInfo(app: app, id: up.id))
        XCTAssertEqual(info["id"]?.stringValue, up.id)
        XCTAssertEqual(info["sha256"]?.stringValue, up.sha256)

        let back = try await alice.exportFile(app, up.id)
        XCTAssertEqual(back.count, data.count)
        XCTAssertTrue(back == data)

        let chunks = ChunkCount()
        let exported = try await alice.exportFile(app, up.id, to: { chunks.add($0.count) })
        XCTAssertEqual(exported, ExportedFile(size: data.count, sha256: up.sha256 ?? ""))
        XCTAssertEqual(chunks.total, data.count)
    }

    func testRefusesAnExportToAStranger() async throws {
        let (alice, _) = try await person(0, "alice-owner")
        let (mallory, _) = try await person(1, "mallory")
        let app = try await kvApp(alice, "private-files")
        let up = try await alice.uploadFile(app, Data(repeating: 7, count: 50_000))
        let refused = await refusal { _ = try await mallory.exportFile(app, up.id) }
        XCTAssertEqual((refused as? CalimeroError)?.code, "NOT_FOUND", "\(String(describing: refused))")
    }

    func testAttestsEveryReplicasReceiptKey() async throws {
        let (alice, _) = try await person(0, "alice-receipts")
        let app = try await kvApp(alice, "receipts")
        let attested = try await alice.verifyAppCluster(app)
        XCTAssertFalse(attested.keys.isEmpty)
        XCTAssertFalse(attested.cluster.isEmpty)
        XCTAssertNotNil(attested.governance["releases"])
        for key in attested.keys.values { XCTAssertEqual(key.count, 32) }
        // No run of this app: the receipt route refuses.
        let none = await refusal { _ = try await alice.pipelineReceipt(app, "no-such-run") }
        XCTAssertNotNil(none as? CalimeroError, "\(String(describing: none))")
    }

    func testTheNodesAttestationIsAMockOnADevelopmentCluster() async throws {
        let (alice, _) = try await person(0, "alice-attest")
        let doc = try JSONValue(
            encoding: try await alice.publicApi.attestation(nonce: String(repeating: "00", count: 32)))
        XCTAssertEqual(doc["mock"]?.boolValue, true)
        let usage = try JSONValue(encoding: try await alice.auth.usage())
        XCTAssertNotNil(usage.objectValue)
    }
}

/// How many plaintext bytes an export handed its sink.
private final class ChunkCount: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0

    func add(_ n: Int) {
        lock.lock()
        defer { lock.unlock() }
        count += n
    }

    var total: Int {
        lock.lock()
        defer { lock.unlock() }
        return count
    }
}
