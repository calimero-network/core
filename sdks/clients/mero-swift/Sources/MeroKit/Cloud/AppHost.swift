import CryptoKit
import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// Where a package's frontend is served: `https://{label}.apps.calimero.network/`.
/// (== core `app_host::label`, mdma `mdma_common.app_host`.)
///
/// `label` is the unpadded lowercase base32 of
/// `SHA-256("calimero.app-host.v1\0" + package)`. A package on one cluster
/// in particular is at `{clusterLabel}`, the same encoding of
/// `SHA-256("calimero.app-host.cluster.v1\0" + cluster + "\0" + package)`.
public enum AppHost {
    /// The apps domain every Calimero Cloud cluster shares.
    public static let defaultDomain = "apps.calimero.network"

    /// The DNS label of `package`'s frontend origin (its primary cluster).
    public static func label(package: String) -> String {
        encode(SHA256.hash(data: Data("calimero.app-host.v1\0\(package)".utf8)))
    }

    /// The DNS label of `package`'s frontend origin on the cluster named `cluster`.
    public static func clusterLabel(cluster: String, package: String) -> String {
        encode(SHA256.hash(data: Data("calimero.app-host.cluster.v1\0\(cluster)\0\(package)".utf8)))
    }

    /// `https://{label}.{domain}`: on `cluster` when one is named, else on the package's primary cluster.
    public static func origin(package: String, cluster: String? = nil, domain: String = defaultDomain) -> URL {
        let label = cluster.map { clusterLabel(cluster: $0, package: package) } ?? label(package: package)
        return URL(string: "https://\(label).\(domain)")!
    }

    /// The newest release whose frontend `origin` serves, from its
    /// `/.well-known/calimero.json` (releases oldest first).
    public static func latestVersion(origin: URL, session: URLSession = .shared) async throws -> String {
        let url = origin.appendingPathComponent(".well-known/calimero.json")
        let (data, response) = try await session.data(from: url)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            let status = (response as? HTTPURLResponse)?.statusCode ?? 0
            throw AccountError.protocolViolation(
                "\(url.host ?? "the app host") answered \(status): no frontend served there")
        }
        struct Info: Decodable { let versions: [String] }
        guard let latest = try JSONDecoder().decode(Info.self, from: data).versions.last else {
            throw AccountError.protocolViolation("the app host serves no release")
        }
        return latest
    }

    private static let alphabet = Array("abcdefghijklmnopqrstuvwxyz234567")

    private static func encode<D: Sequence>(_ digest: D) -> String where D.Element == UInt8 {
        var out = ""
        var acc: UInt32 = 0
        var bits = 0
        for byte in digest {
            acc = (acc << 8) | UInt32(byte)
            bits += 8
            while bits >= 5 {
                bits -= 5
                out.append(alphabet[Int((acc >> UInt32(bits)) & 31)])
            }
        }
        if bits > 0 { out.append(alphabet[Int((acc << UInt32(5 - bits)) & 31)]) }
        return out
    }
}
