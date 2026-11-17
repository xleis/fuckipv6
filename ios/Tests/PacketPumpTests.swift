import XCTest

/// Fake packet flow: the test pushes batches exactly like the system would and
/// records everything the pump writes back.
final class FakePacketFlow: PacketFlow, @unchecked Sendable {
    private let lock = NSLock()
    private var handler: (@Sendable ([PacketPumpPacket]) -> Void)?
    private var written: [PacketPumpPacket] = []
    private(set) var readRequests = 0

    func startReading(_ handler: @escaping @Sendable ([PacketPumpPacket]) -> Void) {
        lock.lock()
        self.handler = handler
        readRequests += 1
        lock.unlock()
    }

    func writePackets(_ packets: [PacketPumpPacket]) {
        lock.lock()
        written.append(contentsOf: packets)
        lock.unlock()
    }

    var writtenPackets: [PacketPumpPacket] {
        lock.lock()
        defer { lock.unlock() }
        return written
    }

    /// Delivers one batch to the currently armed read.
    func push(_ packets: [PacketPumpPacket]) {
        lock.lock()
        let handler = self.handler
        lock.unlock()
        handler?(packets)
    }
}

final class FakePacketChannel: PacketChannel, @unchecked Sendable {
    let mtu: Int
    private let lock = NSLock()
    private var sentPackets: [Data] = []
    private var queue: [Data] = []
    var sendStatus: Int32 = Fv6Status.ok.rawValue
    var receiveError: Fv6Failure?

    init(mtu: Int = 1280) {
        self.mtu = mtu
    }

    var sent: [Data] {
        lock.lock()
        defer { lock.unlock() }
        return sentPackets
    }

    func send(_ packet: Data) -> Int32 {
        lock.lock()
        sentPackets.append(packet)
        let status = sendStatus
        lock.unlock()
        return status
    }

    func enqueue(_ packet: Data) {
        lock.lock()
        queue.append(packet)
        lock.unlock()
    }

    func receive(into buffer: inout [UInt8], timeout: TimeInterval) throws -> Int {
        if let error = receiveError {
            throw error
        }
        lock.lock()
        let next = queue.isEmpty ? nil : queue.removeFirst()
        lock.unlock()
        guard let next else {
            // Mimic the endpoint blocking for its timeout instead of spinning.
            Thread.sleep(forTimeInterval: 0.002)
            throw Fv6Failure(status: .timeout, message: "queue is empty")
        }
        let count = min(next.count, buffer.count)
        buffer.withUnsafeMutableBytes { raw in
            guard let base = raw.baseAddress else { return }
            next.copyBytes(to: base.assumingMemoryBound(to: UInt8.self), count: count)
        }
        return count
    }
}

func ipv6Packet(payloadSize: Int = 16) -> Data {
    var bytes = [UInt8](repeating: 0, count: 40 + payloadSize)
    bytes[0] = 0x60
    bytes[5] = UInt8(payloadSize)
    bytes[6] = 59
    bytes[7] = 64
    for index in 0..<16 {
        bytes[8 + index] = 0xfd
        bytes[24 + index] = 0xfd
    }
    return Data(bytes)
}

func ipv4Packet(payloadSize: Int = 16) -> Data {
    var bytes = [UInt8](repeating: 0, count: 20 + payloadSize)
    bytes[0] = 0x45
    let total = UInt16(bytes.count)
    bytes[2] = UInt8(total >> 8)
    bytes[3] = UInt8(total & 0xff)
    bytes[8] = 64
    bytes[9] = 17
    bytes[12] = 10
    bytes[15] = 1
    bytes[16] = 10
    bytes[19] = 2
    return Data(bytes)
}

func waitUntil(_ timeout: TimeInterval = 3, _ condition: () -> Bool) -> Bool {
    let deadline = Date().addingTimeInterval(timeout)
    while Date() < deadline {
        if condition() {
            return true
        }
        Thread.sleep(forTimeInterval: 0.005)
    }
    return condition()
}

final class PacketPumpTests: XCTestCase {
    private var flow: FakePacketFlow!
    private var channel: FakePacketChannel!
    private var pump: PacketPump!

    override func setUp() {
        super.setUp()
        flow = FakePacketFlow()
        channel = FakePacketChannel()
        pump = PacketPump(flow: flow, channel: channel)
    }

    override func tearDown() {
        pump?.stop()
        super.tearDown()
    }

    private func packet(_ bytes: Data) -> PacketPumpPacket {
        guard let packet = PacketPumpPacket(data: bytes) else {
            fatalError("test packet must be a valid IP datagram")
        }
        return packet
    }

    func testForwardsHostPacketsToTheChannelInOrder() {
        pump.start()
        XCTAssertTrue(pump.isRunning)

        let first = ipv6Packet()
        let second = ipv4Packet()
        flow.push([packet(first), packet(second)])

        XCTAssertTrue(waitUntil { self.channel.sent.count == 2 })
        XCTAssertEqual(channel.sent[0], first)
        XCTAssertEqual(channel.sent[1], second)
        XCTAssertEqual(pump.statistics.hostToNode, 2)
        XCTAssertEqual(pump.statistics.dropped, 0)
    }

    func testDropsPacketWhenTheQueueStaysFull() {
        pump.start()
        channel.sendStatus = Fv6Status.queueFull.rawValue
        flow.push([packet(ipv6Packet())])

        XCTAssertTrue(waitUntil(5) { self.pump.statistics.dropped == 1 })
        XCTAssertEqual(pump.statistics.hostToNode, 0)
        XCTAssertEqual(channel.sent.count, PacketPump.backoffAttempts)
    }

    func testDropsInvalidPacketsWithoutRetrying() {
        pump.start()
        channel.sendStatus = Fv6Status.invalidPacket.rawValue
        flow.push([packet(ipv6Packet())])

        XCTAssertTrue(waitUntil { self.pump.statistics.dropped == 1 })
        XCTAssertEqual(channel.sent.count, 1)
        XCTAssertEqual(pump.statistics.hostToNode, 0)
    }

    func testClosedChannelStopsThePump() {
        pump.start()
        channel.sendStatus = Fv6Status.closed.rawValue
        flow.push([packet(ipv6Packet())])

        XCTAssertTrue(waitUntil { !self.pump.isRunning })
        XCTAssertEqual(pump.statistics.failures, 1)
    }

    func testEmptyBatchMeansTheSystemFlowClosed() {
        pump.start()
        flow.push([])

        XCTAssertTrue(waitUntil { !self.pump.isRunning })
        XCTAssertTrue(pump.stop())
    }

    func testForwardsNodePacketsToTheFlow() {
        let outgoing = ipv6Packet(payloadSize: 8)
        channel.enqueue(outgoing)
        pump.start()

        XCTAssertTrue(waitUntil { self.flow.writtenPackets.count == 1 })
        XCTAssertEqual(flow.writtenPackets[0].data, outgoing)
        XCTAssertEqual(flow.writtenPackets[0].protocolFamily, AF_INET6)
        XCTAssertEqual(pump.statistics.nodeToHost, 1)
    }

    func testClosedChannelEndsTheNodeToHostLoop() {
        pump.start()
        channel.receiveError = Fv6Failure(status: .closed, message: "endpoint closed")

        XCTAssertTrue(waitUntil { !self.pump.isRunning })
        XCTAssertEqual(pump.statistics.failures, 1)
    }

    func testStopIsRepeatableAndBounded() {
        pump.start()
        XCTAssertTrue(pump.stop(timeout: 1))
        XCTAssertFalse(pump.isRunning)
        XCTAssertTrue(pump.stop(timeout: 1))
    }

    func testRestartStartsAFreshSession() {
        pump.start()
        XCTAssertTrue(pump.stop(timeout: 1))
        XCTAssertTrue(pump.stop(timeout: 1))
        pump.start()
        XCTAssertTrue(pump.isRunning)

        flow.push([packet(ipv6Packet())])
        XCTAssertTrue(waitUntil { self.channel.sent.count == 1 })
    }

    func testStaleCallbackAfterStopIsNotForwarded() {
        pump.start()
        XCTAssertTrue(pump.stop(timeout: 1))

        flow.push([packet(ipv6Packet())])
        Thread.sleep(forTimeInterval: 0.05)
        XCTAssertEqual(channel.sent.count, 0)
        XCTAssertEqual(pump.statistics.hostToNode, 0)
    }

    func testPacketFamilyFollowsTheVersionNibble() {
        XCTAssertEqual(PacketPumpPacket(data: ipv4Packet())?.protocolFamily, AF_INET)
        XCTAssertEqual(PacketPumpPacket(data: ipv6Packet())?.protocolFamily, AF_INET6)
        XCTAssertNil(PacketPumpPacket(data: Data([0x00, 0x01])))
        XCTAssertNil(PacketPumpPacket(data: Data()))
    }
}
