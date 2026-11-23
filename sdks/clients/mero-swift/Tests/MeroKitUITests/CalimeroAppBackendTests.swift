import Foundation
import MeroKit
import MeroKitTestSupport
import XCTest

@testable import MeroKitUI

final class CalimeroAppBackendTests: XCTestCase {
    private var cluster: FakeCloud!
    private var node: FakeCloudNode!

    override func setUp() {
        super.setUp()
        cluster = FakeCloud()
        let cluster = self.cluster!
        node = FakeCloudNode { cluster.handle($0) }
        node.install()
    }

    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    private func backend(package: String = "com.calimero.chat") async throws -> CalimeroAppBackend {
        let client = CalimeroClient(
            CalimeroClientOptions(
                baseUrl: "https://node.test", policy: .development, session: MockURLProtocol.makeSession()))
        try await client.signIn(SoftPasskey())
        return CalimeroAppBackend(
            client: client, account: FakeCloud.me, packageName: package, passkey: SoftPasskey())
    }

    func testFoundsAnOrgForTheAppAndListsOnlyThisAppsOrgs() async throws {
        let backend = try await backend()
        let founded = try await backend.foundNamespace(name: "Acme")
        XCTAssertTrue(founded.haEnabled)
        cluster.orgs["org_other"] = ("Other", ["package": "com.calimero.calendar"], [FakeCloud.me: "member"], [:])
        let listed = try await backend.listNamespaces()
        XCTAssertEqual(listed.map(\.id), [founded.namespaceId])
        XCTAssertEqual(listed.first?.name, "Acme")
        XCTAssertEqual(listed.first?.applicationId, "com.calimero.chat")
        XCTAssertEqual(cluster.orgs[founded.namespaceId]?.metadata["package"], "com.calimero.chat")
    }

    func testCreatesTeamsAndAppInstancesInThemAndListsThem() async throws {
        let backend = try await backend()
        let org = try await backend.foundNamespace(name: "Acme").namespaceId
        let team = try await backend.createSubgroup(namespaceId: org, name: "general", isPublic: true)
        XCTAssertEqual(team, teamGroupId(org: org, team: GroupRef.parse(team).team ?? ""))
        let teamId = try XCTUnwrap(GroupRef.parse(team).team)
        XCTAssertEqual(cluster.teams[teamId]?.join, "everyone")
        let app = try await backend.createContext(
            groupId: team, applicationId: "com.calimero.chat", name: "general", initArgs: [:])
        let created = try XCTUnwrap(node.requests.last { $0.path == "/apps" }?.json)
        XCTAssertEqual(created["org"]?.stringValue, org)
        XCTAssertEqual(created["grants"], [["kind": "team", "team": .string(teamId)]])
        XCTAssertEqual(created["package"], "com.calimero.chat")

        let rootApp = try await backend.createContext(groupId: org, applicationId: "", name: "lobby", initArgs: [:])
        XCTAssertEqual(node.requests.last { $0.path == "/apps" }?.json?["grants"], [["kind": "org"]])

        let subgroups = try await backend.listSubgroups(namespaceId: org)
        XCTAssertEqual(subgroups, [AppSubgroup(groupId: team, name: "general", contextIds: [app])])
        let inTeam = try await backend.listContexts(groupId: team)
        XCTAssertEqual(inTeam, [app])
        let atRoot = try await backend.listContexts(groupId: org)
        XCTAssertEqual(atRoot, [rootApp])
        let member = try await backend.isMember(contextId: app)
        XCTAssertTrue(member)
        let stranger = try await backend.isMember(contextId: "app_nope")
        XCTAssertFalse(stranger)
    }

    func testAPerServiceAppInstanceNamesItsService() async throws {
        let backend = try await backend()
        let org = try await backend.foundNamespace(name: "Fleet").namespaceId
        _ = try await backend.createServiceContext(
            groupId: org, applicationId: "com.calimero.battleships", service: "lobby", name: "lobby", initArgs: [:])
        let created = node.requests.last { $0.path == "/apps" }?.json
        XCTAssertEqual(created?["service"], "lobby")
        XCTAssertEqual(created?["package"], "com.calimero.battleships")
    }

    func testInvitesWithATokenAndJoinsByRedeemingIt() async throws {
        let backend = try await backend()
        let org = try await backend.foundNamespace(name: "Acme").namespaceId
        let invitation = try await backend.createInvitation(namespaceId: org)
        XCTAssertEqual(invitation, OrgInvitation(namespaceId: org, token: "inv_\(org)", name: "Acme"))
        cluster.orgs[org]?.members[FakeCloud.me] = nil
        let joined = try await backend.joinNamespace(namespaceId: org, invitation: invitation)
        XCTAssertEqual(joined.namespaceId, org)
        XCTAssertEqual(joined.nodeURL, "https://node.test")
        XCTAssertEqual(cluster.orgs[org]?.members[FakeCloud.me], "member")
    }

    func testMembersRolesAndNames() async throws {
        let backend = try await backend()
        let org = try await backend.foundNamespace(name: "Acme").namespaceId
        cluster.orgs[org]?.members["acc_guest"] = "guest"
        try await backend.setMemberName(groupId: org, name: "Ada")
        let members = try await backend.listMembers(groupId: org)
        XCTAssertEqual(
            members,
            [
                AppMember(identity: "acc_guest", role: "ReadOnly", name: nil),
                AppMember(identity: FakeCloud.me, role: "Admin", name: "Ada"),
            ])
        try await backend.setMemberRole(groupId: org, identity: "acc_guest", role: .member)
        XCTAssertEqual(cluster.orgs[org]?.members["acc_guest"], "member")
        try await backend.removeMember(groupId: org, identity: "acc_guest")
        XCTAssertNil(cluster.orgs[org]?.members["acc_guest"])
        XCTAssertEqual(cluster.stepUps, 2, "a role change and a removal are passkey step-ups")
        try await backend.setGroupName(groupId: org, name: "Acme Inc")
        let renamed = try await backend.groupMetadata(groupId: org)
        XCTAssertEqual(renamed?.name, "Acme Inc")
        XCTAssertEqual(renamed?.data["package"], "com.calimero.chat")
    }

    func testStepUpsWithoutAPasskeyThrowPasskeyRequired() async throws {
        let client = CalimeroClient(
            CalimeroClientOptions(
                baseUrl: "https://node.test", policy: .development, session: MockURLProtocol.makeSession()))
        try await client.signIn(SoftPasskey())
        let bare = CalimeroAppBackend(client: client, account: FakeCloud.me, packageName: "com.calimero.chat")
        let org = try await bare.foundNamespace(name: "Acme").namespaceId
        do {
            _ = try await bare.createInvitation(namespaceId: org)
            XCTFail("expected PasskeyRequired")
        } catch let error as PasskeyRequired {
            XCTAssertEqual(error.method, "createInvitation")
        }
    }

    // MARK: Organization actions on the consent page

    func testWithConsentFoundsInvitesAndJoinsOnTheConsentPage() async throws {
        let consent = RecordingConsent()
        let backend = try await backend(consent: consent)
        let founded = try await backend.foundNamespace(name: "Acme")
        XCTAssertEqual(founded.namespaceId, "org_consent")
        let invitation = try await backend.createInvitation(namespaceId: "org_consent")
        XCTAssertEqual(invitation.token, "inv_consent")
        let joined = try await backend.joinNamespace(namespaceId: "", invitation: invitation)
        XCTAssertEqual(joined.namespaceId, "org_joined")
        XCTAssertEqual(
            consent.requests,
            [
                .createOrg(name: "Acme", returnTo: RecordingConsent.returnTo),
                .createInvite(org: "org_consent", role: .member, teams: [], returnTo: RecordingConsent.returnTo),
                .acceptInvite(token: "inv_consent", returnTo: RecordingConsent.returnTo),
            ])
        let direct = node.requests.filter {
            ($0.path == "/orgs" && $0.method == "POST") || $0.path.hasSuffix("/invites")
                || $0.path.hasSuffix("/redeem")
        }
        XCTAssertEqual(direct.map(\.path), [], "the node's organization routes are not called")
    }

    func testADeclineThrowsAndChangesNothing() async throws {
        let consent = RecordingConsent(status: "declined")
        let backend = try await backend(consent: consent)
        do {
            _ = try await backend.foundNamespace(name: "Acme")
            XCTFail("a decline founds nothing")
        } catch let error as OrgConsentNotApproved {
            XCTAssertEqual(error, OrgConsentNotApproved(kind: "createOrg", status: .declined))
        }
        XCTAssertTrue(cluster.orgs.isEmpty)
    }

    func testDecliningAnInviteOrAJoinThrowsAndCallsNoOrganizationRoute() async throws {
        let backend = try await backend(consent: RecordingConsent(status: "declined"))
        do {
            _ = try await backend.createInvitation(namespaceId: "org_x")
            XCTFail("a declined invite makes no token")
        } catch let error as OrgConsentNotApproved {
            XCTAssertEqual(error.kind, "createInvite")
            XCTAssertEqual(error.errorDescription, "Declined. Nothing was changed.")
        }
        do {
            _ = try await backend.joinNamespace(
                namespaceId: "org_x", invitation: OrgInvitation(namespaceId: "org_x", token: "inv_org_x"))
            XCTFail("a declined join joins nothing")
        } catch let error as OrgConsentNotApproved {
            XCTAssertEqual(error.kind, "acceptInvite")
        }
        XCTAssertFalse(node.requests.contains { $0.path.hasSuffix("/invites") || $0.path.hasSuffix("/redeem") })
    }

    func testAPageLeftWithoutDecidingIsNotAnApproval() async throws {
        let backend = try await backend(consent: RecordingConsent(status: "pending"))
        do {
            _ = try await backend.foundNamespace(name: "Acme")
            XCTFail("an undecided request founds nothing")
        } catch let error as OrgConsentNotApproved {
            XCTAssertEqual(error.status, .pending)
            XCTAssertEqual(error.errorDescription, "Not approved. Nothing was changed.")
        }
    }

    func testAnApprovalWithoutItsOutcomeIsAProtocolViolation() async throws {
        let backend = try await backend(consent: RecordingConsent(outcome: "null"))
        do {
            _ = try await backend.foundNamespace(name: "Acme")
            XCTFail("an approved createOrg without an org id")
        } catch let error as OrgConsentNotApproved {
            XCTFail("not a decline: \(error)")
        } catch {
            XCTAssertTrue("\(error)".contains("createOrg"), "\(error)")
        }
    }

    func testANameTheNodeRefusesLeavesTheWorkspaceWithANote() async throws {
        let consent = RecordingConsent()
        let backend = try await backend(consent: consent)
        // org_consent is not on the fake cluster: its metadata write is refused (404).
        let founded = try await backend.foundNamespace(name: "Acme")
        XCTAssertEqual(founded.namespaceId, "org_consent")
        XCTAssertEqual(founded.nameError, "the workspace name could not be saved")
    }

    // MARK: A session bound to the package (core#4934), end to end

    /// Signed in on the package's served page, as the phone is: everything the
    /// app does goes through, and nothing the node refuses such a session is
    /// called.
    func testAPackageBoundSessionRunsTheWholeAppWithoutARefusedRoute() async throws {
        cluster.boundPackage = "com.calimero.chat"
        let consent = ClusterConsent(cluster: cluster)
        let backend = try await backend(consent: consent)

        // Create a workspace: on the consent page, with Chat's app team.
        let org = try await backend.foundNamespace(name: "Acme").namespaceId
        let appTeam = try XCTUnwrap(consent.appTeams[org])
        let listed = try await backend.listNamespaces()
        XCTAssertEqual(listed.map(\.name), ["Acme"])

        // #general: an open channel reaches the app team.
        let general = try await backend.createContext(groupId: org, applicationId: "", name: "general", initArgs: [:])
        XCTAssertEqual(
            node.requests.last { $0.path == "/apps" }?.json?["grants"], [["kind": "team", "team": .string(appTeam)]])
        let openChannels = try await backend.listContexts(groupId: org)
        XCTAssertEqual(openChannels, [general])
        let noSubgroups = try await backend.listSubgroups(namespaceId: org)
        XCTAssertEqual(noSubgroups, [], "the app team is not a channel")

        // A private channel: its team is made under the app team.
        let secret = try await backend.createSubgroup(namespaceId: org, name: "secret", isPublic: false)
        let secretTeam = try XCTUnwrap(GroupRef.parse(secret).team)
        XCTAssertEqual(cluster.teamParents[secretTeam], appTeam)
        let inSecret = try await backend.createContext(
            groupId: secret, applicationId: "", name: "secret", initArgs: [:])
        let subgroups = try await backend.listSubgroups(namespaceId: org)
        XCTAssertEqual(subgroups, [AppSubgroup(groupId: secret, name: "secret", contextIds: [inSecret])])
        let stillOpen = try await backend.listContexts(groupId: org)
        XCTAssertEqual(stillOpen, [general])

        // Invite: on the consent page, naming the app team, so the invitee has Chat.
        let invitation = try await backend.createInvitation(namespaceId: org)
        XCTAssertEqual(invitation.name, "Acme")
        XCTAssertEqual(cluster.inviteTeams[invitation.token], [appTeam])

        // Join: on the consent page.
        let joined = try await backend.joinNamespace(namespaceId: "", invitation: invitation)
        XCTAssertEqual(joined.namespaceId, org)

        XCTAssertEqual(consent.kinds, ["createOrg", "createInvite", "acceptInvite"])
        XCTAssertEqual(cluster.refused, [])
    }

    /// The same session without the consent page is refused what the node
    /// refuses it, so the test above would catch a route that slipped back.
    func testWithoutTheConsentPageAPackageBoundSessionIsRefused() async throws {
        cluster.boundPackage = "com.calimero.chat"
        let backend = try await backend()
        do {
            _ = try await backend.foundNamespace(name: "Acme")
            XCTFail("POST /orgs is refused to a package-bound session")
        } catch {}
        let org = cluster.consentOrg(name: "Legacy", package: "com.calimero.other").org
        do {
            _ = try await backend.createSubgroup(namespaceId: org, name: "x", isPublic: false)
            XCTFail("a team outside the package's app team is refused")
        } catch {}
        XCTAssertEqual(cluster.refused, ["POST /orgs", "POST /orgs/\(org)/teams"])
    }

    func testAnOrgWithoutAnAppTeamKeepsOrgAccessAndInvitesToNoTeam() async throws {
        let backend = try await backend()
        let org = try await backend.foundNamespace(name: "Acme").namespaceId
        _ = try await backend.createContext(groupId: org, applicationId: "", name: "general", initArgs: [:])
        XCTAssertEqual(node.requests.last { $0.path == "/apps" }?.json?["grants"], [["kind": "org"]])
        let token = try await backend.createInvitation(namespaceId: org).token
        XCTAssertEqual(cluster.inviteTeams[token], [])
        let team = try await backend.createSubgroup(namespaceId: org, name: "t", isPublic: true)
        XCTAssertNil(cluster.teamParents[GroupRef.parse(team).team ?? ""])
    }

    func testTheAppTeamIsTheTopLevelTeamMarkedWithThePackage() {
        let org = OrgInfo(json: [
            "orgId": "o",
            "teams": [
                "t_nested": ["teamId": "t_nested", "parent": "t_chat", "metadata": ["app": ["package": "com.chat"]]],
                "t_other": ["teamId": "t_other", "metadata": ["app": ["package": "com.other"]]],
                "t_plain": ["teamId": "t_plain", "metadata": .null],
                "t_chat": ["teamId": "t_chat", "metadata": ["app": ["package": "com.chat"]]],
            ],
        ])
        XCTAssertEqual(org.appTeam(package: "com.chat"), "t_chat")
        XCTAssertEqual(org.appTeam(package: "com.other"), "t_other")
        XCTAssertNil(org.appTeam(package: "com.none"))
        XCTAssertNil(org.appTeam(package: ""))
    }

    private func backend(consent: any OrgConsentApprover) async throws -> CalimeroAppBackend {
        let client = CalimeroClient(
            CalimeroClientOptions(
                baseUrl: "https://node.test", policy: .development, session: MockURLProtocol.makeSession()))
        try await client.signIn(SoftPasskey())
        return CalimeroAppBackend(
            client: client, account: FakeCloud.me, packageName: "com.calimero.chat", passkey: SoftPasskey(),
            consent: consent)
    }
}

/// Approves (or declines) every request, answering the outcome its kind makes.
private final class RecordingConsent: OrgConsentApprover, @unchecked Sendable {
    static let returnTo = "https://chat.test/1.0.0/?native-consent=1"
    let status: String
    /// Replaces the outcome of an approval (`"null"`: none).
    let outcome: String?
    private(set) var requests: [ConsentRequest] = []

    init(status: String = "approved", outcome: String? = nil) {
        self.status = status
        self.outcome = outcome
    }

    func decide(_ request: @escaping @Sendable (String) -> ConsentRequest) async throws -> ConsentResult {
        let made = request(Self.returnTo)
        requests.append(made)
        let (kind, outcome): (String, String) =
            switch made {
            case .createOrg: ("createOrg", #"{"orgId":"org_consent"}"#)
            case .createInvite: ("createInvite", #"{"token":"inv_consent"}"#)
            case .acceptInvite: ("acceptInvite", #"{"org":"org_joined"}"#)
            default: ("other", "null")
            }
        let body = status == "approved" ? (self.outcome ?? outcome) : "null"
        let json = #"{"requestId":"r1","kind":"\#(kind)","status":"\#(status)","outcome":\#(body)}"#
        return try JSONDecoder().decode(ConsentResult.self, from: Data(json.utf8))
    }
}

/// The node's consent page over the fake cluster: approves, and does what
/// the node does (an org with the package's app team, an invite naming its
/// teams, a join into them).
private final class ClusterConsent: OrgConsentApprover, @unchecked Sendable {
    let cluster: FakeCloud
    private(set) var kinds: [String] = []
    private(set) var appTeams: [String: String] = [:]

    init(cluster: FakeCloud) { self.cluster = cluster }

    func decide(_ request: @escaping @Sendable (String) -> ConsentRequest) async throws -> ConsentResult {
        let (kind, outcome): (String, JSONValue)
        switch request("https://chat.test/1.0.0/?native-consent=1") {
        case .createOrg(let name, _):
            let made = cluster.consentOrg(name: name, package: cluster.boundPackage ?? "")
            appTeams[made.org] = made.appTeam
            (kind, outcome) = ("createOrg", ["orgId": .string(made.org)])
        case .createInvite(let org, _, let teams, _, _, _, _):
            let token = cluster.consentInvite(org: org, teams: teams ?? [])
            (kind, outcome) = ("createInvite", ["token": .string(token)])
        case .acceptInvite(let token, _):
            (kind, outcome) = ("acceptInvite", ["org": .string(cluster.consentRedeem(token))])
        default:
            (kind, outcome) = ("other", .null)
        }
        kinds.append(kind)
        let json: JSONValue = ["requestId": "r1", "kind": .string(kind), "status": "approved", "outcome": outcome]
        return try JSONDecoder().decode(ConsentResult.self, from: JSONEncoder().encode(json))
    }
}
