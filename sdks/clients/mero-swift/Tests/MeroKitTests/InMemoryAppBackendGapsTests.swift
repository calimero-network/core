import Foundation
import MeroKit
import MeroKitTestSupport
import XCTest

/// Names, blobs, leaving and members on the in-memory backend.
final class InMemoryAppBackendGapsTests: XCTestCase {
    private struct Seeded {
        let backend: InMemoryAppBackend
        let namespace: String
        let group: String
        let context: String
    }

    private func seeded() async throws -> Seeded {
        let backend = InMemoryAppBackend()
        let namespace = try await backend.foundNamespace(name: "Team").namespaceId
        let group = try await backend.createSubgroup(namespaceId: namespace, name: "boards", isPublic: true)
        let context = try await backend.createContext(
            groupId: group, applicationId: backend.applicationId, name: "Roadmap", initArgs: [:])
        return Seeded(backend: backend, namespace: namespace, group: group, context: context)
    }

    func testContextsAndGroupsCarryTheirNames() async throws {
        let seed = try await seeded()
        let (backend, namespace, group, context) = (seed.backend, seed.namespace, seed.group, seed.context)

        let entries = try await backend.listContextEntries(groupId: group)
        XCTAssertEqual(entries, [AppContextEntry(id: context, name: "Roadmap")])
        let contextMeta = try await backend.contextMetadata(groupId: group, contextId: context)
        XCTAssertEqual(contextMeta, AppMetadata(name: "Roadmap"))
        let groupMeta = try await backend.groupMetadata(groupId: group)
        XCTAssertEqual(groupMeta?.name, "boards")
        let unknown = try await backend.groupMetadata(groupId: "nope")
        XCTAssertNil(unknown)

        try await backend.setContextName(groupId: group, contextId: context, name: "Q3")
        try await backend.setGroupName(groupId: group, name: "planning")
        try await backend.setGroupName(groupId: namespace, name: "Crew")
        let renamed = try await backend.listContextEntries(groupId: group)
        XCTAssertEqual(renamed.first?.name, "Q3")
        let subgroups = try await backend.listSubgroups(namespaceId: namespace)
        XCTAssertEqual(subgroups.first?.name, "planning")
        let namespaces = try await backend.listNamespaces()
        XCTAssertEqual(namespaces.first?.name, "Crew")
        do {
            try await backend.setGroupName(groupId: "nope", name: "x")
            XCTFail("an unknown group")
        } catch AccountError.invalidInput {}
    }

    func testABlobReadsBackOnlyThroughItsContext() async throws {
        let seed = try await seeded()
        let (backend, context) = (seed.backend, seed.context)
        let blob = try await backend.uploadBlob(contextId: context, data: Data("hello".utf8))
        XCTAssertEqual(blob.size, 5)
        let bytes = try await backend.getBlob(blobId: blob.blobId, contextId: context)
        XCTAssertEqual(bytes, Data("hello".utf8))
        XCTAssertEqual(backend.blobs[blob.blobId], Data("hello".utf8))
        do {
            _ = try await backend.getBlob(blobId: blob.blobId, contextId: "other")
            XCTFail("another context cannot read it")
        } catch AccountError.invalidInput {}

        backend.hasRelay = false
        do {
            _ = try await backend.uploadBlob(contextId: context, data: Data())
            XCTFail("no relay, no upload")
        } catch is SignedOutError {}
    }

    func testMembersRolesAndNames() async throws {
        let seed = try await seeded()
        let (backend, namespace) = (seed.backend, seed.namespace)
        let bob = String(repeating: "b0", count: 32)
        let founder = try await backend.listMembers(groupId: namespace)
        XCTAssertEqual(founder, [AppMember(identity: backend.account, role: "Admin")])
        XCTAssertTrue(founder[0].isAdmin)

        backend.groupMembers[namespace, default: []].append(AppMember(identity: bob, role: "Member", name: "Bob"))
        try await backend.setMemberName(groupId: namespace, name: "Ana")
        try await backend.setMemberRole(groupId: namespace, identity: bob, role: .readOnly)
        var members = try await backend.listMembers(groupId: namespace)
        XCTAssertEqual(
            members,
            [
                AppMember(identity: backend.account, role: "Admin", name: "Ana"),
                AppMember(identity: bob, role: "ReadOnly", name: "Bob"),
            ])

        try await backend.removeMember(groupId: namespace, identity: bob)
        members = try await backend.listMembers(groupId: namespace)
        XCTAssertEqual(members.map(\.identity), [backend.account])
        do {
            try await backend.setMemberRole(groupId: namespace, identity: bob, role: .admin)
            XCTFail("not a member any more")
        } catch AccountError.invalidInput {}
    }

    func testJoiningMakesTheAccountAMember() async throws {
        let backend = InMemoryAppBackend()
        let namespace = String(repeating: "7d", count: 32)
        _ = try await backend.joinNamespace(
            namespaceId: namespace, invitation: InMemoryAppBackend.invitation(namespaceId: namespace))
        let members = try await backend.listMembers(groupId: namespace)
        XCTAssertEqual(members, [AppMember(identity: backend.account, role: "Member")])
    }

    func testLeavingAndDeleting() async throws {
        let seed = try await seeded()
        let (backend, namespace, group, context) = (seed.backend, seed.namespace, seed.group, seed.context)
        try await backend.leaveSubgroup(groupId: group)
        var member = try await backend.isMember(contextId: context)
        XCTAssertFalse(member, "leaving a subgroup leaves its contexts")
        var subgroups = try await backend.listSubgroups(namespaceId: namespace)
        XCTAssertEqual(subgroups.map(\.groupId), [group], "an open subgroup stays listed")

        try await backend.joinContext(contextId: context)
        try await backend.removeContext(groupId: group, contextId: context)
        let left = try await backend.listContexts(groupId: group)
        XCTAssertEqual(left, [])
        member = try await backend.isMember(contextId: context)
        XCTAssertFalse(member)

        try await backend.deleteSubgroup(groupId: group)
        subgroups = try await backend.listSubgroups(namespaceId: namespace)
        XCTAssertEqual(subgroups, [])

        try await backend.leaveNamespace(namespaceId: namespace)
        let namespaces = try await backend.listNamespaces()
        XCTAssertEqual(namespaces, [])
        XCTAssertEqual(backend.groupMembers[namespace], [])
        XCTAssertTrue(backend.log.contains("leaveNamespace \(namespace)"))
    }

    func testABackendOfAnAppsOwnRefusesTheNewCallsByName() async {
        let backend = MinimalBackend()
        do {
            _ = try await backend.listMembers(groupId: "g")
            XCTFail("expected a refusal")
        } catch let error as MeroAppBackendUnsupported {
            XCTAssertEqual(error.method, "listMembers")
        } catch {
            XCTFail("\(error)")
        }
    }

    func testABoundContractIsWhatAGeneratedClientCalls() {
        let calling: any ContextContractCalling = RecordingContextContract().bound(to: "ctx")
        XCTAssertEqual((calling as? BoundContextContract)?.contextId, "ctx")
    }
}

/// A backend written before the names, blobs and members calls: the defaults keep it conforming.
private struct MinimalBackend: MeroAppBackend {
    let account = ""
    var contract: any ContextContract { SignedOutContextContract() }
    let hasRelay = false
    let canSubscribe = false
    func listNamespaces() async throws -> [AppNamespace] { [] }
    func listSubgroups(namespaceId: String) async throws -> [AppSubgroup] { [] }
    func listContexts(groupId: String) async throws -> [String] { [] }
    func isMember(contextId: String) async throws -> Bool { false }
    func joinContext(contextId: String) async throws {}
    func foundNamespace(name: String) async throws -> AppFoundedNamespace {
        AppFoundedNamespace(namespaceId: "", haEnabled: false)
    }
    func createSubgroup(namespaceId: String, name: String, isPublic: Bool) async throws -> String { "" }
    func createContext(
        groupId: String, applicationId: String, name: String, initArgs: JSONValue
    ) async throws
        -> String
    { "" }
    func createInvitation(namespaceId: String) async throws -> SignedGroupOpenInvitation {
        InMemoryAppBackend.invitation(namespaceId: namespaceId)
    }
    func joinNamespace(namespaceId: String, invitation: SignedGroupOpenInvitation) async throws -> AppJoinedNamespace {
        AppJoinedNamespace(namespaceId: namespaceId, published: false, nodeURL: "")
    }
    func events(contextId: String) -> AsyncThrowingStream<AppContextEvent, Error> {
        AsyncThrowingStream { $0.finish() }
    }
}
