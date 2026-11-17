import Foundation

/// The node side of the packet path. PacketEndpoint is the shipping
/// implementation; tests substitute a fake channel.
protocol PacketChannel: AnyObject {
    var mtu: Int { get }
    func send(_ packet: Data) -> Int32
    func receive(into buffer: inout [UInt8], timeout: TimeInterval) throws -> Int
}

extension PacketEndpoint: PacketChannel {}

/// Bridges the system packet flow and the Rust data plane.
///
/// Host-to-node packets arrive through one armed read at a time, so the pump
/// never spawns a task per packet and the P1 device queue stays the only
/// buffer. Node-to-host packets are drained by a single thread. A full queue
/// becomes a bounded retry and then a dropped packet.
///
/// Lifetime: every send is bracketed by an in-flight count guarded by the same
/// condition that stop() waits on, so stop() returning means no callback can
/// touch the endpoint afterwards.
final class PacketPump: @unchecked Sendable {
    struct Statistics: Sendable, Equatable {
        var hostToNode = 0
        var nodeToHost = 0
        var dropped = 0
        var failures = 0
    }

    /// 250 ms keeps idle wake-ups cheap while still draining bursts promptly.
    static let receiveTimeout: TimeInterval = 0.25
    /// A full queue is transient. 64 ms of retries outlasts a scheduling
    /// hiccup without letting a single packet pin the pump thread.
    static let backoffAttempts = 64
    static let backoffInterval: TimeInterval = 0.001
    /// Back-off after a non-fatal node error so the loop cannot spin.
    static let errorBackoff: TimeInterval = 0.01

    private let flow: any PacketFlow
    private let channel: any PacketChannel
    /// Guards running/generation/inFlight/stats and wakes stop() waiters.
    private let lock = NSCondition()
    private let drained = DispatchGroup()
    private var running = false
    /// Bumped on every start and stop: a stale callback from an older session
    /// must not forward packets or re-arm the flow.
    private var generation: UInt64 = 0
    private var inFlight = 0
    private var stats = Statistics()
    private var buffer: [UInt8]

    init(flow: any PacketFlow, channel: any PacketChannel) {
        self.flow = flow
        self.channel = channel
        self.buffer = [UInt8](repeating: 0, count: max(channel.mtu, 1280))
    }

    var statistics: Statistics {
        lock.lock()
        defer { lock.unlock() }
        return stats
    }

    var isRunning: Bool {
        lock.lock()
        defer { lock.unlock() }
        return running
    }

    /// Starts both directions. Repeated calls are no-ops.
    func start() {
        lock.lock()
        if running {
            lock.unlock()
            return
        }
        running = true
        generation &+= 1
        let session = generation
        lock.unlock()

        drained.enter()
        startThread(named: "fv6.pump.nodeToHost") { [self] in
            defer { drained.leave() }
            nodeToHostLoop(session)
        }
        issueRead(session)
    }

    /// Stops both directions and reports whether everything finished inside the
    /// deadline. The endpoint must not be released while this returns false.
    @discardableResult
    func stop(timeout: TimeInterval = 5) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        lock.lock()
        let wasRunning = running
        running = false
        generation &+= 1
        while inFlight > 0, lock.wait(until: deadline) {}
        let callbacksDrained = inFlight == 0
        lock.unlock()

        guard wasRunning else { return callbacksDrained }
        let threadDrained = drained.wait(timeout: .now() + max(0, deadline.timeIntervalSinceNow)) == .success
        return callbacksDrained && threadDrained
    }

    // MARK: - Host to node

    private func issueRead(_ session: UInt64) {
        guard isCurrent(session) else { return }
        flow.startReading { [weak self] packets in
            self?.handleRead(session, packets)
        }
    }

    private func handleRead(_ session: UInt64, _ packets: [PacketPumpPacket]) {
        guard isCurrent(session) else { return }
        if packets.isEmpty {
            // The system flow reports closure with an empty batch.
            markStopped(session)
            return
        }
        guard admit(session) else { return }
        for packet in packets {
            guard isCurrent(session) else { break }
            deliver(packet, session)
        }
        releaseAdmit()
        issueRead(session)
    }

    private func deliver(_ packet: PacketPumpPacket, _ session: UInt64) {
        var attempt = 0
        while true {
            let status = channel.send(packet.data)
            switch status {
            case Fv6Status.ok.rawValue:
                count(\.hostToNode)
                return
            case Fv6Status.queueFull.rawValue, Fv6Status.busy.rawValue:
                attempt += 1
                if attempt >= Self.backoffAttempts {
                    count(\.dropped)
                    return
                }
                Thread.sleep(forTimeInterval: Self.backoffInterval)
                guard isCurrent(session) else { return }
            case Fv6Status.invalidPacket.rawValue:
                count(\.dropped)
                return
            case Fv6Status.closed.rawValue, Fv6Status.invalidHandle.rawValue:
                count(\.failures)
                markStopped(session)
                return
            default:
                count(\.failures)
                return
            }
        }
    }

    // MARK: - Node to host

    private func nodeToHostLoop(_ session: UInt64) {
        while isCurrent(session) {
            do {
                let length = try channel.receive(into: &buffer, timeout: Self.receiveTimeout)
                guard length > 0, length <= buffer.count else {
                    count(\.dropped)
                    continue
                }
                guard let packet = PacketPumpPacket(data: Data(buffer[0..<length])) else {
                    count(\.dropped)
                    continue
                }
                flow.writePackets([packet])
                count(\.nodeToHost)
            } catch let failure as Fv6Failure {
                switch failure.status {
                case .timeout:
                    continue
                case .closed, .invalidHandle:
                    count(\.failures)
                    markStopped(session)
                    return
                case .bufferTooSmall:
                    buffer = [UInt8](repeating: 0, count: max(channel.mtu, 1280))
                default:
                    count(\.failures)
                    Thread.sleep(forTimeInterval: Self.errorBackoff)
                }
            } catch {
                count(\.failures)
                Thread.sleep(forTimeInterval: Self.errorBackoff)
            }
        }
    }

    // MARK: - Session state

    private func startThread(named name: String, _ body: @escaping @Sendable () -> Void) {
        let thread = Thread(block: body)
        thread.name = name
        thread.start()
    }

    private func isCurrent(_ session: UInt64) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return running && generation == session
    }

    /// Marks the current callback as writing to the endpoint. Returns false when
    /// the session already ended, in which case nothing may be sent.
    private func admit(_ session: UInt64) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard running, generation == session else { return false }
        inFlight += 1
        return true
    }

    private func releaseAdmit() {
        lock.lock()
        inFlight -= 1
        lock.broadcast()
        lock.unlock()
    }

    private func markStopped(_ session: UInt64) {
        lock.lock()
        if generation == session {
            running = false
        }
        lock.broadcast()
        lock.unlock()
    }

    /// Counts only for the live session; a stale loop must not pollute stats.
    private func count(_ keyPath: WritableKeyPath<Statistics, Int>) {
        lock.lock()
        stats[keyPath: keyPath] += 1
        lock.unlock()
    }
}
