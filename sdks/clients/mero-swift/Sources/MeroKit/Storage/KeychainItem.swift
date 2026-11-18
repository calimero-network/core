import Foundation

#if canImport(Security)
import Security

/// What a Keychain-backed store saw go wrong when it wrote. The store
/// protocols do not throw (a write that fails leaves this process working from
/// what it holds), so a failure is counted here and handed to `onFailure`
/// instead of being dropped.
public final class KeychainWriteFailures: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    private var last: OSStatus?
    private let onFailure: (@Sendable (OSStatus) -> Void)?

    public init(onFailure: (@Sendable (OSStatus) -> Void)? = nil) {
        self.onFailure = onFailure
    }

    /// How many writes (saves and deletes) failed.
    public var failureCount: Int {
        lock.lock(); defer { lock.unlock() }
        return count
    }

    /// The `OSStatus` of the last failed write, or `nil` when none has failed —
    /// `errSecInteractionNotAllowed` while the device is locked, for one.
    public var lastFailure: OSStatus? {
        lock.lock(); defer { lock.unlock() }
        return last
    }

    func record(_ status: OSStatus) {
        guard status != errSecSuccess else { return }
        lock.lock()
        count += 1
        last = status
        lock.unlock()
        onFailure?(status)
    }
}

/// One generic-password Keychain item.
struct KeychainItem: Sendable {
    let service: String
    let account: String
    let accessGroup: String?

    var query: [String: Any] {
        var q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        if let accessGroup { q[kSecAttrAccessGroup as String] = accessGroup }
        return q
    }

    func read() -> Data? {
        var q = query
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &item) == errSecSuccess else { return nil }
        return item as? Data
    }

    /// Update the item, or add it when there is none. Every write re-asserts
    /// `accessible`, so an item saved under an older, weaker class moves to the
    /// current one the next time it is written. An add that lost a race to
    /// another writer (`errSecDuplicateItem`) is retried as an update.
    @discardableResult
    func write(_ data: Data, accessible: CFString) -> OSStatus {
        let attributes: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: accessible,
        ]
        var status = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
        if status == errSecItemNotFound {
            var add = query
            add[kSecValueData as String] = data
            add[kSecAttrAccessible as String] = accessible
            status = SecItemAdd(add as CFDictionary, nil)
            if status == errSecDuplicateItem {
                status = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
            }
        }
        return status
    }

    /// Delete the item; no item to delete is a success.
    @discardableResult
    func delete() -> OSStatus {
        let status = SecItemDelete(query as CFDictionary)
        return status == errSecItemNotFound ? errSecSuccess : status
    }
}
#endif
