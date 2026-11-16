import Foundation

/// Everything an app's screens need from a signed-in Calimero account.
/// (== Kotlin `com.calimero.mero.app.MeroAppBackend`.)
///
/// The member names predate Calimero 1.0 and are kept so the apps built on
/// this layer port mechanically. Each maps one to one onto a 1.0 concept:
///
/// | This protocol | Calimero 1.0 |
/// | --- | --- |
/// | namespace (`namespaceId`) | organization (`orgId`) |
/// | subgroup (`groupId`) | team: `"{orgId}/{teamId}"` (``teamGroupId(org:team:)``); an org id alone is the org itself |
/// | context (`contextId`) | app instance (`appId`) |
/// | application (`applicationId`) | package (`com.example.app`; `package#service` for one service), or a bundle hash on a dev cluster |
/// | invitation | an org invite token (`inv_…`), ``OrgInvitation`` |
/// | relay | the cluster's node; there is no relay |
///
/// `MeroKitUI`'s `CalimeroAppBackend` is the real one, and `MeroAppSession`
/// builds it from the signed-in client. View models take this protocol, so a
/// test hands them `MeroKitTestSupport`'s `InMemoryAppBackend` instead.
public protocol MeroAppBackend: Sendable {
    /// The account, 64 hex.
    var account: String { get }
    /// The contract in any app instance: views for reads, intents for writes.
    /// ``SignedOutContextContract`` while signed out.
    var contract: any ContextContract { get }
    /// Whether a node serves this account. Always true once signed in on 1.0
    /// (there are no relays); kept so apps that check it still build.
    var hasRelay: Bool { get }
    /// Whether ``events(contextId:)`` streams (`GET /apps/{app}/events`). True on 1.0.
    var canSubscribe: Bool { get }

    /// The organizations this account is in that belong to this app: those
    /// founded by it (their metadata names its package) or running an
    /// instance of it. All of them when the backend knows no package.
    func listNamespaces() async throws -> [AppNamespace]
    /// The teams of an organization the account sees, and the app instances in each.
    func listSubgroups(namespaceId: String) async throws -> [AppSubgroup]
    /// The app instances directly in `groupId`: a team's (`org/team`), or,
    /// for an org id, the org-wide ones (in no team).
    func listContexts(groupId: String) async throws -> [String]
    /// Whether the account reaches the app instance, so its contract calls work.
    func isMember(contextId: String) async throws -> Bool
    /// Join an app instance. On 1.0 an open team's members inherit it, so this
    /// only checks the account reaches it (``NotFound`` when it does not).
    func joinContext(contextId: String) async throws

    /// Create an organization named `name` for this app (`POST /orgs`, its
    /// metadata naming the app's package).
    func foundNamespace(name: String) async throws -> AppFoundedNamespace
    /// Create a team in `namespaceId`, open (`join: everyone`) or
    /// restricted (`join: invite`). Returns its team id.
    func createSubgroup(namespaceId: String, name: String, isPublic: Bool) async throws -> String
    /// Create an app instance of `applicationId` (a package, or a bundle hash)
    /// in `groupId` (a team, or an org id for an org-wide one), initialised
    /// with `initArgs`. Returns its app id.
    func createContext(
        groupId: String, applicationId: String, name: String, initArgs: JSONValue
    ) async throws
        -> String
    /// Create an app instance of one `service` of a multi-service package
    /// (battleships' lobby and match). The default joins them as
    /// `package#service` for ``createContext(groupId:applicationId:name:initArgs:)``.
    /// (== Kotlin `createServiceContext`.)
    func createServiceContext(
        groupId: String, applicationId: String, service: String, name: String, initArgs: JSONValue
    ) async throws -> String

    /// Mint an invitation to an organization: an invite token (`POST /orgs/{org}/invites`, role member).
    func createInvitation(namespaceId: String) async throws -> OrgInvitation
    /// Redeem an invitation (`POST /invites/{token}/redeem`): the account joins its organization.
    func joinNamespace(namespaceId: String, invitation: OrgInvitation) async throws -> AppJoinedNamespace

    /// Live events for one app instance (`GET /apps/{app}/events`, sealed SSE).
    /// Finishes at once when ``canSubscribe`` is false.
    func events(contextId: String) -> AsyncThrowingStream<AppContextEvent, Error>

    // MARK: Names

    /// The contexts directly in `groupId`, with their names (context metadata):
    /// ``listContexts(groupId:)`` with what a screen shows.
    func listContextEntries(groupId: String) async throws -> [AppContextEntry]
    /// An org's or a team's name and metadata, or `nil` when it has neither.
    func groupMetadata(groupId: String) async throws -> AppMetadata?
    /// An app instance's name and metadata, or `nil` when it has neither.
    func contextMetadata(groupId: String, contextId: String) async throws -> AppMetadata?
    /// Rename a team (`POST /orgs/{org}/teams/{team}`), or an org (its metadata's
    /// `name`), keeping its metadata's other fields.
    func setGroupName(groupId: String, name: String) async throws
    /// Rename an app instance (its metadata's `name`), keeping its metadata's other fields.
    func setContextName(groupId: String, contextId: String, name: String) async throws

    // MARK: Blobs

    /// Store `data` as a blob of app instance `contextId` (`POST /apps/{app}/blobs`).
    func uploadBlob(contextId: String, data: Data) async throws -> AppBlob
    /// A blob's bytes, read through the app instance it belongs to.
    func getBlob(blobId: String, contextId: String) async throws -> Data

    // MARK: Leaving and deleting

    /// Leave an organization (`DELETE /orgs/{org}/members/{me}`), and with it its teams and apps.
    func leaveNamespace(namespaceId: String) async throws
    /// Leave a team (`DELETE …/teams/{team}/members/{me}`), and with it its apps.
    func leaveSubgroup(groupId: String) async throws
    /// Delete a team and what is in it. The node refuses it unless the account may.
    func deleteSubgroup(groupId: String) async throws
    /// Delete an app instance (`DELETE /apps/{app}`). The node refuses it unless the account may.
    func removeContext(groupId: String, contextId: String) async throws

    // MARK: Members

    /// The members of an org or a team, with their roles and names (the org's
    /// member metadata `name`: the app's display names).
    func listMembers(groupId: String) async throws -> [AppMember]
    /// Set a member's role in an org or a team. Admins (team leads) only.
    func setMemberRole(groupId: String, identity: String, role: GovernanceMemberRole) async throws
    /// Remove a member from an org or a team. Admins (team leads) only.
    func removeMember(groupId: String, identity: String) async throws
    /// Set the account's own name in the org (`groupId` the org or one of its
    /// teams): its member metadata's `name`, keeping the other fields.
    func setMemberName(groupId: String, name: String) async throws
}

/// What a ``MeroAppBackend`` that predates a call throws for it, naming the
/// call. `CalimeroAppBackend` and `InMemoryAppBackend` implement every
/// call. (== Kotlin `UnsupportedOperationException`.)
public struct MeroAppBackendUnsupported: Error, Equatable, Sendable {
    public let method: String

    public init(method: String) {
        self.method = method
    }
}

// Defaults, so a backend of an app's own (written before these calls) still
// conforms. Each throws ``MeroAppBackendUnsupported``.
extension MeroAppBackend {
    public func createServiceContext(
        groupId: String, applicationId: String, service: String, name: String, initArgs: JSONValue
    ) async throws -> String {
        try await createContext(
            groupId: groupId, applicationId: "\(applicationId)#\(service)", name: name, initArgs: initArgs)
    }

    public func listContextEntries(groupId: String) async throws -> [AppContextEntry] {
        throw MeroAppBackendUnsupported(method: "listContextEntries")
    }

    public func groupMetadata(groupId: String) async throws -> AppMetadata? {
        throw MeroAppBackendUnsupported(method: "groupMetadata")
    }

    public func contextMetadata(groupId: String, contextId: String) async throws -> AppMetadata? {
        throw MeroAppBackendUnsupported(method: "contextMetadata")
    }

    public func setGroupName(groupId: String, name: String) async throws {
        throw MeroAppBackendUnsupported(method: "setGroupName")
    }

    public func setContextName(groupId: String, contextId: String, name: String) async throws {
        throw MeroAppBackendUnsupported(method: "setContextName")
    }

    public func uploadBlob(contextId: String, data: Data) async throws -> AppBlob {
        throw MeroAppBackendUnsupported(method: "uploadBlob")
    }

    public func getBlob(blobId: String, contextId: String) async throws -> Data {
        throw MeroAppBackendUnsupported(method: "getBlob")
    }

    public func leaveNamespace(namespaceId: String) async throws {
        throw MeroAppBackendUnsupported(method: "leaveNamespace")
    }

    public func leaveSubgroup(groupId: String) async throws {
        throw MeroAppBackendUnsupported(method: "leaveSubgroup")
    }

    public func deleteSubgroup(groupId: String) async throws {
        throw MeroAppBackendUnsupported(method: "deleteSubgroup")
    }

    public func removeContext(groupId: String, contextId: String) async throws {
        throw MeroAppBackendUnsupported(method: "removeContext")
    }

    public func listMembers(groupId: String) async throws -> [AppMember] {
        throw MeroAppBackendUnsupported(method: "listMembers")
    }

    public func setMemberRole(groupId: String, identity: String, role: GovernanceMemberRole) async throws {
        throw MeroAppBackendUnsupported(method: "setMemberRole")
    }

    public func removeMember(groupId: String, identity: String) async throws {
        throw MeroAppBackendUnsupported(method: "removeMember")
    }

    public func setMemberName(groupId: String, name: String) async throws {
        throw MeroAppBackendUnsupported(method: "setMemberName")
    }
}

/// A context in a group, with its name. (== Kotlin `AppContextEntry`.)
public struct AppContextEntry: Identifiable, Hashable, Sendable {
    /// The context id.
    public let id: String
    /// Its name (context metadata), when it has one.
    public let name: String?

    public init(id: String, name: String?) {
        self.id = id
        self.name = name
    }
}

/// A group's, member's or context's metadata record. (== Kotlin `AppMetadata`.)
public struct AppMetadata: Hashable, Sendable {
    public let name: String?
    /// The record's other fields. Setting a name keeps them.
    public let data: [String: String]

    public init(name: String?, data: [String: String] = [:]) {
        self.name = name
        self.data = data
    }
}

/// A member of a namespace or subgroup. (== Kotlin `AppMember`.)
public struct AppMember: Identifiable, Hashable, Sendable {
    /// The member's account, 64 hex.
    public let identity: String
    /// `Admin`, `Member` or `ReadOnly` (``GovernanceMemberRole``).
    public let role: String
    /// The name the member set (member metadata), when it set one.
    public let name: String?

    public var id: String { identity }
    public var isAdmin: Bool { role == GovernanceMemberRole.admin.rawValue }

    public init(identity: String, role: String, name: String? = nil) {
        self.identity = identity
        self.role = role
        self.name = name
    }
}

/// A blob of an app instance. (== Kotlin `AppBlob`.)
public struct AppBlob: Hashable, Sendable {
    public let blobId: String
    public let size: Int

    public init(blobId: String, size: Int) {
        self.blobId = blobId
        self.size = size
    }
}

/// A namespace this account is a member of. (== Kotlin `AppNamespace`.)
public struct AppNamespace: Identifiable, Hashable, Sendable {
    /// The organization's id.
    public let id: String
    /// Its name, when it has one.
    public let name: String?
    /// The package a new app instance in it is created with (the app's own).
    public let applicationId: String
    public let memberCount: Int

    public init(id: String, name: String?, applicationId: String, memberCount: Int = 0) {
        self.id = id
        self.name = name
        self.applicationId = applicationId
        self.memberCount = memberCount
    }
}

/// A subgroup of a namespace, and the contexts in it. (== Kotlin `AppSubgroup`.)
public struct AppSubgroup: Hashable, Sendable {
    public let groupId: String
    public let name: String?
    public let contextIds: [String]

    public init(groupId: String, name: String?, contextIds: [String]) {
        self.groupId = groupId
        self.name = name
        self.contextIds = contextIds
    }
}

/// What founding a namespace (an organization) came to. (== Kotlin `AppFoundedNamespace`.)
public struct AppFoundedNamespace: Hashable, Sendable {
    /// The organization's id.
    public let namespaceId: String
    /// Always true on 1.0: every organization lives on the cluster. Kept so
    /// apps that check it still build.
    public let haEnabled: Bool
    /// Always `nil` on 1.0.
    public let haError: String?
    /// Always `nil`: this SDK throws when naming fails.
    public let nameError: String?

    public init(namespaceId: String, haEnabled: Bool = true, haError: String? = nil, nameError: String? = nil) {
        self.namespaceId = namespaceId
        self.haEnabled = haEnabled
        self.haError = haError
        self.nameError = nameError
    }
}

/// What redeeming an invitation came to. (== Kotlin `AppJoinedNamespace`.)
public struct AppJoinedNamespace: Hashable, Sendable {
    /// The organization joined.
    public let namespaceId: String
    /// True once the node committed the membership (1.0 commits it before answering).
    public let published: Bool
    /// The node that admitted the account.
    public let nodeURL: String

    public init(namespaceId: String, published: Bool = true, nodeURL: String) {
        self.namespaceId = namespaceId
        self.published = published
        self.nodeURL = nodeURL
    }

    /// The node that admitted the account; the name predates 1.0, which has no relays.
    public var relayURL: String { nodeURL }
}

/// A live event in an app instance. (== Kotlin `AppContextEvent`.)
public struct AppContextEvent: Hashable, Sendable {
    /// The app instance's id.
    public let contextId: String
    /// The event's name (the app's `#[app::event]` variant).
    public let kind: String
    /// The event's payload, when it has one.
    public let data: JSONValue?
    /// The event's id, `index:seq`, when the node gave one.
    public let id: String?

    public init(contextId: String, kind: String, data: JSONValue? = nil, id: String? = nil) {
        self.contextId = contextId
        self.kind = kind
        self.data = data
        self.id = id
    }
}

/// An invitation to an organization, as an app passes it around: the node's
/// invite ``token`` (`inv_…`, from `POST /orgs/{org}/invites`) with the
/// organization it opens. (== Kotlin `OrgInvitation`.)
///
/// In a link it travels as `base58(deflate(json))` in the `invitation` query
/// parameter, the shape 0.x links had, with the same keys:
///
/// ```json
/// {"invitation": "inv_…", "namespaceId": "<orgId>", "contextId": "<appId, optional>", "name": "<org name, optional>"}
/// ```
///
/// `invitation` was a signed group-open invitation object in 0.x; it is now
/// the token string, which is all `POST /invites/{token}/redeem` needs.
/// Decoding refuses a 0.x payload (an object there). As `Codable` it is this
/// same JSON, so an app that embeds one in a payload of its own keeps working.
public struct OrgInvitation: Codable, Hashable, Sendable, CustomStringConvertible {
    /// The organization it opens (0.x: `namespaceId`).
    public let namespaceId: String
    /// The node's invite token, `inv_…`. Secret: whoever holds it may redeem it while it has uses left.
    public let token: String
    /// The app instance to open after joining, when there is one.
    public let contextId: String?
    /// The organization's name, for the confirmation dialog. Unsigned: a hint only.
    public let name: String?

    public init(namespaceId: String, token: String, contextId: String? = nil, name: String? = nil) {
        self.namespaceId = namespaceId
        self.token = token
        self.contextId = contextId
        self.name = name
    }

    enum CodingKeys: String, CodingKey {
        case token = "invitation"
        case namespaceId, contextId, name
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let token = try c.decode(String.self, forKey: .token)
        let org = try c.decode(String.self, forKey: .namespaceId)
        guard !token.isEmpty, !org.isEmpty else {
            throw DecodingError.dataCorruptedError(forKey: .token, in: c, debugDescription: "empty invitation")
        }
        self.init(
            namespaceId: org, token: token, contextId: try c.decodeIfPresent(String.self, forKey: .contextId),
            name: try c.decodeIfPresent(String.self, forKey: .name))
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(token, forKey: .token)
        try c.encode(namespaceId, forKey: .namespaceId)
        try c.encodeIfPresent(contextId, forKey: .contextId)
        try c.encodeIfPresent(name, forKey: .name)
    }

    /// `base58(deflate(json))`: what goes in a link's `invitation` parameter.
    public func encoded() throws -> String {
        try InviteCodec.encode(self)
    }

    /// The shareable link for the app with package `slug`.
    public func link(slug: String, host: String = InviteLink.defaultHost) throws -> String {
        InviteLink.invitation(token: try encoded(), slug: slug, host: host)
    }

    /// An invitation from a pasted link or code, or `nil` (another app's
    /// link when `expectedSlug` is given, or a 0.x payload).
    public static func parse(_ input: String, expectedSlug: String? = nil) -> OrgInvitation? {
        InviteLink.token(fromPasted: input, expectedSlug: expectedSlug).flatMap(decode)
    }

    /// An invitation from its encoded form (``encoded()``), or `nil`.
    public static func decode(_ code: String) -> OrgInvitation? {
        try? InviteCodec.decode(OrgInvitation.self, from: code)
    }

    /// Without the token, which is a secret.
    public var description: String {
        "OrgInvitation(namespaceId: \(namespaceId), contextId: \(contextId ?? "nil"), name: \(name ?? "nil"))"
    }
}

/// The pre-1.0 name of ``OrgInvitation``, kept so apps that name it build.
/// Its shape changed: a signed group invitation became an invite token.
public typealias SignedGroupOpenInvitation = OrgInvitation

/// The group id of `team` in `org`: what ``AppSubgroup/groupId`` and the
/// team calls take. (== Kotlin `teamGroupId`.)
public func teamGroupId(org: String, team: String) -> String { "\(org)/\(team)" }

/// A group id read back: the organization, and the team when it names one. (== Kotlin `GroupRef`.)
public struct GroupRef: Hashable, Sendable {
    public let org: String
    public let team: String?

    public init(org: String, team: String?) {
        self.org = org
        self.team = team
    }

    /// `org` or `org/team`.
    public static func parse(_ groupId: String) -> GroupRef {
        guard let cut = groupId.firstIndex(of: "/") else { return GroupRef(org: groupId, team: nil) }
        return GroupRef(org: String(groupId[..<cut]), team: String(groupId[groupId.index(after: cut)...]))
    }
}

/// A member's role, in the words the app layer uses. On 1.0 an organization
/// member's `owner`/`admin` reads as `Admin`, `member` as `Member` and
/// `guest` as `ReadOnly`; a team row's `lead` as `Admin`, `member` as
/// `Member`, `readOnly` as `ReadOnly`. (== Kotlin `GovernanceMemberRole`.)
public enum GovernanceMemberRole: String, Sendable, CaseIterable {
    case admin = "Admin"
    case member = "Member"
    case readOnly = "ReadOnly"

    public init(_ role: OrgRole) {
        switch role {
        case .owner, .admin: self = .admin
        case .member: self = .member
        case .guest: self = .readOnly
        }
    }

    public init(_ role: TeamRole) {
        switch role {
        case .lead: self = .admin
        case .member: self = .member
        case .readOnly: self = .readOnly
        }
    }

    /// The organization role this sets.
    public var orgRole: OrgRole {
        switch self {
        case .admin: return .admin
        case .member: return .member
        case .readOnly: return .guest
        }
    }

    /// The team row this sets.
    public var teamRole: TeamRole {
        switch self {
        case .admin: return .lead
        case .member: return .member
        case .readOnly: return .readOnly
        }
    }
}
