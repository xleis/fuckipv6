import Foundation
import NetworkExtension

/// Swift mirror of enum Fv6Status in crates/ios-bridge/include/fuckipv6_ios.h.
enum Fv6Status: Int32, Sendable {
    case ok = 0
    case invalidArgument = 1
    case invalidHandle = 2
    case state = 3
    case io = 4
    case timeout = 5
    case closed = 6
    case bufferTooSmall = 7
    case queueFull = 8
    case invalidPacket = 9
    case panic = 10
    case busy = 11
    case nodeProblem = 12

    var isTransient: Bool {
        self == .queueFull || self == .busy || self == .timeout
    }
}

/// Lifecycle phase mirrored from enum Fv6Phase.
enum NodePhase: Int32, Sendable {
    case created = 0
    case starting = 1
    case running = 2
    case stopping = 3
    case stopped = 4
    case failed = 5
}

struct Fv6Failure: LocalizedError, Sendable {
    let code: Int32
    let message: String

    init(code: Int32, message: String) {
        self.code = code
        self.message = message
    }

    init(status: Fv6Status, message: String) {
        self.init(code: status.rawValue, message: message)
    }

    var status: Fv6Status? { Fv6Status(rawValue: code) }

    var errorDescription: String? {
        let name = status.map { "\($0)" } ?? "unknown"
        return "fv6 \(code) (\(name)): \(message)"
    }
}

/// Host callbacks the Rust data plane invokes. Implemented by the packet tunnel
/// provider, which owns the NEPacketTunnelFlow and the system network settings.
protocol PacketTunnelHost: AnyObject {
    /// Applies the tunnel configuration and takes ownership of the endpoint
    /// handle when it returns .ok.
    func openNetwork(settingsJSON: Data, endpoint: OpaquePointer) -> Int32
    /// Tears the session down. Must be idempotent.
    func closeNetwork() -> Int32
}

private func fv6OpenNetworkTrampoline(
    context: UnsafeMutableRawPointer?,
    settingsJSON: UnsafePointer<UInt8>?,
    settingsLength: Int,
    endpoint: OpaquePointer?
) -> Int32 {
    guard let context, let settingsJSON, let endpoint, settingsLength > 0 else {
        return Fv6Status.invalidArgument.rawValue
    }
    let object = Unmanaged<AnyObject>.fromOpaque(context).takeUnretainedValue()
    guard let host = object as? PacketTunnelHost else {
        return Fv6Status.invalidArgument.rawValue
    }
    return host.openNetwork(settingsJSON: Data(bytes: settingsJSON, count: settingsLength), endpoint: endpoint)
}

private func fv6CloseNetworkTrampoline(context: UnsafeMutableRawPointer?) -> Int32 {
    guard let context else { return Fv6Status.invalidArgument.rawValue }
    let object = Unmanaged<AnyObject>.fromOpaque(context).takeUnretainedValue()
    guard let host = object as? PacketTunnelHost else {
        return Fv6Status.invalidArgument.rawValue
    }
    return host.closeNetwork()
}

/// Owns one Fv6Runtime handle. Every method blocks until the Rust side answers;
/// callers must stay off the main thread. The handle is safe to share between
/// threads: the bridge guards its own state with atomics and mutexes.
final class Fv6Node: @unchecked Sendable {
    struct Configuration: Sendable {
        var storageRoot: URL
        var dnsServers: [String]
        var workerThreads: Int

        init(storageRoot: URL, dnsServers: [String] = [], workerThreads: Int = 2) {
            self.storageRoot = storageRoot
            self.dnsServers = dnsServers
            self.workerThreads = workerThreads
        }

        func json() throws -> String {
            let payload: [String: Any] = [
                "storage_root": storageRoot.path,
                "dns_servers": dnsServers,
                "worker_threads": workerThreads,
            ]
            let data = try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])
            guard let text = String(data: data, encoding: .utf8) else {
                throw Fv6Failure(status: .invalidArgument, message: "configuration is not valid UTF-8")
            }
            return text
        }
    }

    private let pointer: OpaquePointer
    /// Defers the runtime free until every in-flight FFI call has returned.
    private let guardrail: HandleGuard
    private let host: any PacketTunnelHost

    init(configuration: Configuration, host: any PacketTunnelHost) throws {
        self.host = host
        let configJSON = try configuration.json()
        var callbacks = Fv6Callbacks()
        callbacks.open_network = fv6OpenNetworkTrampoline
        callbacks.close_network = fv6CloseNetworkTrampoline
        let context = Unmanaged.passUnretained(host as AnyObject).toOpaque()

        var runtime: OpaquePointer?
        var errorBuffer = [UInt8](repeating: 0, count: 4096)
        var errorLength = 0
        let status = errorBuffer.withUnsafeMutableBufferPointer { buffer -> Int32 in
            fv6_runtime_create(
                configJSON,
                callbacks,
                context,
                &runtime,
                buffer.baseAddress,
                buffer.count,
                &errorLength
            )
        }
        guard status == Fv6Status.ok.rawValue, let runtime else {
            throw Fv6Failure(code: status, message: Self.decodeError(errorBuffer, length: errorLength))
        }
        pointer = runtime
        guardrail = HandleGuard { fv6_runtime_destroy(runtime) }
    }

    deinit {
        destroy()
    }

    /// Creates the network configuration and node identity when they are missing.
    /// Reuses the desktop ConfigManager through the bridge; never generates a
    /// second identity format.
    static func prepareNetwork(storageRoot: URL, networkName: String, coordinatorURL: String, makeDefault: Bool) throws {
        var errorBuffer = [UInt8](repeating: 0, count: 4096)
        var errorLength = 0
        let status = errorBuffer.withUnsafeMutableBufferPointer { buffer -> Int32 in
            fv6_prepare_network(
                storageRoot.path,
                networkName,
                coordinatorURL,
                makeDefault ? 1 : 0,
                buffer.baseAddress,
                buffer.count,
                &errorLength
            )
        }
        guard status == Fv6Status.ok.rawValue else {
            throw Fv6Failure(code: status, message: Self.decodeError(errorBuffer, length: errorLength))
        }
    }

    func start(timeout: TimeInterval = 30) throws {
        try withHandle { runtime in
            let status = fv6_runtime_start(runtime, Self.milliseconds(timeout))
            guard status == Fv6Status.ok.rawValue else {
                throw Fv6Failure(code: status, message: lastErrorStatus())
            }
        }
    }

    func stop(timeout: TimeInterval = 15) throws {
        try withHandle { runtime in
            let status = fv6_runtime_stop(runtime, Self.milliseconds(timeout))
            guard status == Fv6Status.ok.rawValue else {
                throw Fv6Failure(code: status, message: lastErrorStatus())
            }
        }
    }

    var phase: NodePhase? {
        guard guardrail.acquire() else { return nil }
        defer { guardrail.release() }
        return NodePhase(rawValue: fv6_runtime_phase(pointer))
    }

    func snapshotJSON(timeout: TimeInterval = 10) throws -> Data {
        try withHandle { runtime in
            try Self.readPayload(errorMessage: { self.lastErrorStatus() }) { buffer, length, outLength in
                fv6_runtime_snapshot(runtime, buffer, length, outLength, Self.milliseconds(timeout))
            }
        }
    }

    func execute(requestJSON: Data, timeout: TimeInterval = 15) throws -> Data {
        guard let request = String(data: requestJSON, encoding: .utf8) else {
            throw Fv6Failure(status: .invalidArgument, message: "request is not valid UTF-8")
        }
        return try withHandle { runtime in
            try Self.readPayload(errorMessage: { self.lastErrorStatus() }) { buffer, length, outLength in
                fv6_runtime_execute(runtime, request, buffer, length, outLength, Self.milliseconds(timeout))
            }
        }
    }

    func networkPathChanged(timeout: TimeInterval = 10) throws {
        try withHandle { runtime in
            let status = fv6_runtime_network_path_changed(runtime, Self.milliseconds(timeout))
            guard status == Fv6Status.ok.rawValue else {
                throw Fv6Failure(code: status, message: lastErrorStatus())
            }
        }
    }

    func lastError() -> String {
        guard guardrail.acquire() else { return "runtime released" }
        defer { guardrail.release() }
        let runtime = pointer
        // Two-pass read: Rust diagnostics list every valid enum variant and
        // routinely exceed a fixed buffer.
        var required = 0
        var probe = [UInt8](repeating: 0, count: 1)
        let first = probe.withUnsafeMutableBufferPointer { pointer in
            fv6_runtime_last_error(runtime, pointer.baseAddress, pointer.count, &required)
        }
        if first == Fv6Status.ok.rawValue {
            return String(decoding: probe[0 ..< min(required, probe.count)], as: UTF8.self)
        }
        guard first == Fv6Status.bufferTooSmall.rawValue, required > 0 else { return "" }
        var buffer = [UInt8](repeating: 0, count: required)
        var written = 0
        let second = buffer.withUnsafeMutableBufferPointer { pointer in
            fv6_runtime_last_error(runtime, pointer.baseAddress, pointer.count, &written)
        }
        guard second == Fv6Status.ok.rawValue else { return "" }
        return String(decoding: buffer[0 ..< written], as: UTF8.self)
    }

    /// Releases the runtime once the last in-flight call has returned. Safe to
    /// call repeatedly and from any thread.
    func destroy() {
        _ = guardrail.close()
    }

    private func withHandle<T>(_ body: (OpaquePointer) throws -> T) throws -> T {
        guard guardrail.acquire() else {
            throw Fv6Failure(status: .invalidHandle, message: "runtime released")
        }
        defer { guardrail.release() }
        return try body(pointer)
    }

    private func lastErrorStatus() -> String {
        lastError()
    }

    fileprivate static func milliseconds(_ seconds: TimeInterval) -> UInt64 {
        seconds <= 0 ? 0 : UInt64((seconds * 1000).rounded())
    }

    private static func decodeError(_ buffer: [UInt8], length: Int) -> String {
        guard length > 0 else { return "" }
        // Truncate rather than drop the diagnostic when the caller buffer is short.
        let end = min(length, buffer.count)
        return String(decoding: buffer[0 ..< end], as: UTF8.self)
    }

    /// Two-pass payload read: ask for the size, then retry with a large enough
    /// buffer. CBuffer is not exposed to callers.
    private static func readPayload(
        errorMessage: () -> String,
        _ call: (UnsafeMutablePointer<UInt8>?, Int, UnsafeMutablePointer<Int>) -> Int32
    ) throws -> Data {
        var required = 0
        var probe = [UInt8](repeating: 0, count: 1)
        let first = probe.withUnsafeMutableBufferPointer { buffer in
            call(buffer.baseAddress, buffer.count, &required)
        }
        if first == Fv6Status.ok.rawValue {
            return Data(probe.prefix(required))
        }
        guard first == Fv6Status.bufferTooSmall.rawValue, required > 0 else {
            throw Fv6Failure(code: first, message: errorMessage())
        }
        var payload = [UInt8](repeating: 0, count: required)
        var written = 0
        let second = payload.withUnsafeMutableBufferPointer { buffer in
            call(buffer.baseAddress, buffer.count, &written)
        }
        guard second == Fv6Status.ok.rawValue else {
            throw Fv6Failure(code: second, message: errorMessage())
        }
        return Data(payload.prefix(written))
    }
}

/// Owns one Fv6Endpoint handle. The endpoint is handed to the host through
/// Fv6OpenNetworkCallback; the host must release it exactly once.
///
/// Every FFI call is bracketed by HandleGuard, so a release that races with an
/// in-flight send or receive defers the actual free instead of pulling the
/// handle out from under the worker thread.
final class PacketEndpoint: @unchecked Sendable {
    private let pointer: OpaquePointer
    private let guardrail: HandleGuard
    let mtu: Int

    init(pointer: OpaquePointer) throws {
        var mtu: UInt16 = 0
        let status = fv6_endpoint_mtu(pointer, &mtu)
        guard status == Fv6Status.ok.rawValue, mtu >= 1280 else {
            fv6_endpoint_release(pointer)
            throw Fv6Failure(code: status, message: "endpoint reported an invalid MTU")
        }
        self.pointer = pointer
        self.mtu = Int(mtu)
        self.guardrail = HandleGuard { fv6_endpoint_release(pointer) }
    }

    deinit {
        release()
    }

    /// Queues one complete IP packet. Non-blocking; .queueFull means back off.
    func send(_ packet: Data) -> Int32 {
        guard guardrail.acquire() else { return Fv6Status.closed.rawValue }
        defer { guardrail.release() }
        return packet.withUnsafeBytes { raw in
            fv6_endpoint_send(pointer, raw.bindMemory(to: UInt8.self).baseAddress, packet.count)
        }
    }

    /// Dequeues one complete IP packet into a caller buffer of at least mtu
    /// bytes. Returns the number of bytes written, or throws on timeout.
    func receive(into buffer: inout [UInt8], timeout: TimeInterval) throws -> Int {
        guard guardrail.acquire() else {
            throw Fv6Failure(status: .closed, message: "endpoint released")
        }
        defer { guardrail.release() }
        guard buffer.count >= mtu else {
            throw Fv6Failure(status: .bufferTooSmall, message: "buffer holds \(buffer.count) bytes, MTU is \(mtu)")
        }
        var length = 0
        let status = buffer.withUnsafeMutableBufferPointer { pointer in
            fv6_endpoint_recv(self.pointer, pointer.baseAddress, pointer.count, &length, Fv6Node.milliseconds(timeout))
        }
        guard status == Fv6Status.ok.rawValue else {
            throw Fv6Failure(code: status, message: "packet receive failed")
        }
        return length
    }

    /// Releases the handle. Safe to call from any thread and any number of
    /// times; a close that races with an in-flight call is deferred to the
    /// last user instead of freeing memory still in use.
    @discardableResult
    func release() -> HandleGuard.CloseOutcome {
        guardrail.close()
    }

    var isReleased: Bool {
        guardrail.isClosed
    }

    var inFlightUsers: Int {
        guardrail.userCount
    }
}

