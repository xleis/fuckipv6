# fuckipv6 下一代系统演进与子 Agent 并行开发规划 (NEXTGEN_TODO.md)

## 总体架构目标
将 `fuckipv6` 从基础 P2P 虚拟局域网演进为：**具备原生智能分流/Exit Node 翻墙能力、单一二进制自适应 Tauri/WebUI、节点完全对称（既是客户端也是服务端）、具备 3-Hop 洋葱多跳与海量 P2P 基础设施能力的下一代去中心化网络系统**。

---

## 5 大子 Agent 并行开工任务分工表

```mermaid
flowchart TD
    subgraph Agent1 ["Agent 1: 智能分流与规则引擎"]
        A1_1["实现纯 Rust CN-CIDR / GeoIP 基数树 Trie"]
        A1_2["分流规则定义 (Direct / ExitNode / Block)"]
        A1_3["TUN 流量目的地智能判定与分流路由表"]
    end

    subgraph Agent2 ["Agent 2: Exit Node 出口网关与用户态 NAT"]
        A2_1["信令/对端增加 exit_node 声明与协商"]
        A2_2["纯 Rust 用户态公网 NAT 转发 (smoltcp/纯Rust)"]
        A2_3["出口访问白名单与公钥鉴权"]
    end

    subgraph Agent3 ["Agent 3: 单二进制双模融合 (Tauri + WebUI)"]
        A3_1["运行时环境自适应判定 (Display / Wayland / Headless)"]
        A3_2["统一前端静态资源内嵌 (rust-embed)"]
        A3_3["桌面端加载 Tauri，无头端加载 Axum WebUI"]
    end

    subgraph Agent4 ["Agent 4: 对称对等节点网络与一键建网"]
        A4_1["协调端 Actor 化并嵌入 client 二进制"]
        A4_2["一键建网命令与 Ticket 凭证 (fk://...) 生成与解析"]
        A4_3["单节点多网络 (Multi-Network) 并发共存"]
    end

    subgraph Agent5 ["Agent 5: 洋葱多跳路由与海量 P2P 基础设施"]
        A5_1["3-Hop 电路协商协议与分层加密封包 (洋葱剥皮)"]
        A5_2["Kademlia DHT 无中心分布式寻址"]
        A5_3["基于 Blake3 Merkle-Tree 的 P2P 分块文件传输"]
    end
```

---

## 任务执行清单与开发规范

### 🟢 Agent 1 任务清单：智能分流与规则引擎 ✅ **已完成**
- [x] **1.1** 在 `crates/common/src/routing.rs` 定义路由规则结构：
  - `RoutingAction`: `Direct`（直连）、`Proxy`（走指定 ExitNode）、`Mesh`（对等内网）、`Reject`（阻断）。
  - `RoutingMode`: `MeshOnly`、`BypassMainland`（绕过大陆）、`GlobalProxy`（全局代理）。
  - `RoutingConfig` & `ExitNodeConfig`: 结构化配置。
- [x] **1.2** 在 `crates/common/src/routing.rs` 实现高性能嵌入式纯 Rust CN-CIDR 快速匹配 Trie（支持三大运营商 IPv4/IPv6 聚合网段，零外部依赖）。
- [x] **1.3** 改造 TUN 读取包处理流程：当捕获非虚拟内网目标 IP（如访问公网）时，根据规则决定直接由本地物理网卡发送还是封装后投递至 Exit Node。
- [x] **1.4** 添加单元测试：覆盖 IPv4/IPv6 大陆 IP 命中、海外 IP 走 Proxy、内网虚拟 IP 走 Mesh 的完整测试用例。

### 🟢 Agent 2 任务清单：Exit Node 出口网关与用户态 NAT ✅ **已完成**
- [x] **2.1** 在 `crates/common/src/config.rs` 中为 `ClientConfig` 扩展 `routing` 与 `exit_node` 声明。
- [x] **2.2** 在 `crates/client/src/routing/exit_node.rs` 实现出口网关管理器 `ExitNodeManager`：支持按虚拟 IPv6、虚拟 IPv4 及 ClientId 动态匹配出口节点，并实施客户端公钥指纹白名单鉴权。
- [x] **2.3** 在 TUN 数据包接收流程中，当本机声明为 Exit Node 且收到发往公网的目的 IP 时，写入 TUN 触发操作系统内核 NAT/转发。
- [x] **2.4** 集成与端到端测试：在境外服务器 `relay.xlei.cc` 实测开启 Exit Node、启用内核 IP 转发与 MASQUERADE NAT 规则，服务平稳运行。

### 🟢 Agent 3 任务清单：单一二进制自适应双模 (Tauri + WebUI) ✅ **已完成**
- [x] **3.1** 运行期自适应环境检测：
  - 启动时检测参数（`--headless` / `--gui`）。
  - 未指定时自动检测操作系统显示环境：Linux 下检测 `DISPLAY` 与 `WAYLAND_DISPLAY` 环境变量；Windows/macOS 检测交互式桌面会话。
- [x] **3.2** 统一前端资产打包与双向暴露：
  - 无头环境或服务器端自动启动内置 Axum WebUI（默认 `8088` 端口，可通过 `--webui-port` 指定），提供现代可视化实时仪表盘。
  - 桌面环境桥接 Tauri 原生窗口与控制台提示。
- [x] **3.3** REST API 与实时状态控制打通：
  - `GET /api/status`：提供节点双栈虚拟 IP、运行状态、当前分流模式、出口节点。
  - `GET /api/peers`：实时展示对等节点、延迟及连接链路类型（直连/中继）。
  - `POST /api/routing`：零停机热切换分流策略（`MeshOnly` / `BypassMainland` / `GlobalProxy`）与出口节点。

### 🟢 Agent 4 任务清单：对称对等节点网络与一键建网 ✅ **已完成**
- [x] **4.1** 融合架构改造：
  - 提炼协调服务端与 QUIC 中继为轻量内嵌式 `EmbeddedCoordinator` Actor，消除循环依赖，赋能客户端进程直接建网。
- [x] **4.2** 一键建网与 Ticket 协议：
  - 新增命令 `fuckipv6 host --name <NAME>`：本地初始化虚拟网络，自动生成 `fk://<pubkey>@<host:port>?name=<NAME>&v6=...` 邀请凭证，并支持本机自动连接。
  - 新增命令 `fuckipv6 join <Ticket>`：原生自动识别与解析 `fk://` 邀请凭证，自动配置协调服务连接与默认网络。
- [x] **4.3** 多网络并发支持：单个客户端实例可维护多个网络配置与虚拟通道，根据网络名称派生独立 TUN 接口（`fk-<net_name>`），实现内核级路由隔离、P2P 会话隔离与统一 IPC/WebUI 状态聚合。

### 🟢 Agent 5 任务清单：洋葱多跳路由与海量 P2P 基础设施 ✅ **已完成**
- [x] **5.1** 洋葱电路协商协议：
  - 在 `crates/common/src/onion.rs` 实现 3-Hop 多层嵌套密钥协商协议。
  - 发送端采用多层 ChaCha20-Poly1305 进行洋葱封包，每个中继节点仅解密自己一层并获取下一跳路由，实现彻底的通信链路去匿名化。
- [x] **5.2** Kademlia 分布式哈希表（DHT）：
  - 实现无中心 Peer 存储与发现，无需依赖固定中心协调服务器（256-bit XOR 空间、256 K-Buckets、LRU 缓存机制与 TTL 到期垃圾回收）。
- [x] **5.3** P2P 加密消息与内容分块传输：
  - 基于 Blake3 分块哈希，实现点对点大文件高速校验与多源并行拉取。
  - 基于节点公钥地址实现端到端加密的点对点消息推送（临时 X25519 前向安全、Ed25519 签名与防篡改验证）。
  - 在客户端引擎集成 `P2pOverlay` 统一调度信元解密分发。

---

## 开发质量红线与要求
1. **100% 独立自主**：绝不依赖外部黑盒二进制、脚本或运行时动态下载 DLL。
2. **全平台编译兼容**：任何代码变更必须保证 Linux musl (`x86_64-unknown-linux-musl`) 与 Windows GNU (`x86_64-pc-windows-gnu`) 均编译无报错。
3. **严格质量门禁**：`cargo check --workspace`、`cargo clippy --workspace --all-targets -- -D warnings` 与 `cargo test --workspace` 必须 100% 通过。
