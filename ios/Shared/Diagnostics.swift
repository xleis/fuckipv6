import Foundation

/// Append-only diagnostics inside the App Group container.
///
/// The P3 build has no console channel back to the development host, so
/// on-device failures are recorded in `Library/Logs/fuckipv6/app.log` and
/// `xcrun devicectl device copy from`. Writes are best effort: a failure here
/// must never mask the error being reported.
enum Diagnostics {
    private static let queue = DispatchQueue(label: "com.lingxifox.fuckipv6.diagnostics")

    static func record(_ message: String) {
        let line = "\(ISO8601DateFormatter().string(from: Date())) \(message)\n"
        queue.async {
            guard let base = try? AppGroup.container() else { return }
            // Library/Logs is also reachable from the development host with
            // `xcrun devicectl device copy from`; the group container root is not.
            let directory = base
                .appendingPathComponent("Library/Logs", isDirectory: true)
                .appendingPathComponent("fuckipv6", isDirectory: true)
            try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            let url = directory.appendingPathComponent("app.log")
            guard let data = line.data(using: .utf8) else { return }
            if let handle = try? FileHandle(forWritingTo: url) {
                defer { try? handle.close() }
                _ = try? handle.seekToEnd()
                try? handle.write(contentsOf: data)
            } else {
                try? data.write(to: url, options: .atomic)
            }
        }
    }
}
