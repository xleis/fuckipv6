import XCTest

/// Records the callbacks the Rust platform layer makes. The iOS host only
/// reaches openNetwork once a signaling session is up, so lifecycle tests assert
/// on ordering and on the runtime's own phase instead of on traffic.
final class RecordingHost: PacketTunnelHost, @unchecked Sendable {
    private let lock = NSLock()
    private var openCalls = 0
    private var closeCalls = 0
    private var settings: [Data] = []
    var openResult: Int32 = Fv6Status.io.rawValue

    func openNetwork(settingsJSON: Data, endpoint: OpaquePointer) -> Int32 {
        lock.lock()
        openCalls += 1
        settings.append(settingsJSON)
        let result = openResult
        lock.unlock()
        return result
    }

    func closeNetwork() -> Int32 {
        lock.lock()
        closeCalls += 1
        lock.unlock()
        return Fv6Status.ok.rawValue
    }

    var opens: Int {
        lock.lock()
        defer { lock.unlock() }
        return openCalls
    }

    var closes: Int {
        lock.lock()
        defer { lock.unlock() }
        return closeCalls
    }
}

/// Exercises the real Rust runtime through the C ABI the extension uses.
///
/// The configuration points at ws://127.0.0.1:9, so every start reports the
/// node's own initialization failure instead of reaching running. That is the
/// behaviour under test: the bridge must surface the real NodeProblem, keep the
/// runtime reusable, and stop cleanly. The successful start path needs a
/// reachable coordinator and is covered by the real-device run.
final class RustRuntimeTests: XCTestCase {
    private var root: URL!

    override func setUpWithError() throws {
        try super.setUpWithError()
        root = FileManager.default.temporaryDirectory
            .appendingPathComponent("fv6-runtime-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        if let root, FileManager.default.fileExists(atPath: root.path) {
            try FileManager.default.removeItem(at: root)
        }
        try super.tearDownWithError()
    }

    private func makeNode(
        networkName: String = "p3test",
        coordinatorURL: String = "ws://127.0.0.1:9",
        host: RecordingHost
    ) throws -> Fv6Node {
        try Fv6Node.prepareNetwork(
            storageRoot: root,
            networkName: networkName,
            coordinatorURL: coordinatorURL,
            makeDefault: true
        )
        return try Fv6Node(
            configuration: Fv6Node.Configuration(storageRoot: root, dnsServers: [], workerThreads: 2),
            host: host
        )
    }

    /// start() must fail with the node's own problem, mark the runtime failed,
    /// expose the diagnostic, and stay restartable afterwards.
    func testStartReportsTheNodeProblemAndStaysRestartable() throws {
        let host = RecordingHost()
        let node = try makeNode(host: host)
        defer { node.destroy() }
        XCTAssertEqual(node.phase, .created)

        XCTAssertThrowsError(try node.start(timeout: 30)) { error in
            guard let failure = error as? Fv6Failure else {
                return XCTFail("unexpected error type: \(error)")
            }
            XCTAssertEqual(failure.code, Fv6Status.nodeProblem.rawValue)
            XCTAssertTrue(
                failure.message.contains("协调服务器") || failure.message.contains("coordinator"),
                "unexpected message: \(failure.message)"
            )
        }
        XCTAssertEqual(node.phase, .failed)
        XCTAssertFalse(node.lastError().isEmpty)
        // The network lease is only requested once signaling is up, so a failed
        // start must never touch the tunnel settings.
        XCTAssertEqual(host.opens, 0)

        // Commands are refused while the runtime is not running.
        XCTAssertThrowsError(try node.snapshotJSON()) { error in
            XCTAssertEqual((error as? Fv6Failure)?.code, Fv6Status.state.rawValue)
        }

        try node.stop()
        XCTAssertEqual(node.phase, .stopped)

        // A failed start leaves no residue: the node can try again.
        XCTAssertThrowsError(try node.start(timeout: 30)) { error in
            XCTAssertEqual((error as? Fv6Failure)?.code, Fv6Status.nodeProblem.rawValue)
        }
        XCTAssertEqual(node.phase, .failed)

        // Repeated stop is accepted and leaves the runtime stopped.
        try node.stop()
        try node.stop()
        XCTAssertEqual(node.phase, .stopped)
    }

    /// Stopping during start must end in a stopped runtime with the start call
    /// reporting a terminal status. The Rust start path cannot be interrupted
    /// mid-initialization, so stop joins the in-flight start (bounded by its
    /// timeout) and then tears the runtime down.
    func testStopDuringStartEndsInAStoppedRuntime() throws {
        let host = RecordingHost()
        let node = try makeNode(networkName: "p3cancel", host: host)
        defer { node.destroy() }

        let started = DispatchSemaphore(value: 0)
        let lock = NSLock()
        var startStatus: Int32?
        DispatchQueue.global().async {
            do {
                try node.start(timeout: 60)
            } catch let failure as Fv6Failure {
                lock.lock()
                startStatus = failure.code
                lock.unlock()
            } catch {
                XCTFail("unexpected error: \(error)")
            }
            started.signal()
        }

        Thread.sleep(forTimeInterval: 0.5)
        try node.stop(timeout: 30)
        XCTAssertEqual(node.phase, .stopped)

        XCTAssertEqual(started.wait(timeout: .now() + 10), .success)
        lock.lock()
        let status = startStatus
        lock.unlock()
        XCTAssertTrue(
            status == Fv6Status.closed.rawValue || status == Fv6Status.nodeProblem.rawValue,
            "unexpected start status: \(String(describing: status))"
        )

        // The start failure left no residue: the runtime is reusable.
        XCTAssertThrowsError(try node.start(timeout: 30)) { error in
            XCTAssertEqual((error as? Fv6Failure)?.code, Fv6Status.nodeProblem.rawValue)
        }
        try node.stop()
        XCTAssertEqual(node.phase, .stopped)
    }

    func testDestroyWithoutStartIsSafeAndRepeatable() throws {
        let host = RecordingHost()
        let node = try Fv6Node(
            configuration: Fv6Node.Configuration(storageRoot: root, dnsServers: [], workerThreads: 1),
            host: host
        )
        node.destroy()
        node.destroy()
        XCTAssertNil(node.phase)
    }

    func testInvalidConfigurationIsRejectedByTheCBridge() {
        let host = RecordingHost()
        XCTAssertThrowsError(
            try Fv6Node(
                configuration: Fv6Node.Configuration(
                    storageRoot: root,
                    dnsServers: ["not-an-address"],
                    workerThreads: 2
                ),
                host: host
            )
        ) { error in
            guard let failure = error as? Fv6Failure else {
                return XCTFail("unexpected error type: \(error)")
            }
            XCTAssertEqual(failure.code, Fv6Status.invalidArgument.rawValue)
            XCTAssertFalse(failure.message.isEmpty)
        }
    }

    /// Malformed command payloads are rejected before the phase check, while
    /// well-formed commands on a runtime that never started return state.
    func testCommandsAreValidatedBeforeThePhaseCheck() throws {
        let host = RecordingHost()
        let node = try Fv6Node(
            configuration: Fv6Node.Configuration(storageRoot: root, dnsServers: [], workerThreads: 1),
            host: host
        )
        defer { node.destroy() }

        XCTAssertThrowsError(try node.execute(requestJSON: Data("not json".utf8))) { error in
            XCTAssertEqual((error as? Fv6Failure)?.code, Fv6Status.invalidArgument.rawValue)
        }
        XCTAssertThrowsError(
            try node.execute(
                requestJSON: Data("{\"request_id\":2,\"command\":{\"command\":\"not-a-command\"}}".utf8)
            )
        ) { error in
            guard let failure = error as? Fv6Failure else {
                return XCTFail("unexpected error type: \(error)")
            }
            XCTAssertEqual(failure.code, Fv6Status.invalidArgument.rawValue)
            // The bridge must forward the Rust diagnostic, not a generic text.
            XCTAssertTrue(
                failure.message.contains("invalid node command request"),
                "unexpected message: \(failure.message)"
            )
        }
        // The canonical request shape is the one NodeCommandRequest serialises to.
        XCTAssertThrowsError(
            try node.execute(
                requestJSON: Data("{\"request_id\":1,\"command\":{\"command\":\"get_snapshot\"}}".utf8)
            )
        ) { error in
            XCTAssertEqual((error as? Fv6Failure)?.code, Fv6Status.state.rawValue)
        }
    }
}