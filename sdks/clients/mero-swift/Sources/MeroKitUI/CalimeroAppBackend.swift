import Foundation
import MeroAPI
import MeroKit
import OpenAPIRuntime

/// ``MeroAppBackend`` over a signed-in ``CalimeroClient``: Calimero 1.0's
/// orgs, teams and app instances under the app layer's names.
/// (== Kotlin `com.calimero.mero.compose.CalimeroAppBackend`.)
///
/// | Backend call | Node route |
/// | --- | --- |
/// | ``listNamespaces()`` | `GET /auth/me` (orgs), `GET /orgs/{org}` (metadata, members), `GET /apps?package=` |
/// | ``listSubgroups(namespaceId:)`` | `GET /orgs/{org}` (teams but the package's app team), `GET /orgs/{org}/teams/{team}` (apps) |
/// | ``listContexts(groupId:)`` | a team's apps; an org's apps in no team or the package's app team (`GET /apps`) |
/// | ``isMember(contextId:)`` / ``joinContext(contextId:)`` | `GET /apps/{app}` (reachable or `NotFound`) |
/// | ``foundNamespace(name:)`` | `POST /orgs`, `POST /orgs/{org}/metadata {package, name}` |
/// | ``createSubgroup(namespaceId:name:isPublic:)`` | `POST /orgs/{org}/teams {join: everyone \| invite, parent: the app team}` |
/// | ``createContext(groupId:applicationId:name:initArgs:)`` / ``createServiceContext(groupId:applicationId:service:name:initArgs:)`` | `POST /apps {package (+ service) \| bundle, org, grants: [team \| the app team \| org], initArgs}` |
/// | ``createInvitation(namespaceId:)`` / ``joinNamespace(namespaceId:invitation:)`` | `POST /orgs/{org}/invites {teams: the app team}`, `POST /invites/{token}/redeem` |
/// | ``events(contextId:)`` | `GET /apps/{app}/events` (sealed SSE) |
/// | contract `query` / `execute` | `POST /apps/{app}/views` / `POST /apps/{app}/intents` |
/// | blobs | `POST /apps/{app}/blobs`, `GET /apps/{app}/blobs/{id}` |
/// | names | org: metadata `name`; team: its name; app: metadata `name` (else its name) |
/// | members | `GET /orgs/{org}` / team; `POST …/members`, `DELETE …/members/{account}` |
///
/// Membership changes and invites are passkey step-ups on 1.0 (`412
/// CONFIRM_REQUIRED`): they are approved with ``passkey``. Without one they
/// throw ``PasskeyRequired``.
///
/// With ``consent`` (a session bound to the package, which the node refuses
/// organization routes), ``foundNamespace(name:)``, ``createInvitation(namespaceId:)``
/// and ``joinNamespace(namespaceId:invitation:)`` are approved on the node's
/// consent page instead (`createOrg`, `createInvite`, `acceptInvite`), with
/// the same answers. Declining throws ``OrgConsentNotApproved``.
///
/// The package's app team (``OrgInfo/appTeam(package:)``, which an org made on
/// the consent page starts with) is the org's own place for the app: open
/// instances reach it, invites name it, and teams are made under it.
///
/// A `groupId` is `"{orgId}/{teamId}"` for a team (``teamGroupId(org:team:)``)
/// or an org id for the org itself, as a namespace's root group was.
public final class CalimeroAppBackend: MeroAppBackend, Sendable {
    public let account: String
    public let client: CalimeroClient
    public let contract: any ContextContract
    /// The app's package: what an org is founded for and an app instance created from.
    public let packageName: String
    /// The package version new app instances run; the latest when `nil`.
    public let packageVersion: String?
    /// The signed-in account's passkey, which approves the step-ups
    /// (members, invites); `nil` makes them throw ``PasskeyRequired``.
    public let passkey: (any PasskeyProvider)?
    /// Where organization actions are approved for a session bound to the
    /// package (``MeroClient/servedPageConsent(cluster:callback:appsDomain:opener:confirm:)``);
    /// `nil`: the node is called directly.
    public let consent: (any OrgConsentApprover)?

    public init(
        client: CalimeroClient, account: String, packageName: String, packageVersion: String? = nil,
        passkey: (any PasskeyProvider)? = nil, consent: (any OrgConsentApprover)? = nil
    ) {
        self.client = client
        self.passkey = passkey
        self.consent = consent
        self.account = account
        self.packageName = packageName
        self.packageVersion = packageVersion
        contract = CloudContextContract(client: client)
    }

    // MARK: Reads

    // The node's typed answers, read as MeroKit's types through their JSON.

    func accountInfo() async throws -> AccountInfo {
        AccountInfo(json: try JSONValue(encoding: try await client.auth.me()))
    }

    func fetchOrg(_ id: String) async throws -> OrgInfo {
        OrgInfo(json: try JSONValue(encoding: try await client.orgs.get(org: id)))
    }

    func fetchTeam(_ org: String, _ team: String) async throws -> TeamInfo {
        TeamInfo(json: try JSONValue(encoding: try await client.orgs.teamGet(org: org, team: team)), org: org)
    }

    func fetchApps(package: String? = nil) async throws -> [AppInfo] {
        let apps = try await client.apps.list(package: package).apps
        return try MeroJSON.convert(apps, to: [AppInfo].self)
    }

    func appRecord(_ id: String) async throws -> JSONValue {
        try JSONValue(encoding: try await client.apps.get(app: id))
    }

    /// An invite token to `org`, naming the package's app team when it has
    /// one: an invitee outside it has no way into the app there.
    func invite(_ org: String, appTeam: String?) async throws -> String {
        let teams = appTeam.map { [$0] } ?? []
        if let consent {
            let result = try await Self.approved(
                consent.decide { .createInvite(org: org, role: .member, teams: teams, returnTo: $0) })
            guard let token = result.inviteToken else { throw Self.noOutcome(result) }
            return token
        }
        let approval = try approver("createInvitation")
        return try await client.orgs.inviteCreate(org: org, role: .member, teams: teams, passkey: approval).token
    }

    /// The package's app team in `org` (``OrgInfo/appTeam(package:)``).
    func appTeam(_ org: OrgInfo) -> String? {
        org.appTeam(package: packageName)
    }

    func createApp(
        _ name: String, source: AppSource, org: String, grants: [AppGrant], initArgs: JSONValue
    ) async throws -> String {
        var hash: String?
        var release: (package: String, version: String?)?
        var service: String?
        switch source {
        case .bundle(let value): hash = value
        case .package(let id, let v, let s, _):
            release = (id, v)
            service = s
        }
        let bundle = hash
        let package = release?.package
        let version = release?.version
        let named = service
        let args = try OpenAPIValueContainer(initArgs)
        return try await client.apps.create(
            name: name, bundle: bundle, package: package, version: version, service: named, org: org,
            grants: try grants.map { try $0.generated }, initArgs: args
        ).appId
    }

    /// ``passkey``, or ``PasskeyRequired`` for `method`.
    func approver(_ method: String) throws -> any PasskeyProvider {
        guard let passkey else { throw PasskeyRequired(method: method) }
        return passkey
    }

    public var hasRelay: Bool { true }
    public var canSubscribe: Bool { true }

    // MARK: Groups

    /// Where `groupId` lives: an org (`team` nil), or a team of an org (`org/team`).
    func locate(_ groupId: String) -> (org: String, team: String?) {
        let ref = GroupRef.parse(groupId)
        return (ref.org, ref.team)
    }

    public func listNamespaces() async throws -> [AppNamespace] {
        let orgs = try await accountInfo().orgs
        let running = packageName.isEmpty ? [] : Set(try await fetchApps(package: packageName).compactMap(\.org))
        var out: [AppNamespace] = []
        for membership in orgs {
            let org = try await fetchOrg(membership.orgId)
            let mine = org.metadata["package"]?.stringValue == packageName || running.contains(org.orgId)
            guard packageName.isEmpty || mine else { continue }
            out.append(
                AppNamespace(
                    id: org.orgId, name: org.metadata["name"]?.stringValue ?? org.name, applicationId: packageName,
                    memberCount: org.members.count))
        }
        return out
    }

    public func listSubgroups(namespaceId: String) async throws -> [AppSubgroup] {
        let org = try await fetchOrg(namespaceId)
        // The package's app team is the org's own place for the app (its
        // instances are listed with the org's, ``listContextEntries(groupId:)``).
        let own = appTeam(org)
        var out: [AppSubgroup] = []
        for team in org.teams.values.sorted(by: { $0.name < $1.name }) where team.teamId != own {
            let apps = (try? await fetchTeam(namespaceId, team.teamId).apps.map(\.appId)) ?? []
            out.append(
                AppSubgroup(
                    groupId: teamGroupId(org: namespaceId, team: team.teamId), name: team.name, contextIds: apps))
        }
        return out
    }

    public func listContexts(groupId: String) async throws -> [String] {
        try await listedApps(groupId).map(\.id)
    }

    public func listContextEntries(groupId: String) async throws -> [AppContextEntry] {
        let listed = try await listedApps(groupId)
        // A name set with `setContextName` is the app's metadata `name`, which
        // no listing carries: each app's own record does.
        let named = await withTaskGroup(of: (String, String?).self) { group in
            for app in listed {
                group.addTask { (app.id, try? await self.appRecord(app.id)["metadata"]?["name"]?.stringValue) }
            }
            var out: [String: String] = [:]
            for await (id, name) in group { if let name { out[id] = name } }
            return out
        }
        return listed.map { AppContextEntry(id: $0.id, name: named[$0.id] ?? $0.name) }
    }

    /// The apps directly in `groupId`, with the names they were created with.
    private func listedApps(_ groupId: String) async throws -> [(id: String, name: String)] {
        let place = locate(groupId)
        let listed: [(id: String, name: String)]
        if let team = place.team {
            listed = try await fetchTeam(place.org, team).apps.map { ($0.appId, $0.name) }
        } else {
            // The org's own: its apps that no team holds, and those of the
            // package's app team.
            let org = try await fetchOrg(place.org)
            let own = appTeam(org)
            var inTeams = Set<String>()
            for team in org.teams.keys where team != own {
                for app in (try? await fetchTeam(place.org, team).apps) ?? [] { inTeams.insert(app.appId) }
            }
            listed = try await fetchApps().filter { $0.org == place.org && !inTeams.contains($0.appId) }
                .map { ($0.appId, $0.name) }
        }
        return listed
    }

    public func isMember(contextId: String) async throws -> Bool {
        do {
            _ = try await appRecord(contextId)
            return true
        } catch is NotFound {
            return false
        }
    }

    public func joinContext(contextId: String) async throws {
        _ = try await appRecord(contextId)
    }

    public func foundNamespace(name: String) async throws -> AppFoundedNamespace {
        if let consent {
            let result = try await Self.approved(consent.decide { .createOrg(name: name, returnTo: $0) })
            guard let org = result.orgId else { throw Self.noOutcome(result) }
            // A session bound to the package may be refused the metadata
            // write; the org is then listed by the app instances it runs.
            let named = try? await setMetadata(org: org, name: name)
            return AppFoundedNamespace(
                namespaceId: org, nameError: named == nil ? "the workspace name could not be saved" : nil)
        }
        let org = try await client.orgs.create(name: name).orgId
        try await setMetadata(org: org, name: name)
        return AppFoundedNamespace(namespaceId: org)
    }

    func setMetadata(org: String, name: String) async throws {
        var metadata: [String: JSONValue] = ["name": .string(name)]
        if !packageName.isEmpty { metadata["package"] = .string(packageName) }
        let orgMetadata = try OpenAPIValueContainer(JSONValue.object(metadata))
        _ = try await client.orgs.metadata(org: org, metadata: orgMetadata)
    }

    /// `result`, when approved; ``OrgConsentNotApproved`` otherwise.
    static func approved(_ result: ConsentResult) throws -> ConsentResult {
        guard result.isApproved else { throw OrgConsentNotApproved(kind: result.kind, status: result.status) }
        return result
    }

    static func noOutcome(_ result: ConsentResult) -> Error {
        AccountError.protocolViolation("an approved \(result.kind) answered no outcome")
    }

    public func createSubgroup(namespaceId: String, name: String, isPublic: Bool) async throws -> String {
        let join: TeamJoin = isPublic ? .everyone : .invite
        // Under the package's app team when the org has one: a session bound
        // to the package makes teams there only.
        let parent = appTeam(try await fetchOrg(namespaceId))
        let team = try await client.orgs.teamCreate(
            org: namespaceId, name: name, parent: parent, join: join.generated
        ).teamId
        return teamGroupId(org: namespaceId, team: team)
    }

    /// `applicationId` is a package (`package#service` for one service of a
    /// multi-service package), a bundle hash (dev clusters), or empty for the app's own package.
    public func createContext(
        groupId: String, applicationId: String, name: String, initArgs: JSONValue
    ) async throws -> String {
        let place = locate(groupId)
        var package = applicationId.isEmpty ? packageName : applicationId
        var service: String?
        if let hash = package.firstIndex(of: "#") {
            service = String(package[package.index(after: hash)...])
            package = String(package[..<hash])
        }
        let source: AppSource =
            Hex.is32(package) && service == nil
            ? .bundle(package)
            : .package(package, version: package == packageName ? packageVersion : nil, service: service)
        // The org's own instance reaches the package's app team when the org
        // has one (everyone with the app there), else the whole org.
        let team: String?
        if let placed = place.team {
            team = placed
        } else {
            team = appTeam(try await fetchOrg(place.org))
        }
        return try await createApp(
            name, source: source, org: place.org, grants: [team.map { .team($0) } ?? .org], initArgs: initArgs)
    }

    public func createServiceContext(
        groupId: String, applicationId: String, service: String, name: String, initArgs: JSONValue
    ) async throws -> String {
        try await createContext(
            groupId: groupId, applicationId: "\(applicationId.isEmpty ? packageName : applicationId)#\(service)",
            name: name, initArgs: initArgs)
    }

    public func createInvitation(namespaceId: String) async throws -> OrgInvitation {
        let org = try? await fetchOrg(namespaceId)
        return OrgInvitation(
            namespaceId: namespaceId,
            token: try await invite(namespaceId, appTeam: org.flatMap(appTeam)),
            name: org.map { $0.metadata["name"]?.stringValue ?? $0.name })
    }

    public func joinNamespace(namespaceId: String, invitation: OrgInvitation) async throws -> AppJoinedNamespace {
        let token = invitation.token
        let org: String
        if let consent {
            let result = try await Self.approved(consent.decide { .acceptInvite(token: token, returnTo: $0) })
            guard let joined = result.joinedOrg else { throw Self.noOutcome(result) }
            org = joined
        } else {
            org = try await client.orgs.inviteRedeem(token: token).org
        }
        return AppJoinedNamespace(
            namespaceId: org.isEmpty ? namespaceId : org, published: true, nodeURL: await client.nodeUrl)
    }

    public func events(contextId: String) -> AsyncThrowingStream<AppContextEvent, Error> {
        let source = client.events(contextId)
        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    for try await event in source {
                        continuation.yield(
                            AppContextEvent(contextId: contextId, kind: event.kind, data: event.data, id: event.id))
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    // MARK: Names

    public func groupMetadata(groupId: String) async throws -> AppMetadata? {
        let place = locate(groupId)
        if let team = place.team {
            let info = try await fetchTeam(place.org, team)
            return Self.metadata(info.metadata, name: info.name)
        }
        let org = try await fetchOrg(place.org)
        return Self.metadata(org.metadata, name: org.name)
    }

    public func contextMetadata(groupId: String, contextId: String) async throws -> AppMetadata? {
        let app = try await appRecord(contextId)
        return Self.metadata(app["metadata"] ?? .null, name: app["name"]?.stringValue)
    }

    public func setGroupName(groupId: String, name: String) async throws {
        let place = locate(groupId)
        if let team = place.team {
            try await client.updateTeam(place.org, team, name: name)
            return
        }
        let org = try await fetchOrg(place.org)
        var metadata = org.metadata.objectValue ?? [:]
        metadata["name"] = .string(name)
        let orgMetadata = try OpenAPIValueContainer(JSONValue.object(metadata))
        _ = try await client.orgs.metadata(org: place.org, metadata: orgMetadata)
    }

    public func setContextName(groupId: String, contextId: String, name: String) async throws {
        var metadata = try await appRecord(contextId)["metadata"]?.objectValue ?? [:]
        metadata["name"] = .string(name)
        let appMetadata = try OpenAPIValueContainer(JSONValue.object(metadata))
        _ = try await client.apps.metadata(app: contextId, metadata: appMetadata)
    }

    /// A record's name (its metadata's `name`, else `fallback`) and its other string fields.
    static func metadata(_ value: JSONValue, name fallback: String?) -> AppMetadata? {
        let fields = value.objectValue ?? [:]
        let name = fields["name"]?.stringValue ?? fallback.flatMap { $0.isEmpty ? nil : $0 }
        let data = fields.filter { $0.key != "name" }.compactMapValues(\.stringValue)
        if name == nil && data.isEmpty { return nil }
        return AppMetadata(name: name, data: data)
    }

    // MARK: Blobs

    public func uploadBlob(contextId: String, data: Data) async throws -> AppBlob {
        let blob = try await client.apps.blobUpload(app: contextId, body: data)
        return AppBlob(blobId: blob.blobId, size: Int(blob.size))
    }

    public func getBlob(blobId: String, contextId: String) async throws -> Data {
        try await client.apps.blobDownload(app: contextId, id: blobId)
    }

    // MARK: Leaving and deleting

    public func leaveNamespace(namespaceId: String) async throws {
        let approval = try approver("leaveNamespace")
        _ = try await client.orgs.removeMember(
            org: namespaceId, account: account, passkey: approval)
    }

    public func leaveSubgroup(groupId: String) async throws {
        let place = locate(groupId)
        guard let team = place.team else {
            let approval = try approver("leaveSubgroup")
            _ = try await client.orgs.removeMember(
                org: place.org, account: account, passkey: approval)
            return
        }
        let approval = try approver("leaveSubgroup")
        _ = try await client.orgs.teamRemoveMember(
            org: place.org, team: team, account: account, passkey: approval)
    }

    public func deleteSubgroup(groupId: String) async throws {
        let place = locate(groupId)
        guard let team = place.team else {
            throw AccountError.invalidInput("\(groupId) is an organization, not a team")
        }
        _ = try await client.orgs.teamDelete(org: place.org, team: team)
    }

    public func removeContext(groupId: String, contextId: String) async throws {
        _ = try await client.apps.delete(app: contextId)
    }

    // MARK: Members

    public func listMembers(groupId: String) async throws -> [AppMember] {
        let place = locate(groupId)
        let org = try await fetchOrg(place.org)
        func name(_ account: String) -> String? { org.memberMetadata[account]?["name"]?.stringValue }
        if let team = place.team {
            return try await fetchTeam(place.org, team).members.map {
                AppMember(
                    identity: $0.account, role: GovernanceMemberRole($0.role).rawValue, name: name($0.account))
            }
        }
        return org.members.sorted { $0.key < $1.key }.map {
            AppMember(identity: $0.key, role: GovernanceMemberRole($0.value).rawValue, name: name($0.key))
        }
    }

    public func setMemberRole(groupId: String, identity: String, role: GovernanceMemberRole) async throws {
        let place = locate(groupId)
        if let team = place.team {
            let approval = try approver("setMemberRole")
            _ = try await client.orgs.teamSetMember(
                org: place.org, team: team, account: identity, role: role.teamRole.generated,
                passkey: approval)
        } else {
            let approval = try approver("setMemberRole")
            _ = try await client.orgs.setMember(
                org: place.org, account: identity, role: role.orgRole.generated, passkey: approval)
        }
    }

    public func removeMember(groupId: String, identity: String) async throws {
        let place = locate(groupId)
        if let team = place.team {
            let approval = try approver("removeMember")
            _ = try await client.orgs.teamRemoveMember(
                org: place.org, team: team, account: identity, passkey: approval)
        } else {
            let approval = try approver("removeMember")
            _ = try await client.orgs.removeMember(
                org: place.org, account: identity, passkey: approval)
        }
    }

    public func setMemberName(groupId: String, name: String) async throws {
        let place = locate(groupId)
        let org = try await fetchOrg(place.org)
        var metadata = org.memberMetadata[account]?.objectValue ?? [:]
        metadata["name"] = .string(name)
        let memberMetadata = try OpenAPIValueContainer(JSONValue.object(metadata))
        _ = try await client.orgs.memberMetadata(org: place.org, account: account, metadata: memberMetadata)
    }
}

/// A ``CalimeroAppBackend`` method that needs a passkey step-up (`412
/// CONFIRM_REQUIRED`: members, invites) was called on a backend made
/// without a passkey. (== Kotlin `PasskeyRequired`.)
public struct PasskeyRequired: Error, LocalizedError, Sendable {
    public let method: String

    public init(method: String) {
        self.method = method
    }

    public var errorDescription: String? { "\(method) needs a passkey approval; give the backend a passkey" }
}

/// `GET /apps`'s answer, as ``AppInfo``s.
struct AppList: Decodable {
    let apps: [AppInfo]
}
