import Foundation

/// Files the app and its packet tunnel extension share through the App Group
/// container. Secrets never live here; only non-secret configuration and status.
enum AppGroup {
    static let identifier = "group.com.lingxifox.fuckipv6"
    /// Keychain access group shared by both targets.
    static let keychainAccessGroup = "com.lingxifox.fuckipv6"

    enum Failure: LocalizedError {
        case missingContainer

        var errorDescription: String? {
            switch self {
            case .missingContainer:
                return "App Group \(AppGroup.identifier) is unavailable; check the app and the extension entitlements"
            }
        }
    }

    static func container() throws -> URL {
        guard let url = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: identifier) else {
            throw Failure.missingContainer
        }
        return url
    }

    /// Diagnostics only: reports the resolved container and its writability so a
    /// failure to persist configuration is distinguishable from a VPN error.
    static func describe() -> String {
        do {
            let url = try container()
            let writable = FileManager.default.isWritableFile(atPath: url.path)
            return "\(url.path) writable=\(writable)"
        } catch {
            return "unavailable: \(error.localizedDescription)"
        }
    }

    /// Storage root handed to the Rust node. Kept absolute: the node rejects
    /// relative paths and every file under it uses the existing desktop layout
    /// (networks.json, node_identity.pem, <network>/config.toml).
    static func storageRoot() throws -> URL {
        let root = try container().appendingPathComponent("fuckipv6/node", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    static func tunnelConfigurationURL() throws -> URL {
        try container().appendingPathComponent("fuckipv6/tunnel.json")
    }

    static func statusURL() throws -> URL {
        try container().appendingPathComponent("fuckipv6/status.json")
    }
}
