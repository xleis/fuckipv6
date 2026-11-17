import Foundation

/// The node operations the tunnel IPC may invoke. Fv6Node implements this in
/// production; tests supply a fake so the contract is checked without a runtime.
protocol NodeControlling: AnyObject {
    var phase: NodePhase? { get }
    func snapshotJSON(timeout: TimeInterval) throws -> Data
    func execute(requestJSON: Data, timeout: TimeInterval) throws -> Data
    func networkPathChanged(timeout: TimeInterval) throws
    func lastError() -> String
}

extension Fv6Node: NodeControlling {}

/// Versioned control channel between the app and the extension.
///
/// Only the containing app can address the provider through
/// NETunnelProviderSession, so the channel is already scoped to this app group.
/// On top of that every message is length-checked, version-checked and limited
/// to a fixed command set; nothing reaches the node unless the node is running.
final class MessageRouter: @unchecked Sendable {
    static let version = 1
    static let maximumMessageBytes = 64 * 1024
    static let snapshotTimeout: TimeInterval = 10
    static let executeTimeout: TimeInterval = 15
    static let pathChangeTimeout: TimeInterval = 10

    enum Command: String, CaseIterable {
        case status
        case snapshot
        case execute
        case pathChanged
    }

    private let node: @Sendable () -> (any NodeControlling)?

    init(node: @escaping @Sendable () -> (any NodeControlling)?) {
        self.node = node
    }

    /// Parses one app message and returns the encoded reply. Never throws: the
    /// caller always receives a structured answer, including for bad input.
    func handle(_ payload: Data) -> Data {
        guard !payload.isEmpty, payload.count <= Self.maximumMessageBytes else {
            return Self.failure(
                code: Fv6Status.invalidArgument.rawValue,
                command: nil,
                message: "message length \(payload.count) is outside 1...\(Self.maximumMessageBytes)"
            )
        }
        guard let object = (try? JSONSerialization.jsonObject(with: payload)) as? [String: Any] else {
            return Self.failure(
                code: Fv6Status.invalidArgument.rawValue,
                command: nil,
                message: "message is not a JSON object"
            )
        }
        guard let version = object["v"] as? Int, version == Self.version else {
            return Self.failure(
                code: Fv6Status.invalidArgument.rawValue,
                command: nil,
                message: "unsupported message version"
            )
        }
        guard let name = object["cmd"] as? String, let command = Command(rawValue: name) else {
            return Self.failure(
                code: Fv6Status.invalidArgument.rawValue,
                command: nil,
                message: "unknown command"
            )
        }
        guard let node = node() else {
            return Self.failure(
                code: Fv6Status.state.rawValue,
                command: command.rawValue,
                message: "node is not running"
            )
        }

        do {
            switch command {
            case .status:
                var result: [String: Any] = ["lastError": node.lastError()]
                result["phase"] = node.phase.map { Int($0.rawValue) } ?? -1
                return try Self.success(command, result)
            case .snapshot:
                let data = try node.snapshotJSON(timeout: Self.snapshotTimeout)
                guard let snapshot = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else {
                    throw Fv6Failure(status: .invalidArgument, message: "snapshot is not a JSON object")
                }
                return try Self.success(command, snapshot)
            case .execute:
                guard let request = object["request"] as? String,
                      let requestData = request.data(using: .utf8),
                      (try? JSONSerialization.jsonObject(with: requestData)) is [String: Any]
                else {
                    return Self.failure(
                        code: Fv6Status.invalidArgument.rawValue,
                        command: command.rawValue,
                        message: "execute requires a JSON object in \"request\""
                    )
                }
                let outcome = try node.execute(requestJSON: requestData, timeout: Self.executeTimeout)
                guard let result = (try? JSONSerialization.jsonObject(with: outcome)) as? [String: Any] else {
                    throw Fv6Failure(status: .invalidArgument, message: "command outcome is not a JSON object")
                }
                return try Self.success(command, result)
            case .pathChanged:
                try node.networkPathChanged(timeout: Self.pathChangeTimeout)
                return try Self.success(command, [:])
            }
        } catch let failure as Fv6Failure {
            return Self.failure(code: failure.code, command: command.rawValue, message: failure.message)
        } catch {
            return Self.failure(
                code: Fv6Status.io.rawValue,
                command: command.rawValue,
                message: error.localizedDescription
            )
        }
    }

    private static func success(_ command: Command, _ result: Any) throws -> Data {
        let payload: [String: Any] = [
            "ok": true,
            "v": version,
            "cmd": command.rawValue,
            "result": result,
        ]
        return try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])
    }

    private static func failure(code: Int32, command: String?, message: String) -> Data {
        let error: [String: Any] = ["code": Int(code), "message": message]
        var payload: [String: Any] = [
            "ok": false,
            "v": version,
            "error": error,
        ]
        if let command {
            payload["cmd"] = command
        }
        return (try? JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys]))
            ?? Data("{\"ok\":false,\"v\":1}".utf8)
    }
}
