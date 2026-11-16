import SwiftUI

/// Placeholder screen. The iOS client is not implemented yet: every action is
/// intentionally absent until the shared node contract is wired in.
struct RootView: View {
    var body: some View {
        NavigationStack {
            ContentUnavailableView {
                Label("iOS 客户端尚未实现", systemImage: "hammer")
            } description: {
                Text("占位骨架，没有可运行功能。后续通过 node-command-request-v1 / node-snapshot-v1 契约接入节点，不在此处复制网络与路由逻辑。")
            }
            .navigationTitle("fuckipv6")
        }
    }
}

#Preview {
    RootView()
}
