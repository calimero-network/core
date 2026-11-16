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
        XCTAssertEqual(created["access"], ["kind": "team", "team": .string(teamId)])
        XCTAssertEqual(created["package"], "com.calimero.chat")

        let rootApp = try await backend.createContext(groupId: org, applicationId: "", name: "lobby", initArgs: [:])
        XCTAssertEqual(node.requests.last { $0.path == "/apps" }?.json?["access"], ["kind": "org"])

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
}
