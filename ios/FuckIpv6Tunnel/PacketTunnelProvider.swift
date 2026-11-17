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
    /// Bumped for every open-network request and every teardown so a late
    /// callback from an older session can never overwrite a newer one, and so a
    /// connection opened while the node is shutting down is rejected.
    private var session: UInt64 = 0
    /// Handles whose drain timed out. They stay owned by the provider so a later
    /// teardown retries the drain instead of leaking the pump or freeing an
    /// endpoint that a live thread still reads.
    private var pendingPump: PacketPump?
    private var pendingEndpoint: PacketEndpoint?

    private lazy var router = MessageRouter { [weak self] in
        self?.currentNode()
    }

    override func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {
        Diagnostics.record("startTunnel called")
        let box = CompletionBox(completionHandler)
        Thread.detachNewThread { [self] in
            do {
                try startNode()
                box.call(nil)
            } catch {
                publish(.failed, message: error.localizedDescription)
                let report = teardownNode()
                if !report.isClean {
                    Diagnostics.record("teardown after failed start: \(report.summary)")
                }
            
                box.call(error)
            }
        }
    }

    override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
        let box = CompletionBox(completionHandler)
        Thread.detachNewThread { [self] in
            publish(.stopping, message: "stop reason \(reason.rawValue)")
            var report = teardownNode()
            if !report.isClean {
                // Second chance: stopping the node usually unblocks a receive
                // that outlived the first wait.
                report = retryPendingCleanup(report, timeout: 2)
            }
            if report.isClean {
                publish(.stopped, message: nil)
            } else {
                // The system still requires this callback, but a tunnel whose
                // handles are not drained must not be reported as a clean stop.
                Diagnostics.record("stopTunnel finished with pending cleanup: \(report.summary)")
                publish(.failed, message: "cleanup incomplete: \(report.summary)")
            }
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
        // The identity must be back on disk before anything can create a new
        // one, otherwise a fresh key would replace the stored identity.
        do {
            if try IdentityVault().restore(networkName: configuration.networkName, into: storageRoot) {
                publish(.starting, message: "restored node identity from keychain")
            }
        } catch {
            NSLog("fuckipv6: restoring the node identity failed: \(error.localizedDescription)")
        }

        let rootListing = (try? FileManager.default.contentsOfDirectory(atPath: storageRoot.path))?
            .sorted().joined(separator: ",") ?? "unreadable"
        Diagnostics.record("storage root files: \(rootListing)")
        let networkDirectory = storageRoot.appendingPathComponent(configuration.networkName, isDirectory: true)
        let networkListing = (try? FileManager.default.contentsOfDirectory(atPath: networkDirectory.path))?
            .sorted().joined(separator: ",") ?? "missing"
        Diagnostics.record("network dir files: \(networkListing)")
        // The network record lives in the configuration generation store, so a
        // missing <root>/<network>/config.toml says nothing about registration.
        // Creating is therefore attempted unconditionally and the duplicate
        // rejection is treated as success: the node then starts from the record.
        publish(.starting, message: "creating network configuration")
        do {
            try Fv6Node.prepareNetwork(
                storageRoot: storageRoot,
                networkName: configuration.networkName,
                coordinatorURL: configuration.coordinatorURL,
                makeDefault: true
            )
        } catch let failure as Fv6Failure where failure.message.contains("已存在")
            || failure.message.contains("already exists") {
            Diagnostics.record("network already registered; continuing")
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

    /// What a teardown actually achieved. A tunnel is only reported as cleanly
    /// stopped when every field below is true.
    struct TeardownReport {
        var pumpDrained = true
        var nodeStopped = true
        var endpointFreed = true
        var settingsCleared = true
        var pendingUsers = 0

        var isClean: Bool {
            pumpDrained && nodeStopped && endpointFreed && settingsCleared
        }

        var summary: String {
            var parts: [String] = []
            if !pumpDrained { parts.append("packet pump did not drain") }
            if !nodeStopped { parts.append("node stop failed") }
            if !endpointFreed { parts.append("endpoint release deferred to \(pendingUsers) in-flight call(s)") }
            if !settingsCleared { parts.append("network settings not cleared") }
            return parts.isEmpty ? "clean" : parts.joined(separator: "; ")
        }
    }

    /// Stops a pump and reports whether both directions drained. Repeatable: a
    /// second call keeps waiting for threads the first call could not join.
    @discardableResult
    private func stopPump(_ pump: PacketPump?, timeout: TimeInterval, reason: String) -> Bool {
        guard let pump else { return true }
        if pump.stop(timeout: timeout) {
            return true
        }
        Diagnostics.record("packet pump still running after \(timeout)s (\(reason))")
        return false
    }

    /// Releases an endpoint handle at most once. When a pump thread is still
    /// inside the FFI call the handle guard defers the free to that thread's
    /// exit, so a timed-out drain can never leave a dangling pointer.
    private func releaseEndpoint(_ endpoint: PacketEndpoint?, reason: String) -> (freed: Bool, users: Int) {
        guard let endpoint else { return (true, 0) }
        switch endpoint.release() {
        case .freed:
            return (true, 0)
        case let .deferred(users):
            Diagnostics.record("endpoint release deferred to \(users) in-flight call(s) (\(reason))")
            return (false, users)
        case .alreadyClosed:
            return (true, 0)
        }
    }

    /// Retries handles parked by an earlier timeout and merges the outcome into
    /// the report of that teardown.
    private func retryPendingCleanup(_ report: TeardownReport, timeout: TimeInterval) -> TeardownReport {
        var merged = report
        state.lock()
        let pump = self.pendingPump
        let endpoint = self.pendingEndpoint
        state.unlock()
        guard pump != nil || endpoint != nil else { return merged }

        let drained = stopPump(pump, timeout: timeout, reason: "retry")
        merged.pumpDrained = report.pumpDrained || drained
        if drained {
            let release = releaseEndpoint(endpoint, reason: "retry")
            merged.endpointFreed = merged.endpointFreed || release.freed
            merged.pendingUsers = release.users
            state.lock()
            self.pendingPump = nil
            self.pendingEndpoint = nil
            state.unlock()
        }
        return merged
    }

    /// Stops every resource in dependency order: pump, node, endpoint, settings,
    /// then the plaintext identity. Safe to call repeatedly and from any thread.
    /// Endpoint release always runs through the handle guard, so a thread that
    /// outlived the drain keeps the memory alive until it returns.
    @discardableResult
    private func teardownNode() -> TeardownReport {
        Diagnostics.record("teardownNode")
        state.lock()
        let node = self.node
        let endpoint = self.endpoint
        let pump = self.pump
        let settings = self.appliedSettings
        self.node = nil
        self.endpoint = nil
        self.pump = nil
        self.appliedSettings = nil
        // A callback still in flight now belongs to a dead session and must not
        // install an endpoint after the node is gone.
        session &+= 1
        state.unlock()

        var report = TeardownReport()
        report.pumpDrained = stopPump(pump, timeout: 5, reason: "teardown")

        if let node {
            do {
                try node.stop()
            } catch {
                report.nodeStopped = false
                NSLog("fuckipv6: node stop failed: \(error.localizedDescription)")
            }
            node.destroy()
        }

        if !report.pumpDrained {
            // The node is down, so a receive blocked on it has no reason to
            // stay: give the thread one bounded retry before parking it.
            report.pumpDrained = stopPump(pump, timeout: 2, reason: "teardown retry")
        }
        let release = releaseEndpoint(endpoint, reason: "teardown")
        report.endpointFreed = release.freed
        report.pendingUsers = release.users

        if !report.pumpDrained {
            state.lock()
            self.pendingPump = pump
            if !report.endpointFreed {
                self.pendingEndpoint = endpoint
            }
            state.unlock()
        }

        if settings != nil {
            report.settingsCleared = clearNetworkSettings()
        }
        captureIdentity()
        if !report.isClean {
            Diagnostics.record("teardown pending: \(report.summary)")
        }
        return report
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

    /// Clears the system network settings and reports whether that succeeded.
    @discardableResult
    private func clearNetworkSettings() -> Bool {
        let result = ApplyResult()
        setTunnelNetworkSettings(nil) { error in
            result.complete(error)
        }
        if let error = result.wait(seconds: 10) {
            NSLog("fuckipv6: clearing network settings failed: \(error.localizedDescription)")
            return false
        }
        return true
    }

    // MARK: - PacketTunnelHost

    func openNetwork(settingsJSON: Data, endpoint pointer: OpaquePointer) -> Int32 {
        do {
            let settings = try JSONDecoder().decode(TunnelSettings.self, from: settingsJSON)
            Diagnostics.record("openNetwork: network=\(settings.network) mtu=\(settings.mtu) transport=\(settings.transport) routes=\(settings.routes.count)")
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
                Diagnostics.record("applying tunnel settings failed: \(error.localizedDescription)")
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
                // stale, so drop it instead of leaking the handle. The guard
                // defers the free if anything already entered it.
                _ = releaseEndpoint(endpoint, reason: "stale open")
                return Fv6Status.state.rawValue
            }

            let drained = stopPump(previousPump, timeout: 5, reason: "device replacement")
            let released = releaseEndpoint(previous, reason: "device replacement")
            if !drained || !released.freed {
                // Keep ownership of anything that is still unwinding so a later
                // teardown can retry the drain.
                state.lock()
                if !drained {
                    self.pendingPump = previousPump
                }
                if !released.freed {
                    self.pendingEndpoint = previous
                }
                state.unlock()
            }
            pump.start()
            Diagnostics.record("packet pump started")

            NSLog("fuckipv6: tunnel configured network=\(settings.network) mtu=\(settings.mtu) transport=\(settings.transport)")
            return Fv6Status.ok.rawValue
        } catch let failure as Fv6Failure {
            return failure.code
        } catch {
            return Fv6Status.invalidArgument.rawValue
        }
    }

    func closeNetwork() -> Int32 {
        Diagnostics.record("closeNetwork")
        state.lock()
        let endpoint = self.endpoint
        let pump = self.pump
        let hadSettings = self.appliedSettings != nil
        self.endpoint = nil
        self.pump = nil
        self.appliedSettings = nil
        // Invalidate an openNetwork that is still applying its settings: its
        // endpoint belongs to a tunnel this close already tore down.
        session &+= 1
        state.unlock()

        let drained = stopPump(pump, timeout: 5, reason: "closeNetwork")
        let released = releaseEndpoint(endpoint, reason: "closeNetwork")
        if !drained || !released.freed {
            state.lock()
            if !drained {
                self.pendingPump = pump
            }
            if !released.freed {
                self.pendingEndpoint = endpoint
            }
            state.unlock()
        }
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
        Diagnostics.record("tunnel phase -> \(phase.rawValue) \(message ?? "")")
        do {
            try status.save()
        } catch {
            NSLog("fuckipv6: publishing status failed: \(error.localizedDescription)")
        }
    }
}
