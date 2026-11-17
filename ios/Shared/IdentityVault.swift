import Foundation
import Security

/// Minimal secret storage boundary. The shipping implementation is the
/// Keychain; tests inject an in-memory store.
protocol SecretStore: AnyObject {
    func load(account: String) throws -> Data?
    func save(_ data: Data, account: String) throws
    func delete(account: String) throws
}

struct SecretStoreError: LocalizedError, Sendable {
    let status: OSStatus
    let operation: String

    var errorDescription: String? {
        "keychain \(operation) failed with OSStatus \(status)"
    }
}

/// Keychain-backed store for the node identity.
///
/// Items are device-only and readable after the first unlock, so the tunnel can
/// start from the lock screen while backups and other devices never see the
/// private key. The access group is supplied by the caller: the extension uses
/// its default group, and a shared group can be passed once the prefixed team
/// identifier is known at runtime.
final class KeychainSecretStore: SecretStore {
    static let defaultService = "com.lingxifox.fuckipv6.node-identity"

    private let service: String
    private let accessGroup: String?

    init(service: String = KeychainSecretStore.defaultService, accessGroup: String? = nil) {
        self.service = service
        self.accessGroup = accessGroup
    }

    func load(account: String) throws -> Data? {
        var query = baseQuery(account: account)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecItemNotFound {
            return nil
        }
        guard status == errSecSuccess else {
            throw SecretStoreError(status: status, operation: "read")
        }
        return item as? Data
    }

    func save(_ data: Data, account: String) throws {
        let query = baseQuery(account: account)
        let attributes: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        let updated = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
        if updated == errSecSuccess {
            return
        }
        guard updated == errSecItemNotFound else {
            throw SecretStoreError(status: updated, operation: "update")
        }
        var insert = query
        insert.merge(attributes) { _, new in new }
        let status = SecItemAdd(insert as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw SecretStoreError(status: status, operation: "add")
        }
    }

    func delete(account: String) throws {
        let status = SecItemDelete(baseQuery(account: account) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw SecretStoreError(status: status, operation: "delete")
        }
    }

    private func baseQuery(account: String) -> [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        if let accessGroup {
            query[kSecAttrAccessGroup as String] = accessGroup
        }
        return query
    }
}

/// Keeps the node identity (node_identity.pem) in the Keychain and only
/// materialises it on disk while the node runs.
///
/// The Rust runtime still loads the identity from a file, so the vault moves
/// the existing PEM between the two stores without changing the identity
/// format. A crashed run can leave the plaintext file behind; the next start
/// reads it as-is, and teardown captures it again.
struct IdentityVault {
    static let fileName = "node_identity.pem"

    /// Keychain account for the PEM. The identity file lives at the storage
    /// root, not inside a network directory, so one account covers every network
    /// this node joins; keying it by network name would strand the key as soon
    /// as the node rejoins under a different name.
    static let account = "node-identity"
    private let store: any SecretStore
    private let fileManager: FileManager

    init(store: any SecretStore = KeychainSecretStore(), fileManager: FileManager = .default) {
        self.store = store
        self.fileManager = fileManager
    }

    /// Restores the identity before the node starts. Returns true when a stored
    /// key was written back to disk.
    @discardableResult
    func restore(into storageRoot: URL) throws -> Bool {
        let file = Self.identityURL(storageRoot: storageRoot)
        if fileManager.fileExists(atPath: file.path) {
            return false
        }
        guard let secret = try store.load(account: Self.account), !secret.isEmpty else {
            return false
        }
        try write(secret, to: file)
        return true
    }

    /// Captures the identity after the node stopped and removes the plaintext
    /// copy. Returns true when a key was stored.
    @discardableResult
    func capture(from storageRoot: URL) throws -> Bool {
        let file = Self.identityURL(storageRoot: storageRoot)
        guard let secret = try? Data(contentsOf: file), !secret.isEmpty else {
            return false
        }
        try store.save(secret, account: Self.account)
        try fileManager.removeItem(at: file)
        return true
    }

    static func identityURL(storageRoot: URL) -> URL {
        storageRoot.appendingPathComponent(fileName)
    }

    private func write(_ data: Data, to url: URL) throws {
        try fileManager.createDirectory(
            at: url.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try data.write(to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }
}
