import Foundation

/// Status the extension publishes for the app. Written atomically, read on
/// demand: no shared mutable state between the two processes.
struct TunnelStatus: Codable, Sendable, Equatable {
    enum Phase: String, Codable, Sendable {
        case stopped
        case starting
        case running
        case stopping
        case failed
    }

    var phase: Phase
    var message: String?
    var updatedAt: Date
    var snapshotJSON: String?

    static func load() throws -> TunnelStatus? {
        let url = try AppGroup.statusURL()
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return try decoder.decode(TunnelStatus.self, from: Data(contentsOf: url))
    }

    func save() throws {
        let url = try AppGroup.statusURL()
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(self).write(to: url, options: .atomic)
    }
}
