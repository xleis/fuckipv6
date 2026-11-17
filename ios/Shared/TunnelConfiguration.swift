import Foundation

/// Non-secret tunnel inputs. The app writes this file into the App Group
/// container; the extension reads it when the system starts the tunnel, so the
/// data plane never depends on the app staying in the foreground.
struct TunnelConfiguration: Codable, Sendable, Equatable {
    var networkName: String
    var coordinatorURL: String
    var dnsServers: [String]
    var workerThreads: Int

    static let defaultNetworkName = "mesh"

    static func load() throws -> TunnelConfiguration? {
        let url = try AppGroup.tunnelConfigurationURL()
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        let data = try Data(contentsOf: url)
        return try JSONDecoder().decode(TunnelConfiguration.self, from: data)
    }

    func save() throws {
        let url = try AppGroup.tunnelConfigurationURL()
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(self).write(to: url, options: .atomic)
    }

    static var empty: TunnelConfiguration {
        TunnelConfiguration(networkName: defaultNetworkName, coordinatorURL: "", dnsServers: [], workerThreads: 2)
    }

    var isValid: Bool {
        !networkName.isEmpty
            && (coordinatorURL.hasPrefix("ws://") || coordinatorURL.hasPrefix("wss://"))
    }
}
