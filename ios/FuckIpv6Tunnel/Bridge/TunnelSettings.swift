import Foundation
import NetworkExtension

/// Mirror of the JSON produced by crates/ios-bridge/src/settings.rs.
struct TunnelSettings: Decodable, Sendable, Equatable {
    struct HostAddress: Decodable, Sendable, Equatable {
        let address: String
        let prefix: Int
    }

    let network: String
    let mtu: Int
    let ipv4: HostAddress?
    let ipv6: HostAddress?
    let routes: [String]
    let dns: [String]
    let transport: String

    /// Real transport endpoint (coordinator-assigned P2P address), never a
    /// virtual mesh address. Used as the tunnel remote address.
    var remoteAddress: String {
        if transport.hasPrefix("[") {
            if let close = transport.firstIndex(of: "]") {
                return String(transport[transport.index(after: transport.startIndex)..<close])
            }
            return transport
        }
        guard let colon = transport.lastIndex(of: ":") else { return transport }
        return String(transport[transport.startIndex..<colon])
    }

    /// Builds the split-tunnel network settings. Only the mesh address space is
    /// captured: default routes are deliberately left out so the QUIC and
    /// signaling transports keep using the physical interface.
    func makeNetworkSettings() throws -> NEPacketTunnelNetworkSettings {
        guard mtu >= 1280 else {
            throw Fv6Failure(code: Fv6Status.invalidArgument.rawValue, message: "MTU \(mtu) is below the IPv6 minimum")
        }
        let settings = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: remoteAddress)
        settings.mtu = NSNumber(value: mtu)

        if let ipv4 {
            let v4 = NEIPv4Settings(
                addresses: [ipv4.address],
                subnetMasks: [Self.ipv4Mask(prefix: ipv4.prefix)]
            )
            v4.includedRoutes = routes.compactMap(Self.ipv4Route(cidr:))
            settings.ipv4Settings = v4
        }
        if let ipv6 {
            let v6 = NEIPv6Settings(addresses: [ipv6.address], networkPrefixLengths: [NSNumber(value: ipv6.prefix)])
            v6.includedRoutes = routes.compactMap(Self.ipv6Route(cidr:))
            settings.ipv6Settings = v6
        }
        if !dns.isEmpty {
            settings.dnsSettings = NEDNSSettings(servers: dns)
        }
        return settings
    }

    private static func ipv4Mask(prefix: Int) -> String {
        let clamped = max(0, min(32, prefix))
        let mask: UInt32 = clamped == 0 ? 0 : ~UInt32(0) << (32 - clamped)
        return "\((mask >> 24) & 0xff).\((mask >> 16) & 0xff).\((mask >> 8) & 0xff).\(mask & 0xff)"
    }

    private static func ipv4Route(cidr: String) -> NEIPv4Route? {
        let parts = cidr.split(separator: "/")
        guard parts.count == 2, let prefix = Int(parts[1]), !parts[0].contains(":") else { return nil }
        return NEIPv4Route(destinationAddress: String(parts[0]), subnetMask: ipv4Mask(prefix: prefix))
    }

    private static func ipv6Route(cidr: String) -> NEIPv6Route? {
        let parts = cidr.split(separator: "/")
        guard parts.count == 2, let prefix = Int(parts[1]), parts[0].contains(":") else { return nil }
        return NEIPv6Route(destinationAddress: String(parts[0]), networkPrefixLength: NSNumber(value: prefix))
    }
}
