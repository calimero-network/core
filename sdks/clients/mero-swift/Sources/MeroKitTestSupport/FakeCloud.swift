import Foundation
import MeroKit

// swiftlint:disable large_tuple cyclomatic_complexity function_body_length

/// A small Calimero 1.0 cluster in memory, to put behind a ``FakeCloudNode``:
/// one signed-in account (``me``), its orgs, teams, app instances and
/// invites, with the routes `CalimeroAppBackend` and `MeroAppSession` use. An
/// app's views and intents are answered by ``contract``.
///
/// ```swift
/// let cloud = FakeCloud()
/// FakeCloudNode { cloud.handle($0) }.install()
/// ```
public final class FakeCloud: @unchecked Sendable {
    /// The signed-in account.
    public static let me = "acc_me"
    /// Answers `POST /apps/{app}/views` and `/intents`: the method's result.
    public var contract: @Sendable (_ app: String, _ method: String, _ args: JSONValue) -> JSONValue = { _, _, _ in
        .null
    }
    private let lock = NSLock()
    public var orgs:
        [String: (name: String, metadata: JSONValue, members: [String: String], memberMeta: [String: JSONValue])] =
            [:]
    public var teams: [String: (org: String, name: String, join: String, members: [String: String])] = [:]
    public var apps: [String: (name: String, org: String?, team: String?, package: String)] = [:]
    private var next = 0
    /// How many `412 CONFIRM_REQUIRED` step-ups the cluster asked for.
    public private(set) var stepUps = 0

    public init() {}

    func id(_ prefix: String) -> String {
        next += 1
        return "\(prefix)\(next)"
    }

    /// Answer one request as the cluster.
    public func handle(_ request: FakeCloudRequest) -> FakeCloudAnswer {
        lock.lock()
        defer { lock.unlock() }
        let parts = request.path.split(separator: "?")[0].split(separator: "/").map(String.init)
        let body = request.json ?? [:]
        func on(_ method: String, _ pattern: String) -> [String]? {
            let want = pattern.split(separator: "/").map(String.init)
            guard request.method == method, want.count == parts.count else { return nil }
            var wild: [String] = []
            for (w, p) in zip(want, parts) {
                if w == "*" { wild.append(p) } else if w != p { return nil }
            }
            return wild
        }
        // Core's passkey step-ups (412 CONFIRM_REQUIRED, then the same body
        // with `confirm`): memberships and invite creation.
        let stepUp =
            on("POST", "orgs/*/members") != nil || on("DELETE", "orgs/*/members/*") != nil
            || on("POST", "orgs/*/invites") != nil || on("POST", "orgs/*/teams/*/members") != nil
            || on("DELETE", "orgs/*/teams/*/members/*") != nil || on("POST", "orgs/*/capabilities") != nil
        if stepUp, body["confirm"] == nil {
            stepUps += 1
            return .error(
                412, "CONFIRM_REQUIRED",
                details: ["challengeId": "su", "challenge": "AAAA", "rpId": "localhost"])
        }
        let w: [String]
        switch true {
        case on("POST", "auth/passkey/register/begin") != nil:
            return .json([
                "challengeId": "c",
                "options": [
                    "challenge": "AAAA", "rp": ["id": "localhost", "name": "Calimero"],
                    "user": ["id": "AQID", "name": "dev", "displayName": "dev"],
                ],
            ])
        case on("POST", "auth/passkey/register/finish") != nil:
            return .json([
                "accountId": .string(Self.me), "sessionToken": "v2.s", "expiresAt": 1_900_000_000,
                "refreshToken": "rt_s",
            ])
        case on("POST", "apps/*/views") != nil || on("POST", "apps/*/intents") != nil:
            let app = parts[1]
            let method = body["method"]?.stringValue ?? ""
            return .json(["index": 1, "result": contract(app, method, body["args"] ?? [:]), "events": []])
        case on("GET", "apps/*/events") != nil:
            return .events([], holdOpen: true)
        case on("POST", "auth/passkey/login/begin") != nil:
            return .json(["challengeId": "c", "options": ["challenge": "AAAA", "rpId": "localhost"]])
        case on("POST", "auth/passkey/login/finish") != nil || on("POST", "auth/refresh") != nil:
            return .json([
                "accountId": .string(Self.me), "sessionToken": "v2.s", "expiresAt": 1_900_000_000,
                "refreshToken": "rt_s",
            ])
        case on("POST", "auth/logout") != nil:
            return .json([:])
        case on("GET", "auth/me") != nil:
            let mine = orgs.filter { $0.value.members[Self.me] != nil }.sorted { $0.key < $1.key }
            return .json([
                "accountId": .string(Self.me), "name": "dev", "passkeys": [], "via": "session", "passkeyId": .null,
                "approvedReleases": [], "outboundApprovals": [:],
                "orgs": .array(
                    mine.map {
                        [
                            "orgId": .string($0.key), "name": .string($0.value.name),
                            "role": .string($0.value.members[Self.me]!),
                        ]
                    }),
            ])
        case on("POST", "orgs") != nil:
            let org = id("org")
            orgs[org] = (body["name"]?.stringValue ?? "", .null, [Self.me: "owner"], [:])
            return .json(["orgId": .string(org)])
        case on("GET", "orgs/*") != nil:
            w = on("GET", "orgs/*")!
            let org = w[0]
            guard let o = orgs[org] else { return .error(404, "NOT_FOUND") }
            var teamJSON: [String: JSONValue] = [:]
            for (tid, t) in teams where t.org == org {
                teamJSON[tid] = [
                    "teamId": .string(tid), "name": .string(t.name), "join": .string(t.join), "parent": .null,
                    "members": .object(t.members.mapValues { .bool($0 == "lead") }), "readOnly": [],
                    "capabilities": [:], "metadata": .null, "removed": [],
                ]
            }
            return .json([
                "orgId": .string(org), "name": .string(o.name), "metadata": o.metadata,
                "members": .object(o.members.mapValues { .string($0) }), "teams": .object(teamJSON),
                "memberMetadata": .object(o.memberMeta), "defaultCapabilities": 4, "capabilities": [:],
                "approvedReleases": [], "outboundApprovals": [:], "upgradeRuns": [:],
            ])
        case on("POST", "orgs/*/metadata") != nil:
            w = on("POST", "orgs/*/metadata")!
            let org = w[0]
            orgs[org]?.metadata = body["metadata"] ?? .null
            return .json([:])
        case on("POST", "orgs/*/members/*/metadata") != nil:
            w = on("POST", "orgs/*/members/*/metadata")!
            let org = w[0]
            let account = w[1]
            orgs[org]?.memberMeta[account] = body["metadata"]
            return .json([:])
        case on("POST", "orgs/*/members") != nil:
            w = on("POST", "orgs/*/members")!
            let org = w[0]
            orgs[org]?.members[body["account"]?.stringValue ?? ""] = body["role"]?.stringValue
            return .json([:])
        case on("DELETE", "orgs/*/members/*") != nil:
            w = on("DELETE", "orgs/*/members/*")!
            let org = w[0]
            let account = w[1]
            orgs[org]?.members[account] = nil
            return .json([:])
        case on("POST", "orgs/*/teams") != nil:
            w = on("POST", "orgs/*/teams")!
            let org = w[0]
            let team = id("team")
            teams[team] = (
                org, body["name"]?.stringValue ?? "", body["join"]?.stringValue ?? "invite", [Self.me: "lead"]
            )
            return .json(["teamId": .string(team)])
        case on("GET", "orgs/*/teams/*") != nil:
            w = on("GET", "orgs/*/teams/*")!
            let team = w[1]
            guard let t = teams[team] else { return .error(404, "NOT_FOUND") }
            return .json([
                "teamId": .string(team), "name": .string(t.name), "join": .string(t.join), "parent": .null,
                "metadata": .null, "children": [], "myRole": .string(t.members[Self.me] ?? "member"),
                "member": .bool(t.members[Self.me] != nil),
                "members": .array(
                    t.members.map { entry -> JSONValue in
                        ["account": .string(entry.key), "role": .string(entry.value), "capabilities": 0]
                    }),
                "apps": .array(
                    apps.filter { $0.value.team == team }.sorted { $0.key < $1.key }.map {
                        ["appId": .string($0.key), "name": .string($0.value.name), "contextId": .string($0.key)]
                    }),
            ])
        case on("POST", "orgs/*/invites") != nil:
            w = on("POST", "orgs/*/invites")!
            let org = w[0]
            return .json(["token": .string("inv_\(org)")])
        case on("POST", "invites/*/redeem") != nil:
            w = on("POST", "invites/*/redeem")!
            let token = w[0]
            let org = String(token.dropFirst(4))
            orgs[org]?.members[Self.me] = "member"
            return .json(["org": .string(org), "orgId": .string(org)])
        case on("POST", "apps") != nil:
            let app = id("app")
            let access = body["access"]
            apps[app] = (
                body["name"]?.stringValue ?? "", body["org"]?.stringValue,
                access?["kind"] == "team" ? access?["team"]?.stringValue : nil, body["package"]?.stringValue ?? ""
            )
            return .json([
                "appId": .string(app), "contextId": .string(app), "group": 1, "replicas": [1], "compute": false,
            ])
        case on("GET", "apps") != nil:
            let package = request.path.contains("package=") ? request.path.components(separatedBy: "package=")[1] : nil
            return .json([
                "apps": .array(
                    apps.filter { package == nil || $0.value.package == package }.sorted { $0.key < $1.key }.map {
                        Self.app(
                            $0.key, name: $0.value.name, org: $0.value.org, team: $0.value.team,
                            package: $0.value.package)
                    })
            ])
        case on("GET", "apps/*") != nil:
            w = on("GET", "apps/*")!
            let app = w[0]
            guard let a = apps[app] else { return .error(404, "NOT_FOUND") }
            return .json(Self.app(app, name: a.name, org: a.org, team: a.team, package: a.package))
        default:
            return .error(404, "NOT_FOUND", message: "no route \(request.method) \(request.path)")
        }
    }
}

// swiftlint:enable large_tuple cyclomatic_complexity function_body_length

extension FakeCloud {
    /// An app instance as `GET /apps` and `GET /apps/{app}` answer it (the document's `App`).
    static func app(_ id: String, name: String, org: String?, team: String?, package: String) -> JSONValue {
        [
            "appId": .string(id), "name": .string(name), "group": 1, "bundle": "b", "owner": .string(me),
            "org": org.map { .string($0) } ?? .null,
            "access": team.map { ["kind": "team", "team": .string($0)] } ?? ["kind": "org"],
            "confirmMethods": [], "replicas": [1], "createdAt": 1, "initArgs": [:], "metadata": .null,
            "release": ["package": .string(package), "version": "1.0.0"], "package": .string(package),
        ]
    }
}
