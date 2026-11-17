import Combine
import Foundation
import NetworkExtension

/// Thin wrapper around NETunnelProviderManager. The app never touches the Rust
/// runtime: it only writes non-secret configuration, starts and stops the
/// system tunnel, and reads the status file the extension publishes.
@MainActor
final class VPNController: ObservableObject {
    static let providerBundleIdentifier = "com.lingxifox.fuckipv6.tunnel"

    @Published private(set) var vpnStatus: NEVPNStatus = .invalid
    @Published private(set) var tunnelStatus: TunnelStatus?
    @Published private(set) var errorMessage: String?
    @Published var configuration: TunnelConfiguration = .empty

    private var manager: NETunnelProviderManager?
    /// Sendable box so the observer token can be released from a nonisolated deinit.
    private final class ObserverBox: @unchecked Sendable {
        var token: NSObjectProtocol?
    }

    private let observerBox = ObserverBox()

    init() {
        observerBox.token = NotificationCenter.default.addObserver(
            forName: .NEVPNStatusDidChange,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                self?.refreshStatus()
            }
        }
        if let stored = try? TunnelConfiguration.load() {
            configuration = stored
        }
    }

    deinit {
        if let token = observerBox.token {
            NotificationCenter.default.removeObserver(token)
        }
    }

    var isConnected: Bool {
        vpnStatus == .connected
    }

    var isBusy: Bool {
        vpnStatus == .connecting || vpnStatus == .disconnecting || vpnStatus == .reasserting
    }

    func refresh() async {
        do {
            try await loadManager()
            refreshStatus()
            reloadTunnelStatus()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func saveConfiguration() async {
        do {
            try configuration.save()
            try await loadManager()
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func connect() async {
        do {
            try configuration.save()
            try await loadManager()
            guard let manager else { return }
            try manager.connection.startVPNTunnel()
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
        refreshStatus()
    }

    func disconnect() async {
        manager?.connection.stopVPNTunnel()
        refreshStatus()
    }

    func reloadTunnelStatus() {
        tunnelStatus = try? TunnelStatus.load()
    }

    private func loadManager() async throws {
        let managers = try await NETunnelProviderManager.loadAllFromPreferences()
        let existing = managers.first { manager in
            let proto = manager.protocolConfiguration as? NETunnelProviderProtocol
            return proto?.providerBundleIdentifier == Self.providerBundleIdentifier
        }
        let manager = existing ?? NETunnelProviderManager()

        let proto = NETunnelProviderProtocol()
        proto.providerBundleIdentifier = Self.providerBundleIdentifier
        proto.serverAddress = configuration.coordinatorURL
        if let group = try? AppGroup.storageRoot() {
            proto.providerConfiguration = ["storageRoot": group.path]
        }
        manager.protocolConfiguration = proto
        manager.localizedDescription = "fuckipv6"
        manager.isEnabled = true
        try await manager.saveToPreferences()
        try await manager.loadFromPreferences()

        self.manager = manager
        refreshStatus()
    }

    private func refreshStatus() {
        vpnStatus = manager?.connection.status ?? .invalid
        reloadTunnelStatus()
    }

    // MARK: - Provider IPC

    /// Sends one versioned message to the provider and renders the reply for the
    /// debug UI. Errors are returned as text: this is a diagnostic path, not a
    /// control path.
    func sendProviderMessage(_ message: [String: Any]) async -> String {
        guard let session = manager?.connection as? NETunnelProviderSession else {
            return "no tunnel session; connect first"
        }
        guard let payload = try? JSONSerialization.data(withJSONObject: message) else {
            return "message is not encodable"
        }
        do {
            guard let reply = try await send(session: session, payload: payload) else {
                return "provider returned no reply"
            }
            guard let object = try? JSONSerialization.jsonObject(with: reply),
                  let pretty = try? JSONSerialization.data(
                      withJSONObject: object,
                      options: [.prettyPrinted, .sortedKeys]
                  )
            else {
                return String(decoding: reply, as: UTF8.self)
            }
            return String(decoding: pretty, as: UTF8.self)
        } catch {
            return error.localizedDescription
        }
    }

    private func send(session: NETunnelProviderSession, payload: Data) async throws -> Data? {
        try await withCheckedThrowingContinuation { continuation in
            do {
                try session.sendProviderMessage(payload) { reply in
                    continuation.resume(returning: reply)
                }
            } catch {
                continuation.resume(throwing: error)
            }
        }
    }
}
