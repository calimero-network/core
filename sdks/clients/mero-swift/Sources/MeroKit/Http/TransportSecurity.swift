import Foundation

/// A request was refused before it left the device: it would have carried
/// credentials or tokens over plaintext `http://` / `ws://` to a host that is
/// not local. (== Kotlin `InsecureTransportException`.)
public struct InsecureTransportError: Error, Sendable, Equatable, LocalizedError {
    /// The URL that was refused.
    public let url: String
    /// What the URL was for: "node", "cloud", "relay", "wallet", "release mirror".
    public let role: String

    public init(url: String, role: String) {
        self.url = url
        self.role = role
    }

    public var errorDescription: String? {
        "Refusing to reach the \(role) at \(url) over plaintext: use https"
            + (role == "node" ? ", or a local address (loopback, link-local, private LAN, .local)" : "")
            + ". For a development rig, set TransportSecurity.allowInsecureHTTP."
    }
}

/// Which URLs MeroKit will speak plaintext `http://` / `ws://` to.
/// (== Kotlin `TransportSecurity`.)
///
/// - A **node** may be plain http only when its host is local: loopback,
///   link-local (`169.254/16`, `fe80::/10`), a private LAN address (RFC 1918,
///   `fc00::/7`), or a `.local` / `.localhost` name. A dev node on your machine
///   or LAN keeps working; a password or token is never sent in the clear to a
///   host across the internet. A session sealed to an attested TEE
///   (``SealedURLProtocol``) is end-to-end encrypted, so its node URL is not
///   checked.
/// - The **Cloud** manager, a **relay**, the **wallet** and the **release
///   mirror** must be https, except on loopback (a dev rig on this machine,
///   or the Android emulator's host alias `10.0.2.2`).
///
/// ``allowInsecureHTTP`` lifts both rules. Development only.
public enum TransportSecurity {
    private static let flag = InsecureFlag()

    /// **Development only.** Send plain http/ws to any host. Leave it `false`
    /// in a shipped app: a credential or token sent this way can be read and
    /// replayed by anyone on the path.
    public static var allowInsecureHTTP: Bool {
        get { flag.value }
        set { flag.value = newValue }
    }

    /// Whether `url` is https/wss, or http/ws to a local host — what a login
    /// form should check before it asks for a password.
    public static func isSecureOrLocal(_ url: String) -> Bool {
        guard let comps = URLComponents(string: url), let scheme = comps.scheme?.lowercased() else { return false }
        if scheme == "https" || scheme == "wss" { return true }
        guard scheme == "http" || scheme == "ws", let host = comps.host, !host.isEmpty else { return false }
        return isLocalHost(host)
    }

    /// Throw ``InsecureTransportError`` if `url` is plain http/ws to a node
    /// host that is not local (unless ``allowInsecureHTTP``).
    public static func checkNodeURL(_ url: URL) throws {
        guard !allowInsecureHTTP, isPlaintext(url), !isLocalHost(url.host ?? "") else { return }
        throw InsecureTransportError(url: redacted(url), role: "node")
    }

    /// Throw ``InsecureTransportError`` if `url` (a Cloud, relay, wallet or
    /// mirror URL) is plain http/ws to anything but loopback (unless
    /// ``allowInsecureHTTP``).
    public static func checkServiceURL(_ url: URL, role: String) throws {
        guard !allowInsecureHTTP, isPlaintext(url), !isLoopbackHost(url.host ?? "") else { return }
        throw InsecureTransportError(url: redacted(url), role: role)
    }

    /// `localhost`, `127.0.0.0/8`, `::1`, and the Android emulator's alias for
    /// its host machine, `10.0.2.2` (kept here so both SDKs agree).
    public static func isLoopbackHost(_ host: String) -> Bool {
        let host = normalized(host)
        if host == "localhost" || host == "::1" || host == "10.0.2.2" { return true }
        return ipv4(host)?.first == 127
    }

    /// Loopback, link-local, a private LAN address, or a `.local` /
    /// `.localhost` name.
    public static func isLocalHost(_ host: String) -> Bool {
        let host = normalized(host)
        if host.isEmpty { return false }
        if isLoopbackHost(host) || host.hasSuffix(".local") || host.hasSuffix(".localhost") { return true }
        if let octets = ipv4(host) {
            let (a, b) = (octets[0], octets[1])
            return a == 10 || (a == 172 && (16...31).contains(b)) || (a == 192 && b == 168) || (a == 169 && b == 254)
        }
        guard host.contains(":"), let first = host.split(separator: ":").first, let word = UInt16(first, radix: 16)
        else { return false }
        return word & 0xffc0 == 0xfe80 || word & 0xfe00 == 0xfc00
    }

    private static func isPlaintext(_ url: URL) -> Bool {
        let scheme = url.scheme?.lowercased()
        return scheme == "http" || scheme == "ws"
    }

    /// The URL without its query or fragment, which may carry a token.
    private static func redacted(_ url: URL) -> String {
        guard var comps = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return url.host ?? "" }
        comps.query = nil
        comps.fragment = nil
        comps.user = nil
        comps.password = nil
        return comps.string ?? url.host ?? ""
    }

    private static func normalized(_ host: String) -> String {
        var host = host.lowercased()
        if host.hasPrefix("["), host.hasSuffix("]") { host = String(host.dropFirst().dropLast()) }
        if let zone = host.firstIndex(of: "%") { host = String(host[..<zone]) }
        while host.hasSuffix(".") { host.removeLast() }
        return host
    }

    private static func ipv4(_ host: String) -> [Int]? {
        let parts = host.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 4 else { return nil }
        let octets = parts.compactMap { part -> Int? in
            guard !part.isEmpty, part.count <= 3, part.allSatisfy(\.isASCII), part.allSatisfy(\.isNumber) else {
                return nil
            }
            return Int(part).flatMap { $0 <= 255 ? $0 : nil }
        }
        return octets.count == 4 ? octets : nil
    }
}

private final class InsecureFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var stored = false

    var value: Bool {
        get {
            lock.lock()
            defer { lock.unlock() }
            return stored
        }
        set {
            lock.lock()
            defer { lock.unlock() }
            stored = newValue
        }
    }
}
