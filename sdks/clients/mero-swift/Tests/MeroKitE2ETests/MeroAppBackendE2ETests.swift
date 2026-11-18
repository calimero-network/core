import Foundation
import MeroKit
import MeroKitUI
import XCTest

/// The app layer the sample app and the mobile apps use (``CalimeroAppBackend``)
/// against a `merod local` cluster: an organization founded for a package, a
/// team, an app instance from the package, an invitation another account
/// redeems, calls and views through the contract, events, names and blobs.
///
/// Skipped unless `MERO_E2E_NODE_URL` and `MERO_E2E_MPK` (a signed kv-store
/// `.mpk`) are set; `MERO_E2E_NODES` as for ``ClusterE2ETests``.
///
/// Named to run after ``ClusterE2ETests`` (XCTest runs classes by name): the
/// fixture package's first installer manages its origins, which that suite's
/// origin case needs to be.
final class MeroAppBackendE2ETests: XCTestCase {
    private static let env = ProcessInfo.processInfo.environment

    private func client(_ i: Int) throws -> CalimeroClient {
        guard let first = Self.env["MERO_E2E_NODE_URL"], !first.isEmpty else {
            throw XCTSkip("MERO_E2E_NODE_URL is not set")
        }
        let urls = [first] + (Self.env["MERO_E2E_NODES"] ?? "").split(separator: ",").map(String.init)
        return CalimeroClient(CalimeroClientOptions(baseUrl: urls[i % urls.count], policy: .development))
    }

    /// A new account on node `i` as an app backend for `package`; with
    /// `install`, the account first installs that signed `.mpk` (a cluster's
    /// registry would install the package on first use; a development cluster
    /// has none) and `package` is what it holds.
    private func backend(
        _ i: Int, _ name: String, package: String = "", install: Data? = nil
    ) async throws
        -> CalimeroAppBackend
    {
        let c = try client(i)
        let passkey = SoftPasskey()
        let session = try await c.signUp(passkey, name: name)
        var package = package
        if let install {
            let installed = try JSONValue(encoding: try await c.packages.upload(body: install))
            package = try XCTUnwrap(installed["package"]?.stringValue)
        }
        return CalimeroAppBackend(client: c, account: session.accountId, packageName: package, passkey: passkey)
    }

    func testRunsTheAppLayerOnAPackage() async throws {
        guard let path = Self.env["MERO_E2E_MPK"], !path.isEmpty else { throw XCTSkip("MERO_E2E_MPK is not set") }
        let alice = try await backend(0, "alice-app", install: try Data(contentsOf: URL(fileURLWithPath: path)))
        let package = alice.packageName
        let founded = try await alice.foundNamespace(name: "Team Space")
        let org = founded.namespaceId
        let spaces = try await alice.listNamespaces()
        XCTAssertTrue(spaces.contains { $0.id == org }, "\(spaces)")

        // A team's group id: "{org}/{team}".
        let group = try await alice.createSubgroup(namespaceId: org, name: "general", isPublic: true)
        XCTAssertTrue(group.hasPrefix("\(org)/"), group)
        let app = try await alice.createContext(
            groupId: group, applicationId: package, name: "notes", initArgs: [:])
        let contexts = try await alice.listContexts(groupId: group)
        XCTAssertEqual(contexts, [app])
        let subgroups = try await alice.listSubgroups(namespaceId: org)
        XCTAssertTrue(subgroups.contains { $0.groupId == group && $0.contextIds.contains(app) }, "\(subgroups)")

        _ = try await alice.contract.execute(contextId: app, method: "set", args: ["key": "a", "value": "1"])
        let value = try await alice.contract.query(contextId: app, method: "get", args: ["key": "a"])
        XCTAssertEqual(value, "1")
        for try await event in alice.events(contextId: app) {
            XCTAssertEqual(event.contextId, app)
            XCTAssertFalse(event.kind.isEmpty)
            break
        }

        try await alice.setContextName(groupId: group, contextId: app, name: "Notes")
        let entries = try await alice.listContextEntries(groupId: group)
        XCTAssertEqual(entries.first { $0.id == app }?.name, "Notes")

        let blob = try await alice.uploadBlob(contextId: app, data: Data("hello".utf8))
        let fetched = try await alice.getBlob(blobId: blob.blobId, contextId: app)
        XCTAssertEqual(fetched, Data("hello".utf8))

        // Bob joins with an invitation and reaches the team's app.
        let invitation = try await alice.createInvitation(namespaceId: org)
        let bob = try await backend(1, "bob-app", package: package)
        let joined = try await bob.joinNamespace(namespaceId: org, invitation: invitation)
        XCTAssertEqual(joined.namespaceId, org)
        let reachable = try await bob.isMember(contextId: app)
        XCTAssertTrue(reachable)
        let bobRead = try await bob.contract.query(contextId: app, method: "get", args: ["key": "a"])
        XCTAssertEqual(bobRead, "1")
        let members = try await alice.listMembers(groupId: org)
        XCTAssertTrue(members.contains { $0.identity == bob.account }, "\(members)")

        try await alice.removeMember(groupId: org, identity: bob.account)
        try? await Task.sleep(nanoseconds: 300_000_000)
        let after = try await bob.isMember(contextId: app)
        XCTAssertFalse(after)
    }
}
