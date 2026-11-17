import Darwin
import Foundation
import NetworkExtension

/// One complete IP packet plus the address family the system flow expects.
struct PacketPumpPacket: Sendable {
    let data: Data
    let protocolFamily: Int32

    init(data: Data, protocolFamily: Int32) {
        self.data = data
        self.protocolFamily = protocolFamily
    }

    /// Family derived from the IP version nibble. Packets are whole datagrams,
    /// so the first nibble always belongs to the packet being forwarded.
    init?(data: Data) {
        guard let first = data.first else { return nil }
        switch first >> 4 {
        case 4:
            self.init(data: data, protocolFamily: AF_INET)
        case 6:
            self.init(data: data, protocolFamily: AF_INET6)
        default:
            return nil
        }
    }
}

/// The system packet flow, narrowed to the two calls the pump needs. Tests
/// drive the pump through this protocol instead of a NetworkExtension session.
///
/// The read side is callback based on purpose: NEPacketTunnelFlow has no
/// cancellation, so a blocking read cannot be interrupted by a stop request and
/// would leave a thread inside the system call while the endpoint behind it is
/// already released.
protocol PacketFlow: AnyObject {
    /// Arms exactly one read. The handler receives the arriving packets, or an
    /// empty array once the flow is closed.
    func startReading(_ handler: @escaping @Sendable ([PacketPumpPacket]) -> Void)
    func writePackets(_ packets: [PacketPumpPacket])
}

/// NEPacketTunnelFlow adapter.
final class TunnelPacketFlow: PacketFlow, @unchecked Sendable {
    private let flow: NEPacketTunnelFlow

    init(flow: NEPacketTunnelFlow) {
        self.flow = flow
    }

    func startReading(_ handler: @escaping @Sendable ([PacketPumpPacket]) -> Void) {
        flow.readPackets { packets, _ in
            var batch: [PacketPumpPacket] = []
            batch.reserveCapacity(packets.count)
            for data in packets {
                // Drop anything that is not an IP datagram instead of handing a
                // malformed buffer to the data plane.
                guard let packet = PacketPumpPacket(data: data) else { continue }
                batch.append(packet)
            }
            handler(batch)
        }
    }

    func writePackets(_ packets: [PacketPumpPacket]) {
        guard !packets.isEmpty else { return }
        flow.writePackets(packets.map(\.data), withProtocols: packets.map { NSNumber(value: $0.protocolFamily) })
    }
}
