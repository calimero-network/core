import Foundation
import MeroKit
import MeroKitTestSupport
import XCTest

/// The in-memory backend apps test their view models against behaves like a
/// relay as far as a view model can tell.
final class InMemoryAppBackendTests: XCTestCase {
    func testFoundingThenCreatingFillsTheListings() async throws {
        let backend = InMemoryAppBackend()
        let founded = try await backend.foundNamespace(name: "Team")
        let group = try await backend.createSubgroup(namespaceId: founded.namespaceId, name: "general", isPublic: true)
        let context = try await backend.createContext(
            groupId: group, applicationId: backend.applicationId, name: "general", initArgs: ["name": "general"])

        let namespaces = try await backend.listNamespaces()
        XCTAssertEqual(namespaces.map(\.id), [founded.namespaceId])
        XCTAssertEqual(namespaces.first?.name, "Team")
        XCTAssertEqual(founded.namespaceId.count, 64)
        XCTAssertTrue(founded.haEnabled)
        let subgroups = try await backend.listSubgroups(namespaceId: founded.namespaceId)
        XCTAssertEqual(subgroups, [AppSubgroup(groupId: group, name: "general", contextIds: [context])])
        let inGroup = try await backend.listContexts(groupId: group)
        XCTAssertEqual(inGroup, [context])
        let member = try await backend.isMember(contextId: context)
        XCTAssertTrue(member, "the creator is a member of what it creates")
        XCTAssertEqual(backend.publicSubgroups, [group])
        XCTAssertEqual(backend.initArgs[context], ["name": "general"])
        XCTAssertEqual(
            backend.log,
            [
                "foundNamespace Team", "createSubgroup \(founded.namespaceId) general open",
                "createContext \(group) \(backend.applicationId) general", "listNamespaces",
                "listSubgroups \(founded.namespaceId)", "listContexts \(group)", "isMember \(context)",
            ])
    }

    func testJoiningGivesARelaylessAccountItsRelay() async throws {
        let backend = InMemoryAppBackend()
        backend.hasRelay = false
        let namespace = String(repeating: "7d", count: 32)
        do {
            _ = try await backend.contract.query(contextId: "c", method: "get", args: [:])
            XCTFail("a relayless contract refuses")
        } catch is SignedOutError {}

        let invitation = InMemoryAppBackend.invitation(namespaceId: namespace)
        let joined = try await backend.joinNamespace(namespaceId: namespace, invitation: invitation)

        XCTAssertTrue(joined.published)
        XCTAssertEqual(joined.relayURL, "https://relay.test")
        XCTAssertTrue(backend.hasRelay)
        let ids = try await backend.listNamespaces().map(\.id)
        XCTAssertEqual(ids, [namespace])
        XCTAssertEqual(invitation.namespaceId, namespace)
    }

    func testJoiningAContextMakesTheAccountAMember() async throws {
        let backend = InMemoryAppBackend()
        let before = try await backend.isMember(contextId: "ctx")
        try await backend.joinContext(contextId: "ctx")
        let after = try await backend.isMember(contextId: "ctx")
        XCTAssertFalse(before)
        XCTAssertTrue(after)
    }

    func testFailureFailsEveryAdminCall() async {
        let backend = InMemoryAppBackend()
        backend.failure = URLError(.notConnectedToInternet)
        do {
            _ = try await backend.listNamespaces()
            XCTFail("expected the failure")
        } catch {
            XCTAssertEqual((error as? URLError)?.code, .notConnectedToInternet)
        }
        XCTAssertEqual(backend.log, ["listNamespaces"])
    }

    func testEmittedEventsReachTheirContextsStreamOnly() async throws {
        let backend = InMemoryAppBackend()
        let stream = backend.events(contextId: "a")
        backend.emit(AppContextEvent(contextId: "b", kind: "StateMutation"))
        backend.emit(AppContextEvent(contextId: "a", kind: "StateMutation"))
        backend.finishEvents()

        var seen: [AppContextEvent] = []
        for try await event in stream { seen.append(event) }
        XCTAssertEqual(seen, [AppContextEvent(contextId: "a", kind: "StateMutation")])
    }

    func testNoStreamWithoutASubscription() async throws {
        let backend = InMemoryAppBackend()
        backend.canSubscribe = false
        var count = 0
        for try await _ in backend.events(contextId: "a") { count += 1 }
        XCTAssertEqual(count, 0)
    }
}
