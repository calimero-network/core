import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

// The Calimero Cloud node directory: which nodes of a cluster take traffic
// now, and in what order this client tries them. Port of mero-js
// `cloud-v2/directory.ts`.
//
// `GET {directory}/{cluster}` (no auth) answers
// `{ cluster, nodes: [{ id, url, zone, weight }], refreshAfterSeconds }`. The
// directory is not trusted for security: it only picks which nodes to try.
// Every node is attested under the caller's own policy before anything is
// sent to it, so a bad list can only make connecting fail.

/// The Calimero Cloud control plane's public node directory. (== mero-js `DEFAULT_DIRECTORY`.)
public let defaultDirectory = "https://manager.cloud.calimero.network/api/directory"

/// How long a listing stands when the directory does not say.
let defaultDirectoryRefreshSeconds: Double = 60

/// Why ``DirectoryError`` was thrown. (== mero-js `DirectoryFailure`.)
public enum DirectoryFailure: String, Sendable {
    /// The directory could not be reached, or answered an error.
    case unreachable
    /// The directory does not know the cluster (a 404).
    case notFound = "not-found"
    /// It answered something that is not a listing.
    case badResponse = "bad-response"
    /// It listed no usable node.
    case empty
}

/// The directory could not name a node to try. (== mero-js `DirectoryError`.)
public struct DirectoryError: Error, LocalizedError, CustomStringConvertible, Sendable {
    public let cluster: String
    public let reason: DirectoryFailure
    public let message: String
    public let status: Int?

    public init(cluster: String, reason: DirectoryFailure, message: String, status: Int? = nil) {
        self.cluster = cluster
        self.reason = reason
        self.message = message
        self.status = status
    }

    public var errorDescription: String? { "directory for cluster \(cluster): \(message)" }
    public var description: String { errorDescription ?? message }
}

/// One usable node of a listing. (== mero-js `DirectoryNode`.)
public struct DirectoryNode: Sendable, Equatable {
    public var url: String
    public var zone: String?
    public var weight: Int

    public init(url: String, zone: String? = nil, weight: Int) {
        self.url = url
        self.zone = zone
        self.weight = weight
    }
}

/// A cluster's listing. (== mero-js `DirectoryListing`.)
public struct DirectoryListing: Sendable, Equatable {
    public var nodes: [DirectoryNode]
    public var refreshAfterSeconds: Double
}

/// The URL of `cluster`'s listing in `directory`. (== mero-js `directoryUrl`.)
public func directoryUrl(_ directory: String, cluster: String) -> String {
    var base = directory
    while base.hasSuffix("/") { base.removeLast() }
    return "\(base)/\(cluster.addingPercentEncoding(withAllowedCharacters: .urlPathSegment) ?? cluster)"
}

/// Read `cluster`'s listing, keeping only the entries with an http(s) URL and
/// a positive integer weight (the first of a repeated URL). (== mero-js `readDirectory`.)
public func readDirectory(
    _ directory: String, cluster: String, session: URLSession = .shared
) async throws
    -> DirectoryListing
{
    guard let url = URL(string: directoryUrl(directory, cluster: cluster)) else {
        throw DirectoryError(cluster: cluster, reason: .unreachable, message: "bad directory URL \(directory)")
    }
    var request = URLRequest(url: url)
    request.setValue("application/json", forHTTPHeaderField: "accept")
    let data: Data
    let status: Int
    do {
        let (body, response) = try await session.data(for: request)
        data = body
        status = (response as? HTTPURLResponse)?.statusCode ?? 0
    } catch {
        throw DirectoryError(cluster: cluster, reason: .unreachable, message: "could not be reached: \(error)")
    }
    if status == 404 {
        throw DirectoryError(cluster: cluster, reason: .notFound, message: "unknown cluster", status: 404)
    }
    guard (200..<300).contains(status) else {
        throw DirectoryError(cluster: cluster, reason: .unreachable, message: "answered \(status)", status: status)
    }
    let body: JSONValue
    do {
        body = try MeroJSON.decode(JSONValue.self, from: data)
    } catch {
        throw DirectoryError(
            cluster: cluster, reason: .badResponse, message: "answered something other than JSON", status: status)
    }
    guard let entries = body["nodes"]?.arrayValue else {
        throw DirectoryError(cluster: cluster, reason: .badResponse, message: "answered no node list", status: status)
    }
    var nodes: [DirectoryNode] = []
    var seen = Set<String>()
    for entry in entries {
        if let node = usableNode(entry), seen.insert(node.url).inserted { nodes.append(node) }
    }
    if nodes.isEmpty {
        throw DirectoryError(cluster: cluster, reason: .empty, message: "lists no usable node", status: status)
    }
    var refresh = defaultDirectoryRefreshSeconds
    if case .number(let after) = body["refreshAfterSeconds"], after.isFinite, after >= 1 { refresh = after }
    return DirectoryListing(nodes: nodes, refreshAfterSeconds: refresh)
}

private func usableNode(_ entry: JSONValue) -> DirectoryNode? {
    guard let raw = entry["url"]?.stringValue, case .number(let weight) = entry["weight"],
        weight.isFinite, weight >= 1, weight.rounded() == weight, weight <= Double(Int32.max),
        let parsed = URL(string: raw), let scheme = parsed.scheme?.lowercased(), scheme == "https" || scheme == "http",
        parsed.host != nil
    else { return nil }
    var url = raw
    while url.hasSuffix("/") { url.removeLast() }
    return DirectoryNode(url: url, zone: entry["zone"]?.stringValue, weight: Int(weight))
}

/// The order to try `nodes` in: those in `zone` first (when given), each
/// group in weighted-random order, a node's weight its relative chance to come
/// next. `random` answers in [0, 1). (== mero-js `orderNodes`.)
public func orderNodes(
    _ nodes: [DirectoryNode], zone: String?, random: () -> Double = { Double.random(in: 0..<1) }
) -> [String] {
    let near = zone.map { zone in nodes.filter { $0.zone == zone } } ?? []
    let far = zone.map { zone in nodes.filter { $0.zone != zone } } ?? nodes
    return weightedOrder(near, random) + weightedOrder(far, random)
}

private func weightedOrder(_ nodes: [DirectoryNode], _ random: () -> Double) -> [String] {
    var left = nodes
    var order: [String] = []
    while !left.isEmpty {
        var roll = random() * Double(left.reduce(0) { $0 + $1.weight })
        var pick = 0
        while pick < left.count - 1, roll >= Double(left[pick].weight) {
            roll -= Double(left[pick].weight)
            pick += 1
        }
        order.append(left.remove(at: pick).url)
    }
    return order
}
