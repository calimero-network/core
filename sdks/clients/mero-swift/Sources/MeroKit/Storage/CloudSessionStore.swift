import Foundation

/// What a mobile app keeps between launches to reconnect without asking for
/// the passkey again: the cluster it talks to, whether it trusts mock
/// attestation there (a development cluster), and the session.
public struct StoredCloudSession: Codable, Sendable, Equatable {
    public var nodeURL: String
    public var nodes: [String]
    public var allowMock: Bool
    public var session: CalimeroSession

    public init(nodeURL: String, nodes: [String] = [], allowMock: Bool = false, session: CalimeroSession) {
        self.nodeURL = nodeURL
        self.nodes = nodes
        self.allowMock = allowMock
        self.session = session
    }
}

/// Where a signed-in session (and, on a development cluster, its software
/// passkey) is kept. The refresh token in it is a credential: keep it in the
/// Keychain (``KeychainCloudSessionStore``).
public protocol CloudSessionStore: Sendable {
    func load() -> StoredCloudSession?
    func save(_ session: StoredCloudSession)
    func clear()
    /// A ``SoftPasskey``'s JSON for `nodeURL`, kept for a development cluster.
    func softPasskey(for nodeURL: String) -> Data?
    func saveSoftPasskey(_ json: Data, for nodeURL: String)
}

/// A ``CloudSessionStore`` in memory: tests, previews.
public final class MemoryCloudSessionStore: CloudSessionStore, @unchecked Sendable {
    private let lock = NSLock()
    private var stored: StoredCloudSession?
    private var passkeys: [String: Data] = [:]

    public init(_ stored: StoredCloudSession? = nil) {
        self.stored = stored
    }

    public func load() -> StoredCloudSession? {
        lock.lock()
        defer { lock.unlock() }
        return stored
    }

    public func save(_ session: StoredCloudSession) {
        lock.lock()
        stored = session
        lock.unlock()
    }

    public func clear() {
        lock.lock()
        stored = nil
        lock.unlock()
    }

    public func softPasskey(for nodeURL: String) -> Data? {
        lock.lock()
        defer { lock.unlock() }
        return passkeys[nodeURL]
    }

    public func saveSoftPasskey(_ json: Data, for nodeURL: String) {
        lock.lock()
        passkeys[nodeURL] = json
        lock.unlock()
    }
}

#if canImport(Security)
import Security

/// A ``CloudSessionStore`` in the Keychain, this device only, readable after
/// the first unlock.
public struct KeychainCloudSessionStore: CloudSessionStore {
    public let service: String

    public init(service: String) {
        self.service = service
    }

    private var sessionItem: KeychainItem { KeychainItem(service: service, account: "cloud-session", accessGroup: nil) }

    private func passkeyItem(_ nodeURL: String) -> KeychainItem {
        KeychainItem(service: service, account: "soft-passkey:\(nodeURL)", accessGroup: nil)
    }

    public func load() -> StoredCloudSession? {
        sessionItem.read().flatMap { try? JSONDecoder().decode(StoredCloudSession.self, from: $0) }
    }

    public func save(_ session: StoredCloudSession) {
        guard let data = try? JSONEncoder().encode(session) else { return }
        sessionItem.write(data, accessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly)
    }

    public func clear() {
        sessionItem.delete()
    }

    public func softPasskey(for nodeURL: String) -> Data? {
        passkeyItem(nodeURL).read()
    }

    public func saveSoftPasskey(_ json: Data, for nodeURL: String) {
        passkeyItem(nodeURL).write(json, accessible: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly)
    }
}
#endif
