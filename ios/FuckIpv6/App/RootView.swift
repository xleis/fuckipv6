import SwiftUI

/// Minimal developer host: configure, connect, disconnect, inspect status.
/// Product UI is deliberately out of scope (P5).
struct RootView: View {
    @StateObject private var controller = VPNController()
    @State private var ipcReply: String?

    var body: some View {
        NavigationStack {
            Form {
                Section("网络配置") {
                    TextField("网络名称", text: $controller.configuration.networkName)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    TextField("Coordinator URL", text: $controller.configuration.coordinatorURL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                    TextField("DNS（逗号分隔）", text: Binding(
                        get: { controller.configuration.dnsServers.joined(separator: ",") },
                        set: { controller.configuration.dnsServers = $0
                            .split(separator: ",")
                            .map { $0.trimmingCharacters(in: .whitespaces) }
                            .filter { !$0.isEmpty } }
                    ))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                }

                Section("操作") {
                    Button("保存配置") {
                        Task { await controller.saveConfiguration() }
                    }
                    Button("连接") {
                        Task { await controller.connect() }
                    }
                    .disabled(controller.isConnected || controller.isBusy)
                    Button("断开", role: .destructive) {
                        Task { await controller.disconnect() }
                    }
                    .disabled(!controller.isConnected && !controller.isBusy)
                }

                Section("VPN 状态") {
                    LabeledContent("连接状态", value: statusText)
                    LabeledContent("隧道阶段", value: controller.tunnelStatus?.phase.rawValue ?? "unknown")
                    if let message = controller.tunnelStatus?.message {
                        Text(message).font(.footnote).foregroundStyle(.secondary)
                    }
                    if let error = controller.errorMessage {
                        Text(error).font(.footnote).foregroundStyle(.red)
                    }
                }

                Section("节点快照") {
                    ScrollView {
                        Text(controller.tunnelStatus?.snapshotJSON ?? "尚无快照")
                            .font(.system(.footnote, design: .monospaced))
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .frame(maxHeight: 240)
                }
                Section("Provider IPC") {
                    Button("查询状态") {
                        Task { ipcReply = await controller.sendProviderMessage(["v": 1, "cmd": "status"]) }
                    }
                    .disabled(!controller.isConnected)
                    Button("读取快照") {
                        Task { ipcReply = await controller.sendProviderMessage(["v": 1, "cmd": "snapshot"]) }
                    }
                    .disabled(!controller.isConnected)
                    ScrollView {
                        Text(ipcReply ?? "尚无回复")
                            .font(.system(.footnote, design: .monospaced))
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .frame(maxHeight: 200)
                }
            }
            .navigationTitle("fuckipv6 P3")
            .task { await controller.refresh() }
            .refreshable { await controller.refresh() }
        }
    }

    private var statusText: String {
        switch controller.vpnStatus {
        case .invalid: return "invalid"
        case .disconnected: return "disconnected"
        case .connecting: return "connecting"
        case .connected: return "connected"
        case .reasserting: return "reasserting"
        case .disconnecting: return "disconnecting"
        @unknown default: return "unknown"
        }
    }
}

#Preview {
    RootView()
}
