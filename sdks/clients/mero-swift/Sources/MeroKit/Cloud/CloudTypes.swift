import Foundation

// What a Calimero 1.0 node answers, as `CalimeroClient` returns it. Port of
// the types in mero-js `cloud-v2/client.ts`. A type mero-js leaves as
// `Record<string, unknown>` is a `JSONValue` here.

/// A signed-in session: a passkey session token (`v2.…`, 15 minutes) and its
/// refresh token (`rt_…`, 30 days, used once). (== mero-js `CalimeroSession`.)
public struct CalimeroSession: Codable, Sendable, Equatable {
    public var accountId: String
    public var sessionToken: String
    /// Unix seconds.
    public var expiresAt: Double
    public var refreshToken: String

    public init(accountId: String, sessionToken: String, expiresAt: Double, refreshToken: String) {
        self.accountId = accountId
        self.sessionToken = sessionToken
        self.expiresAt = expiresAt
        self.refreshToken = refreshToken
    }
}

/// One event a call emitted. (== mero-js `CallResult.events[]`.)
public struct CallEvent: Codable, Sendable, Equatable {
    public var kind: String
    public var data: JSONValue?
}

/// What `POST /apps/{app}/intents` answers. (== mero-js `CallResult`.)
public struct CallResult: Codable, Sendable, Equatable {
    public var index: Int
    public var result: JSONValue?
    public var events: [CallEvent]

    public init(index: Int, result: JSONValue?, events: [CallEvent] = []) {
        self.index = index
        self.result = result
        self.events = events
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        index = try c.decodeIfPresent(Int.self, forKey: .index) ?? 0
        result = try c.decodeIfPresent(JSONValue.self, forKey: .result)
        events = try c.decodeIfPresent([CallEvent].self, forKey: .events) ?? []
    }
}

/// What `POST /apps/{app}/views` answers. (== mero-js `ViewResult`.)
public struct ViewResult: Codable, Sendable, Equatable {
    public var index: Int
    public var result: JSONValue?

    public init(index: Int, result: JSONValue?) {
        self.index = index
        self.result = result
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        index = try c.decodeIfPresent(Int.self, forKey: .index) ?? 0
        result = try c.decodeIfPresent(JSONValue.self, forKey: .result)
    }
}

/// One server-sent event: `id` (`index:seq` for an app's events), `kind`
/// (the event's name) and `data`, parsed as JSON when it is. (== mero-js `CloudEvent`.)
public struct CloudEvent: Sendable, Equatable {
    public var id: String
    public var kind: String
    public var data: JSONValue

    public init(id: String, kind: String, data: JSONValue) {
        self.id = id
        self.kind = kind
        self.data = data
    }
}

/// What installing a package release answers. (== mero-js `PackageInstalled`.)
public struct PackageInstalled: Codable, Sendable, Equatable {
    public var package: String
    public var version: String
    public var signerId: String
    /// Service name (`""` for a single-service bundle) → the bundle hash it runs as.
    public var services: [String: String]
}

/// An installed package and its releases. (== mero-js `PackageInfo`.)
public struct PackageInfo: Codable, Sendable, Equatable {
    public struct Version: Codable, Sendable, Equatable {
        public var version: String
        public var mpk: String?
        public var size: Int?
        /// `registry` or `upload`.
        public var source: String?
        public var services: [String: String]?
        public var metadata: JSONValue?
        public var outbound: [String]?
        public var installedBy: String?
        public var installedAt: Double?
    }

    public var package: String
    public var signerId: String
    public var owner: String?
    public var latest: String?
    public var versions: [Version]
}

/// One app instance as `listApps` answers it. (== mero-js `AppInfo`.)
public struct AppInfo: Codable, Sendable, Equatable, Identifiable {
    public var appId: String
    public var name: String
    public var owner: String
    public var org: String?
    public var bundle: String
    public var package: String?
    public var version: String?
    /// The deployment it was created from, if any.
    public var deployment: String?

    public var id: String { appId }

    public init(
        appId: String, name: String, owner: String, org: String? = nil, bundle: String = "", package: String? = nil,
        version: String? = nil, deployment: String? = nil
    ) {
        self.appId = appId
        self.name = name
        self.owner = owner
        self.org = org
        self.bundle = bundle
        self.package = package
        self.version = version
        self.deployment = deployment
    }
}

/// An organization-wide upgrade run. (== mero-js `UpgradeRun`.)
public struct UpgradeRun: Codable, Sendable, Equatable {
    public var runId: String
    public var package: String
    public var version: String
    public var team: String?
    public var cascade: Bool
    /// The account that started it, or `operator`.
    public var by: String
    public var startedAt: Double
    public var apps: [String]
    public var done: [String]
    /// App id → why its upgrade failed.
    public var failed: [String: JSONValue]
    public var finished: Bool
}

/// One of an app's blobs: `blobId` base58 (what apps print), `id` hex. (== mero-js `BlobInfo`.)
///
/// A component app's sealed file (core rc.9+, ``CalimeroClient/uploadFile(_:_:)``)
/// lists with `sealed` true and the plaintext's `sha256` (hex), for the app's
/// owner and its organization's members only; `size` is the plaintext's.
public struct BlobInfo: Codable, Sendable, Equatable {
    public var blobId: String
    public var id: String
    public var size: Int
    /// The plaintext's SHA-256 (hex): a sealed file's only.
    public var sha256: String?
    /// `true` for a component app's sealed file; absent for a blob.
    public var sealed: Bool?
    /// The upload's index in the app's log: what ``CalimeroClient/uploadFile(_:_:)`` answers.
    public var index: Int?

    public init(
        blobId: String, id: String, size: Int, sha256: String? = nil, sealed: Bool? = nil, index: Int? = nil
    ) {
        self.blobId = blobId
        self.id = id
        self.size = size
        self.sha256 = sha256
        self.sealed = sealed
        self.index = index
    }
}

/// One device present in an app; `state` hex, `nil` once retracted. (== mero-js `PresenceEntry`.)
public struct PresenceEntry: Codable, Sendable, Equatable {
    public var account: String
    public var device: String
    public var state: String?
}

/// What an organization-wide upgrade targets. (== mero-js `OrgUpgradeTarget`.)
public struct OrgUpgradeTarget: Codable, Sendable, Equatable {
    public var package: String
    public var version: String
    /// Only this team's apps (and, with `cascade`, those of the teams below it).
    public var team: String?
    public var cascade: Bool?
    public var migrate: String?

    public init(package: String, version: String, team: String? = nil, cascade: Bool? = nil, migrate: String? = nil) {
        self.package = package
        self.version = version
        self.team = team
        self.cascade = cascade
        self.migrate = migrate
    }
}

/// A member's role in an organization. (== mero-js `'guest' | 'member' | 'admin' | 'owner'`.)
public enum OrgRole: String, Codable, Sendable, CaseIterable {
    case guest, member, admin, owner
}

/// A member's row in a team. (== mero-js `'lead' | 'member' | 'readOnly'`.)
public enum TeamRole: String, Codable, Sendable, CaseIterable {
    case lead, member, readOnly
}

/// Who may join a team: `everyone` who reaches its parent (open) or by `invite` only.
public enum TeamJoin: String, Codable, Sendable {
    case everyone, invite
}

/// Who may reach an app. (== mero-js `setAccess`'s `{ kind, … }`.)
public enum AppAccess: Sendable, Equatable {
    /// Only the owner.
    case owner
    /// Members of the app's organization.
    case org
    /// The owner and these accounts.
    case accounts([String])
    /// Anyone, signed in or not.
    case `public`
    /// Members of one team of the app's organization.
    case team(String)

    public var json: JSONValue {
        switch self {
        case .owner: return ["kind": "owner"]
        case .org: return ["kind": "org"]
        case .accounts(let accounts): return ["kind": "accounts", "accounts": .array(accounts.map { .string($0) })]
        case .public: return ["kind": "public"]
        case .team(let team): return ["kind": "team", "team": .string(team)]
        }
    }

    public init?(json: JSONValue?) {
        switch json?["kind"]?.stringValue {
        case "owner": self = .owner
        case "org": self = .org
        case "public": self = .public
        case "accounts": self = .accounts((json?["accounts"]?.arrayValue ?? []).compactMap(\.stringValue))
        case "team":
            guard let team = json?["team"]?.stringValue else { return nil }
            self = .team(team)
        default: return nil
        }
    }
}

/// `GET /orgs/{org}`: the organization as the caller sees it. (== mero-js `OrgInfo`.)
public struct OrgInfo: Sendable, Equatable {
    /// One of its teams the caller sees.
    public struct Team: Sendable, Equatable {
        public var teamId: String
        public var name: String
        public var parent: String?
        public var join: TeamJoin
        /// Direct members: account → lead.
        public var members: [String: Bool]
        public var readOnly: [String]
        public var metadata: JSONValue
    }

    public var orgId: String
    public var name: String
    /// Account → role.
    public var members: [String: OrgRole]
    /// Team id → team, the ones the caller sees.
    public var teams: [String: Team]
    public var metadata: JSONValue
    /// Account → that member's metadata (a display name, a title).
    public var memberMetadata: [String: JSONValue]
    /// Accounts holding `APPROVE_RELEASES` there (owner and admins included).
    public var releaseApprovers: [String]
    /// The signer ids (`did:key`) bound to it as publisher.
    public var publishes: [String]
    /// `nil` unless a deadline stopped new work: approve `release` (or newer) to lift it.
    public var newWorkStopped: ReleaseDeadlineStop?
    /// The recovery key and any recovery under way (core rc.9+); `nil` while
    /// the organization has none.
    public var recovery: OrgRecovery?
    /// The whole answer.
    public var raw: JSONValue

    public init(json: JSONValue) {
        raw = json
        recovery = json["recovery"].flatMap(OrgRecovery.init(json:))
        orgId = json["orgId"]?.stringValue ?? ""
        name = json["name"]?.stringValue ?? ""
        members = (json["members"]?.objectValue ?? [:]).compactMapValues { $0.stringValue.flatMap(OrgRole.init) }
        teams = (json["teams"]?.objectValue ?? [:]).reduce(into: [:]) { out, entry in
            let t = entry.value
            out[entry.key] = Team(
                teamId: t["teamId"]?.stringValue ?? entry.key, name: t["name"]?.stringValue ?? "",
                parent: t["parent"]?.stringValue, join: t["join"]?.stringValue.flatMap(TeamJoin.init) ?? .invite,
                members: (t["members"]?.objectValue ?? [:]).compactMapValues(\.boolValue),
                readOnly: (t["readOnly"]?.arrayValue ?? []).compactMap(\.stringValue),
                metadata: t["metadata"] ?? .null)
        }
        metadata = json["metadata"] ?? .null
        memberMetadata = json["memberMetadata"]?.objectValue ?? [:]
        releaseApprovers = (json["releaseApprovers"]?.arrayValue ?? []).compactMap(\.stringValue)
        publishes = (json["publishes"]?.arrayValue ?? []).compactMap(\.stringValue)
        if let stop = json["newWorkStopped"], let release = stop["release"]?.stringValue {
            newWorkStopped = ReleaseDeadlineStop(release: release, after: stop["after"]?.stringValue ?? "")
        } else {
            newWorkStopped = nil
        }
    }
}

/// A passed release deadline that stopped new work.
public struct ReleaseDeadlineStop: Sendable, Equatable {
    public var release: String
    public var after: String
}

/// `GET /orgs/{org}/teams/{team}`. (mero-js answers it as a record.)
public struct TeamInfo: Sendable, Equatable {
    public struct Member: Sendable, Equatable {
        public var account: String
        public var role: TeamRole
        public var capabilities: Int
    }

    public var teamId: String
    public var name: String
    public var parent: String?
    public var join: TeamJoin
    public var members: [Member]
    /// The teams directly below it the caller sees.
    public var children: [String]
    /// The apps deployed to it.
    public var apps: [AppInfo]
    public var metadata: JSONValue
    /// The caller's own row, when it has one.
    public var myRole: TeamRole?
    /// Whether the caller is a member (directly, or by an open team's inheritance).
    public var isMember: Bool
    public var raw: JSONValue

    public init(json: JSONValue, org: String? = nil) {
        raw = json
        teamId = json["teamId"]?.stringValue ?? ""
        name = json["name"]?.stringValue ?? ""
        parent = json["parent"]?.stringValue
        join = json["join"]?.stringValue.flatMap(TeamJoin.init) ?? .invite
        members = (json["members"]?.arrayValue ?? []).compactMap { m in
            guard let account = m["account"]?.stringValue else { return nil }
            return Member(
                account: account, role: m["role"]?.stringValue.flatMap(TeamRole.init) ?? .member,
                capabilities: m["capabilities"]?.intValue ?? 0)
        }
        children = (json["children"]?.arrayValue ?? []).compactMap(\.stringValue)
        apps = (json["apps"]?.arrayValue ?? []).compactMap { a in
            guard let id = a["appId"]?.stringValue else { return nil }
            return AppInfo(appId: id, name: a["name"]?.stringValue ?? "", owner: "", org: org)
        }
        metadata = json["metadata"] ?? .null
        myRole = json["myRole"]?.stringValue.flatMap(TeamRole.init)
        isMember = json["member"]?.boolValue ?? false
    }
}

/// An organization the signed-in account is in, as `/auth/me` lists it.
public struct OrgMembership: Sendable, Equatable {
    public var orgId: String
    public var name: String
    public var role: OrgRole
}

/// `GET /auth/me`. (mero-js answers it as a record.)
public struct AccountInfo: Sendable, Equatable {
    public var accountId: String
    public var name: String?
    public var orgs: [OrgMembership]
    /// The passkey (device) this session was opened with.
    public var passkeyId: String?
    /// How the credential signed in: `session` or `apiKey`.
    public var via: String?
    /// The package the session is bound to (core rc.9+): opened on an app's
    /// frontend or on an origin its manager registered
    /// (``CalimeroClient/addPackageOrigin(_:origin:passkey:rpId:)``). `nil`
    /// for an unbound session.
    public var app: String?
    /// The registered web origin the session was opened on, when the node recorded one.
    public var origin: String?
    public var raw: JSONValue

    public init(json: JSONValue) {
        raw = json
        accountId = json["accountId"]?.stringValue ?? ""
        name = json["name"]?.stringValue
        orgs = (json["orgs"]?.arrayValue ?? []).compactMap { o in
            guard let id = o["orgId"]?.stringValue else { return nil }
            return OrgMembership(
                orgId: id, name: o["name"]?.stringValue ?? "",
                role: o["role"]?.stringValue.flatMap(OrgRole.init) ?? .member)
        }
        passkeyId = json["passkeyId"]?.stringValue
        via = json["via"]?.stringValue
        app = json["app"]?.stringValue
        origin = json["origin"]?.stringValue
    }
}

/// `GET /sign-up` (core rc.9+): who may create an account on the cluster,
/// read without a session, so an app can say "invite only" before any
/// passkey prompt. (== mero-js `SignUpPolicy`.)
public struct SignUpPolicy: Codable, Sendable, Equatable {
    /// `open` (anyone), `invite` (an organization or sign-up invite), `closed` (a sign-up invite only).
    public enum Policy: String, Codable, Sendable, Equatable {
        case open
        case invite
        case closed
    }

    public var policy: Policy
    /// Whether an organization invite lets a new person sign up (and join it).
    public var orgInvites: Bool
    /// Whether a cluster sign-up invite does (always).
    public var signUpInvites: Bool
    /// Whether a sign-up without an invite is refused.
    public var inviteRequired: Bool

    public init(policy: Policy, orgInvites: Bool, signUpInvites: Bool, inviteRequired: Bool) {
        self.policy = policy
        self.orgInvites = orgInvites
        self.signUpInvites = signUpInvites
        self.inviteRequired = inviteRequired
    }
}

/// An upgrade key's scope (core rc.10+): the one app it upgrades, and the
/// package whose signed releases it may move the app to (default: the
/// package the app runs). (== mero-js `createApiKey`'s `upgrades`.)
public struct UpgradeKeyScope: Sendable, Equatable {
    public var app: String
    public var package: String?

    public init(app: String, package: String? = nil) {
        self.app = app
        self.package = package
    }
}

/// One web origin registered for a package (core rc.9+), so a frontend its
/// managers host themselves signs people in with passkeys.
/// (== mero-js `PackageOrigin` / `PackageOriginAdded`.)
public struct PackageOrigin: Codable, Sendable, Equatable {
    /// The package; absent from a listing's rows on some nodes.
    public var package: String?
    /// `scheme://host[:port]`.
    public var origin: String
    /// The WebAuthn relying-party id passkeys made there are bound to.
    public var rpId: String
    /// The account that registered it.
    public var addedBy: String
    /// RFC 3339.
    public var addedAt: String

    public init(package: String? = nil, origin: String, rpId: String, addedBy: String, addedAt: String) {
        self.package = package
        self.origin = origin
        self.rpId = rpId
        self.addedBy = addedBy
        self.addedAt = addedAt
    }
}

/// An organization's recovery key (core rc.9+, core `Org::recovery`).
/// (== mero-js `OrgRecovery`.)
public struct OrgRecovery: Sendable, Equatable {
    /// The key's Ed25519 public half, base64url.
    public var publicKey: String
    /// When the owner set it (Unix seconds).
    public var setAt: Int
    /// A recovery under way: warn every member, the owner first, who may cancel it.
    public var pending: PendingRecovery?

    public init(publicKey: String, setAt: Int, pending: PendingRecovery? = nil) {
        self.publicKey = publicKey
        self.setAt = setAt
        self.pending = pending
    }

    init?(json: JSONValue) {
        guard let publicKey = json["publicKey"]?.stringValue else { return nil }
        self.publicKey = publicKey
        setAt = json["setAt"]?.intValue ?? 0
        pending = json["pending"].flatMap { p in
            p["account"]?.stringValue.map {
                PendingRecovery(
                    account: $0, startedAt: p["startedAt"]?.intValue ?? 0, readyAt: p["readyAt"]?.intValue ?? 0)
            }
        }
    }
}

/// When a recovery started and the earliest it may finish (Unix seconds).
/// (== mero-js `RecoveryTimes`.)
public struct RecoveryTimes: Codable, Sendable, Equatable {
    public var startedAt: Int
    public var readyAt: Int

    public init(startedAt: Int, readyAt: Int) {
        self.startedAt = startedAt
        self.readyAt = readyAt
    }
}

/// A recovery under way: the account the organization goes to, and when (Unix seconds).
public struct PendingRecovery: Sendable, Equatable {
    public var account: String
    public var startedAt: Int
    public var readyAt: Int

    public init(account: String, startedAt: Int, readyAt: Int) {
        self.account = account
        self.startedAt = startedAt
        self.readyAt = readyAt
    }
}

/// What finishing a recovery answers: the organization, and the owners it
/// had before (who left it). (== mero-js `finishRecovery`'s answer.)
public struct RecoveryFinished: Codable, Sendable, Equatable {
    public var org: String
    public var previousOwners: [String]

    public init(org: String, previousOwners: [String]) {
        self.org = org
        self.previousOwners = previousOwners
    }
}

/// How a call or a view is sent again while the node is unavailable: with
/// the same key, after a backoff that starts at `baseDelay` and doubles up to
/// `maxDelay`, no jitter, at most `retries` times. The defaults are the Rust
/// client's: 6 retries, 200 ms doubling, at most 2 s. (== mero-js `CloudRetryOptions`.)
public struct CloudRetryOptions: Sendable, Equatable {
    public var retries: Int
    /// Seconds.
    public var baseDelay: Double
    /// Seconds.
    public var maxDelay: Double

    public init(retries: Int = 6, baseDelay: Double = 0.2, maxDelay: Double = 2) {
        self.retries = retries
        self.baseDelay = baseDelay
        self.maxDelay = maxDelay
    }

    /// The backoff before retry `retry` (0 for the first). (== mero-js `retryDelayMs`.)
    public func delay(_ retry: Int) -> Double {
        min(baseDelay * pow(2, Double(retry)), maxDelay)
    }
}

/// What a member may do in an organization beyond their role, as core's
/// `caps` bits. (== mero-js `ORG_CAPABILITIES`.)
public struct OrgCapabilities: OptionSet, Sendable, Hashable {
    public let rawValue: Int

    public init(rawValue: Int) {
        self.rawValue = rawValue
    }

    public static let createApp = OrgCapabilities(rawValue: 1 << 0)
    public static let inviteMembers = OrgCapabilities(rawValue: 1 << 1)
    public static let joinOpenTeams = OrgCapabilities(rawValue: 1 << 2)
    public static let manageMembers = OrgCapabilities(rawValue: 1 << 3)
    public static let manageApplication = OrgCapabilities(rawValue: 1 << 4)
    public static let createTeam = OrgCapabilities(rawValue: 1 << 5)
    public static let deleteTeam = OrgCapabilities(rawValue: 1 << 6)
    public static let manageVisibility = OrgCapabilities(rawValue: 1 << 7)
    public static let manageMetadata = OrgCapabilities(rawValue: 1 << 8)
    public static let approveReleases = OrgCapabilities(rawValue: 1 << 10)

    /// What a member holds unless an admin says otherwise. (== mero-js `DEFAULT_ORG_CAPABILITIES`.)
    public static let `default`: OrgCapabilities = .joinOpenTeams
}
