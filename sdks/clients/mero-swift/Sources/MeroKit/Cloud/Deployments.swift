import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

// Blob content, xcalls, deployments and deployers (core rc.7+). Port of the
// types mero-js `cloud-v2/client.ts` added for them.

/// A blob's bytes with the content type the node serves them as. (== mero-js `BlobContent`.)
public struct BlobContent: Sendable, Equatable {
    public var data: Data
    /// What the uploader gave (component apps); `application/octet-stream` otherwise.
    public var contentType: String

    public init(data: Data, contentType: String) {
        self.data = data
        self.contentType = contentType
    }
}

/// One xcall that was never delivered (core's `DeadLetter`). (== mero-js `DeadLetter`.)
public struct DeadLetter: Codable, Sendable, Equatable {
    public var seq: Int
    /// The target app.
    public var app: String
    public var method: String
    public var reason: String
    /// The log entry's timestamp it was dead-lettered at (ms since the epoch).
    public var atMs: Int64
}

/// `GET /apps/{app}/xcalls`: a component app's outbox and its dead letters,
/// newest last. (== mero-js `AppXcalls`.)
public struct AppXcalls: Codable, Sendable, Equatable {
    /// Messages waiting in the outbox.
    public var pending: Int
    public var deadLetters: [DeadLetter]
}

/// An account or an organization, as a deployer, a deployment's owner or the
/// cluster's owner: `{ "account": id }` or `{ "org": id }`. (== mero-js `Deployer`.)
public enum Deployer: Codable, Sendable, Equatable, Hashable {
    case account(String)
    case org(String)

    enum CodingKeys: String, CodingKey { case account, org }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        if let account = try c.decodeIfPresent(String.self, forKey: .account) {
            self = .account(account)
        } else {
            self = .org(try c.decode(String.self, forKey: .org))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .account(let id): try c.encode(id, forKey: .account)
        case .org(let id): try c.encode(id, forKey: .org)
        }
    }

    /// The deployer's key, as `GET /deployers` lists it and `DELETE
    /// /deployers/{key}` takes it: `account:{id}` or `org:{id}`. (== mero-js `deployerKey`.)
    public var key: String {
        switch self {
        case .account(let id): return "account:\(id)"
        case .org(let id): return "org:\(id)"
        }
    }

    var json: JSONValue {
        switch self {
        case .account(let id): return ["account": .string(id)]
        case .org(let id): return ["org": .string(id)]
        }
    }
}

/// A deployer's key. (== mero-js `deployerKey`.)
public func deployerKey(_ deployer: Deployer) -> String { deployer.key }

/// One deployer grant, as `GET /deployers` lists it. (== mero-js `DeployerGrant`.)
public struct DeployerGrant: Codable, Sendable, Equatable {
    /// `account:{id}` or `org:{id}`.
    public var key: String
    public var deployer: Deployer?
    /// RFC 3339; `nil`: it does not end.
    public var until: String?
    public var untilSecs: Int64?
    public var grantedAt: String
    public var grantedBy: String
}

/// Who may install apps: any account, or granted deployers only.
public enum DeploymentsRule: String, Codable, Sendable {
    case open, granted
}

/// Who grants deployers: the operator (its key) or the cluster's owner (with a passkey).
public enum DeployersRule: String, Codable, Sendable {
    case `operator`, owner
}

/// `GET /deployers`: who may install apps on the cluster, and who grants it.
/// (== mero-js `DeployersInfo`.)
public struct DeployersInfo: Codable, Sendable, Equatable {
    /// Absent on a default cluster.
    public var deployments: DeploymentsRule?
    public var deployers: DeployersRule?
    public var owner: Deployer?
    public var grants: [DeployerGrant]
}

/// One app a deployment offers, as its managers read it. (== mero-js `DeploymentOffer`.)
public struct DeploymentOffer: Codable, Sendable, Equatable {
    /// The app's name in the deployment.
    public var app: String
    public var package: String
    public var version: String
    public var addedBy: String
    /// RFC 3339.
    public var addedAt: String
}

/// A deployment as its managers read it (`GET /deployments`). (== mero-js `DeploymentInfo`.)
public struct DeploymentInfo: Codable, Sendable, Equatable, Identifiable {
    public var id: String
    public var name: String
    public var owner: Deployer
    public var apps: [DeploymentOffer]
    public var createdBy: String
    /// RFC 3339.
    public var createdAt: String
}

/// `GET /deployments/{id}`, public: what a frontend and the customer's
/// consent show. Never its managers, members or customers. (== mero-js `PublicDeployment`.)
public struct PublicDeployment: Codable, Sendable, Equatable, Identifiable {
    public struct Owner: Codable, Sendable, Equatable {
        /// `account` or `org`.
        public var kind: String
        /// `nil` if it is gone.
        public var name: String?
    }

    public struct App: Codable, Sendable, Equatable {
        public var app: String
        public var package: String
        public var version: String
    }

    public var id: String
    public var name: String
    public var owner: Owner
    public var apps: [App]
}

/// `GET /deployments/{id}/orgs`: the organizations with instances created
/// from a deployment. (== mero-js `DeploymentOrgs`.)
public struct DeploymentOrgs: Codable, Sendable, Equatable {
    public struct Org: Codable, Sendable, Equatable {
        public struct App: Codable, Sendable, Equatable {
            public var app: String
            public var version: String
        }

        public var orgId: String
        public var name: String?
        public var instances: Int
        /// Which of the deployment's apps they run, at which release.
        public var apps: [App]
        /// RFC 3339: when its first instance from the deployment was created.
        public var firstCreatedAt: String
    }

    public var deployment: String
    public var orgs: [Org]
    /// Instances outside any organization, counted only.
    public var outsideOrgs: Int
}

/// A package frontend's `/.well-known/calimero.json`: the package, its
/// releases, the deployments offering it and the cluster's API URLs.
/// (== mero-js `FrontendManifest`.)
public struct FrontendManifest: Codable, Sendable, Equatable {
    public struct Deployment: Codable, Sendable, Equatable {
        public var id: String
        public var name: String
        public var app: String
        public var version: String
    }

    public var package: String
    public var versions: [String]
    public var deployments: [Deployment]
    public var api: [String]

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        package = try c.decode(String.self, forKey: .package)
        versions = try c.decodeIfPresent([String].self, forKey: .versions) ?? []
        deployments = try c.decodeIfPresent([Deployment].self, forKey: .deployments) ?? []
        api = try c.decodeIfPresent([String].self, forKey: .api) ?? []
    }
}

/// Read the `/.well-known/calimero.json` of the frontend at `origin`. Served
/// in the clear by the frontend host: for choosing a deployment and the API
/// URL to connect to, never for trust (the client attests the node it then
/// talks to). (== mero-js `frontendManifest`; there is no page origin to
/// default to on a device, so `origin` is required.)
public func frontendManifest(_ origin: String, session: URLSession = .shared) async throws -> FrontendManifest {
    guard let base = URL(string: origin), let url = URL(string: "/.well-known/calimero.json", relativeTo: base) else {
        throw AccountError.invalidInput("bad frontend origin \(origin)")
    }
    let (data, response) = try await session.data(from: url.absoluteURL)
    let status = (response as? HTTPURLResponse)?.statusCode ?? 0
    guard (200..<300).contains(status) else { throw errorFrom(status: status, data: data) }
    do {
        return try MeroJSON.decode(FrontendManifest.self, from: data)
    } catch {
        throw AccountError.protocolViolation("the frontend manifest did not decode: \(error)")
    }
}

/// The deployment a frontend page was opened for: the node serves
/// `/d/{deployment}/` by redirecting to `/{version}/…?deployment={id}`, so
/// the page creates its instances from that deployment
/// (``AppSource/package(_:version:service:deployment:)``). `nil` when the URL
/// was not opened through a deployment. (== mero-js `pageDeployment`, which
/// defaults to the page's URL.)
public func pageDeployment(_ url: URL?) -> String? {
    guard let url, let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems else { return nil }
    guard let value = items.first(where: { $0.name == "deployment" })?.value, !value.isEmpty else { return nil }
    return value
}
