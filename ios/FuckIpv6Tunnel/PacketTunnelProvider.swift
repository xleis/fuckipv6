import Foundation
import NetworkExtension

/// Wraps a completion handler so it can cross a Sendable boundary. The system
/// hands us non-Sendable closures; we call each of them exactly once.
final class CompletionBox<Value>: @unchecked Sendable {
    private let handler: (Value) -> Void

    init(_ handler: @escaping (Value) -> Void) {
        self.handler = handler
    }

    func call(_ value: Value) {
        handler(value)
    }
}

/// Result of an asynchronous tunnel settings application.
private final class ApplyResult: @unchecked Sendable {
    private let semaphore = DispatchSemaphore(value: 0)
    private let lock = NSLock()
    private var failure: Error?

    func complete(_ error: Error?) {
        lock.lock()
        failure = error
        lock.unlock()
        semaphore.signal()
    }

    func wait(seconds: TimeInterval) -> Error? {
        if semaphore.wait(timeout: .now() + seconds) == .timedOut {
            return Fv6Failure(status: .timeout, message: "applying tunnel network settings timed out")
        }
        lock.lock()
        defer { lock.unlock() }
        return failure
    }
}

/// Packet Tunnel Provider: hosts the real Rust node runtime (EmbeddedNodeRuntime
/// behind the C ABI) and bridges the iOS packet flow to it.
///
/// Ownership: the provider owns the runtime handle, the packet endpoint handed
/// over by the open-network callback, the pump that moves packets, and the
/// system network settings. Teardown always stops the pump before releasing the
/// endpoint so no callback can outlive the handle it references.
final class PacketTunnelProvider: NEPacketTunnelProvider, PacketTunnelHost, @unchecked Sendable {
    private let state = NSLock()
    private var node: Fv6Node?
    private var endpoint: PacketEndpoint?
    private var pump: PacketPump?
    private var appliedSettings: TunnelSettings?
    /// Bumped for every open-network request so a late callback from an older
    /// session can never overwrite the endpoint of a newer one.
    private var session: UInt64 = 0

    private lazy var router = MessageRouter { [weak self] in
        self?.currentNode()
    }

    override func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {
        let box = CompletionBox(completionHandler)
        Thread.detachNewThread { [self] in
            do {
                try startNode()
                box.call(nil)
            } catch {
                publish(.failed, message: error.localizedDescription)
                teardownNode()
                box.call(error)
            }
        }
    }

    override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
        let box = CompletionBox(completionHandler)
        Thread.detachNewThread { [self] in
            publish(.stopping, message: "stop reason \(reason.rawValue)")
            teardownNode()
            publish(.stopped, message: nil)
            box.call(())
        }
    }

    override func handleAppMessage(_ messageData: Data, completionHandler: ((Data?) -> Void)?) {
        completionHandler?(router.handle(messageData))
    }

    // MARK: - Node lifecycle

    private func startNode() throws {
        publish(.starting, message: "loading configuration")

        let storageRoot = try AppGroup.storageRoot()
        guard let configuration = try TunnelConfiguration.load() else {
            throw Fv6Failure(status: .invalidArgument, message: "no tunnel configuration; save one from the app first")
        }
        let networkConfig = storageRoot
            .appendingPathComponent(configuration.networkName, isDirectory: true)
            .appendingPathComponent("config.toml")

        // The identity must be back on disk before anything can create a new
        // one, otherwise a fresh key would replace the stored identity.
        do {
            if try IdentityVault().restore(networkName: configuration.networkName, into: storageRoot) {
                publish(.starting, message: "restored node identity from keychain")
            }
        } catch {
            NSLog("fuckipv6: restoring the node identity failed: \(error.localizedDescription)")
        }

        if !FileManager.default.fileExists(atPath: networkConfig.path) {
            publish(.starting, message: "creating network configuration")
            try Fv6Node.prepareNetwork(
                storageRoot: storageRoot,
                networkName: configuration.networkName,
                coordinatorURL: configuration.coordinatorURL,
                makeDefault: true
            )
        }

        publish(.starting, message: "starting node")
        let node = try Fv6Node(
            configuration: Fv6Node.Configuration(
                storageRoot: storageRoot,
                dnsServers: configuration.dnsServers,
                workerThreads: configuration.workerThreads
            ),
            host: self
        )
        state.lock()
        self.node = node
        state.unlock()

        do {
            try node.start()
        } catch {
            teardownNode()
            throw error
        }

        var snapshotJSON: String?
        if let snapshot = try? node.snapshotJSON() {
            snapshotJSON = String(decoding: snapshot, as: UTF8.self)
        }
        publish(.running, message: nil, snapshotJSON: snapshotJSON)
    }

    /// Stops every resource in dependency order: pump, node, endpoint, settings,
    /// then the plaintext identity. Safe to call repeatedly and from any thread.
    private func teardownNode() {
        state.lock()
        let node = self.node
        let endpoint = self.endpoint
        let pump = self.pump
        let settings = self.appliedSettings
        self.node = nil
        self.endpoint = nil
        self.pump = nil
        self.appliedSettings = nil
        state.unlock()

        if let pump, !pump.stop(timeout: 5) {
            NSLog("fuckipv6: packet pump did not drain in 5s")
        }
        if let node {
            do {
                try node.stop()
            } catch {
                NSLog("fuckipv6: node stop failed: \(error.localizedDescription)")
            }
            node.destroy()
        }
        endpoint?.release()
        if settings != nil {
            clearNetworkSettings()
        }
        captureIdentity()
    }

    private func captureIdentity() {
        guard let configuration = try? TunnelConfiguration.load(),
              let storageRoot = try? AppGroup.storageRoot()
        else {
            return
        }
        do {
            if try IdentityVault().capture(networkName: configuration.networkName, from: storageRoot) {
                NSLog("fuckipv6: node identity moved to the keychain")
            }
        } catch {
            NSLog("fuckipv6: capturing the node identity failed: \(error.localizedDescription)")
        }
    }

    private func clearNetworkSettings() {
        let result = ApplyResult()
        setTunnelNetworkSettings(nil) { error in
            result.complete(error)
        }
        if let error = result.wait(seconds: 10) {
            NSLog("fuckipv6: clearing network settings failed: \(error.localizedDescription)")
        }
    }

    // MARK: - PacketTunnelHost

    func openNetwork(settingsJSON: Data, endpoint pointer: OpaquePointer) -> Int32 {
        do {
            let settings = try JSONDecoder().decode(TunnelSettings.self, from: settingsJSON)
            let networkSettings = try settings.makeNetworkSettings()

            state.lock()
            session &+= 1
            let request = session
            state.unlock()

            let result = ApplyResult()
            setTunnelNetworkSettings(networkSettings) { error in
                result.complete(error)
            }
            if let error = result.wait(seconds: 20) {
                NSLog("fuckipv6: applying tunnel settings failed: \(error.localizedDescription)")
                return (error as? Fv6Failure)?.code ?? Fv6Status.io.rawValue
            }

            let endpoint = try PacketEndpoint(pointer: pointer)
            let pump = PacketPump(flow: TunnelPacketFlow(flow: packetFlow), channel: endpoint)

            state.lock()
            let isCurrent = session == request
            var previous: PacketEndpoint?
            var previousPump: PacketPump?
            if isCurrent {
                previous = self.endpoint
                previousPump = self.pump
                self.endpoint = endpoint
                self.pump = pump
                self.appliedSettings = settings
            }
            state.unlock()

            guard isCurrent else {
                // A newer session already owns the tunnel: this endpoint is
                // stale, so drop it instead of leaking the handle.
                endpoint.release()
                return Fv6Status.state.rawValue
            }

            if let previousPump, !previousPump.stop(timeout: 5) {
                NSLog("fuckipv6: previous packet pump did not drain in 5s")
            }
            previous?.release()
            pump.start()

            NSLog("fuckipv6: tunnel configured network=\(settings.network) mtu=\(settings.mtu) transport=\(settings.transport)")
            return Fv6Status.ok.rawValue
        } catch let failure as Fv6Failure {
            return failure.code
        } catch {
            return Fv6Status.invalidArgument.rawValue
        }
    }

    func closeNetwork() -> Int32 {
        state.lock()
        let endpoint = self.endpoint
        let pump = self.pump
        let hadSettings = self.appliedSettings != nil
        self.endpoint = nil
        self.pump = nil
        self.appliedSettings = nil
        state.unlock()

        if let pump, !pump.stop(timeout: 5) {
            NSLog("fuckipv6: packet pump did not drain in 5s")
        }
        endpoint?.release()
        if hadSettings {
            clearNetworkSettings()
        }
        return Fv6Status.ok.rawValue
    }

    // MARK: - Helpers

    private func currentNode() -> (any NodeControlling)? {
        state.lock()
        defer { state.unlock() }
        return node
    }

    private func publish(_ phase: TunnelStatus.Phase, message: String?, snapshotJSON: String? = nil) {
        let status = TunnelStatus(
            phase: phase,
            message: message,
            updatedAt: Date(),
            snapshotJSON: snapshotJSON
        )
        do {
            try status.save()
        } catch {
            NSLog("fuckipv6: publishing status failed: \(error.localizedDescription)")
        }
    }
}
