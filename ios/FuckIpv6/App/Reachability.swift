import Foundation
import Network

/// Fail-fast diagnostics for the coordinator endpoint.
///
/// The probe doubles as the trigger for the iOS Local Network permission
/// prompt: iOS attributes that prompt to the app, and a packet tunnel extension
/// cannot raise it on its own. Running it from the app before the tunnel starts
/// is what makes a LAN coordinator reachable from the extension.
enum Reachability {
    struct Endpoint {
        let host: String
        let port: UInt16

        init?(_ urlString: String) {
            guard let url = URL(string: urlString), let host = url.host, !host.isEmpty else {
                return nil
            }
            self.host = host
            let explicitPort = url.port
            switch url.scheme?.lowercased() {
            case "wss":
                self.port = UInt16(clamping: explicitPort ?? 443)
            default:
                self.port = UInt16(clamping: explicitPort ?? 80)
            }
        }
    }

    /// Returns nil when the endpoint accepts a TCP connection, otherwise a
    /// human-readable diagnostic. Never throws: callers always get an answer.
    static func check(urlString: String, timeout: TimeInterval = 4) async -> String? {
        guard let endpoint = Endpoint(urlString) else {
            return "cannot parse the coordinator URL \(urlString)"
        }
        guard let port = NWEndpoint.Port(rawValue: endpoint.port) else {
            return "invalid port \(endpoint.port)"
        }
        let connection = NWConnection(
            host: NWEndpoint.Host(endpoint.host),
            port: port,
            using: .tcp
        )
        let queue = DispatchQueue(label: "com.lingxifox.fuckipv6.reachability")
        return await withCheckedContinuation { (continuation: CheckedContinuation<String?, Never>) in
            let probe = Probe(connection, continuation: continuation)
            connection.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    probe.finish(nil)
                case .failed(let error):
                    probe.finish("connect failed: \(error.localizedDescription)")
                case .waiting(let error):
                    probe.finish(
                        "connect blocked: \(error.localizedDescription); "
                            + "check the Local Network permission"
                    )
                default:
                    break
                }
            }
            connection.start(queue: queue)
            queue.asyncAfter(deadline: .now() + timeout) {
                probe.finish("connect timed out after \(Int(timeout))s")
            }
        }
    }

    /// Single-resume guard: Network.framework reports several states and the
    /// timeout races them, so the continuation is claimed exactly once.
    private final class Probe: @unchecked Sendable {
        private let lock = NSLock()
        private let connection: NWConnection
        private var continuation: CheckedContinuation<String?, Never>?

        init(_ connection: NWConnection, continuation: CheckedContinuation<String?, Never>) {
            self.connection = connection
            self.continuation = continuation
        }

        func finish(_ diagnostic: String?) {
            lock.lock()
            let pending = continuation
            continuation = nil
            lock.unlock()
            guard let pending else { return }
            connection.cancel()
            pending.resume(returning: diagnostic)
        }
    }
}
