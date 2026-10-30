# LXT Public SYN-ACK Rescue Lab V4 — 两台真实 Linux 主机/虚拟机

实验用途：验证公网传输下，阻断**特定测试 TCP 服务端口的回程 SYN-ACK** 后，是否能够让已认证的 UDP Sidecar 把服务端**真实内核生成的 SYN-ACK**送到客户端，交给 Linux TUN 注入 TCP 协议栈，后续数据仍使用原生 TCP。

> **归档说明（2026-10-30）**：本文件是 V4 ZIP 自带的原始 README，其中“尚未经过两台真实公网主机的端到端测试”是发布时的状态。V4 后续已完成真实公网测试，验收 8/8 PASS，见 [`../../reports/LXT_V4_PUBLIC_TEST_REPORT.md`](../../reports/LXT_V4_PUBLIC_TEST_REPORT.md)。命令、端口和实现说明仍然有效。

**不要在朋友的生产代理服务器上直接尝试改防火墙。** 请确认对测试机拥有权限，只在你的客户端实验机上临时 DROP 指定 SYN-ACK。仅限 IPv4。原始包只在无网络特权环境完成静态检查/离线校验，可能存在设备/NAT/防火墙差异。

## A. 准备

- 服务器：Linux x86_64/arm64，有公网上行；root（仅 sidecar 需要）；开放受限入站 `TCP 39090`、`UDP 39091`，**尽可能只允许客户端的公网 IP**。不要改 SSH、防火墙默认策略、路由或系统代理。高位 TCP 端口必须未被占用。
- 客户端：**Linux**；Mac 建议在 Parallels Ubuntu VM 运行。需 `/dev/net/tun`、`iproute2` 和 `iptables`（仅受控阻断时）。macOS 原生不支持这套 Linux TUN 注入方式。
- 双端 Python 3.10+。为避免共享服务器上的额外攻击面，不使用 sudo 启动 echo 服务，只让 sidecar 拥有必要的 RAW/TUN 权限。
- 时钟尽量同步（协议认证要求时间偏差 < 30 秒）。
- 记录客户端/服务端公网 IPv4、实际网卡名。服务端捕获网卡须承载真实 TCP SYN-ACK 出口，不一定叫 `eth0`。

### 生成一致的预共享密钥

在你本地终端执行 `openssl rand -hex 32`，将 64 个十六进制字符安全复制到两端。不要将其发给本聊天，也不要写入日志/提交 Git。

在两端终端里分别执行：

```bash
export LXT_LAB_KEY='此处粘贴相同的64位十六进制密钥'
```

仅用于实验。服务端认证收到的 UDP HELLO，然后才允许根据同一公网 IPv4 匹配、转送 SYN-ACK。客户端会把经 HMAC 验证且 ACK 序列号匹配正在建立连接的 SYN-ACK，改写目标为客户端本地 IP/端口并注入 TUN。这解决了部分 NAT 转换问题，但**不能保证所有 CGNAT、TCP 代理、ECMP 多出口和防火墙环境都兼容**。

### 查网卡

```bash
ip -br -4 addr
ip -4 route get <对端公网IP>
```

以下用 `<SERVER_IP>` 表示朋友提供的测试服务器公网 IPv4，`<SERVER_NIC>`/`<CLIENT_NIC>` 表示实际网卡名；尖括号不要原样复制。

## B. 服务端启动（两个终端）

终端 1（普通用户运行 echo，默认每收到两条消息主动关连接以测试重建）：

```bash
python3 lxt_public_lab.py echo-server --bind 0.0.0.0 --tcp-port 39090 --close-after 2
```

终端 2（仅 sidecar 需要权限，须传入密钥环境变量）：

```bash
sudo env LXT_LAB_KEY="$LXT_LAB_KEY" python3 lxt_public_lab.py server-sidecar \
    --interface <SERVER_NIC> --tcp-port 39090 --udp-port 39091
```

旁路工具**不会更改服务器防火墙、iptables、SSH 或路由**。

## C. 客户端第一轮：先测试正常公网 TCP

```bash
python3 lxt_public_lab.py probe --server <SERVER_IP> --count 6 --timeout 4
```

如果不成功，先排查测试端口的防火墙/NAT/路由。不要继续做阻断测试。

## D. 客户端第二轮：Sidecar + 正常 TCP

终端 1 启动客户端 Sidecar：

```bash
sudo env LXT_LAB_KEY="$LXT_LAB_KEY" python3 lxt_public_lab.py client-sidecar \
    --interface <CLIENT_NIC> --server <SERVER_IP> \
    --tcp-port 39090 --udp-port 39091
```

观察到 `HELLO_ACK verified`，再在终端 2 执行一次正常 probe（验证 UDP 39091 可达）。

```bash
python3 lxt_public_lab.py probe --server <SERVER_IP> --count 6 --timeout 4
```

如果服务器没有出现 `HELLO verified` 或客户端没有 `HELLO_ACK verified`，先排查 UDP 路径，不要开启阻断。

## E. 第三轮：**仅客户端**临时阻断真实 SYN-ACK

> 只能在你自己的 Linux 客户端 VM 上执行！如果你通过 SSH 连接客户端，请不要使用阻断脚本。此脚本会给客户端 INPUT 插入**单条限定网卡、服务器 IPv4、TCP 源端口 39090、SYN+ACK 标志**的 DROP 规则，不影响 SSH/其他端口；结束时按精确规则删除。若脚本被 `kill -9` 或断电中止，仍可能残留，参见下方手动回滚。

先保持两个 Sidecar、echo 服务器持续运行；客户端另开一个终端：

```bash
sudo ./client_drop_once.sh <SERVER_IP> <CLIENT_NIC> \
    python3 lxt_public_lab.py probe --server <SERVER_IP> --count 6 --timeout 4
```

期待的成功证据：`[probe] ESTABLISHED ...`、`[client-sidecar] injected SYN-ACK`、`[server-sidecar] relayed SYN-ACK`、6/6 ACK。**客户侧 SYN-ACK 被防火墙阻止进入普通物理网卡的 TCP 栈，救援报文由独立 TUN 注入**。

再做一次**没有运行客户端 Sidecar**的阻断对照（先 Ctrl+C 停止客户端 Sidecar），同样的命令应超时；试验期间服务端 Sidecar 保持运行即可。

**重要区别：** 客户端 INPUT 阻断模拟的是“本机 TCP 栈收不到原始 SYN-ACK”，不是证明公网中间设备真的丢弃了 SYN-ACK。中间 NAT/防火墙仍有可能看到原始报文，所以受控实验通过**不代表**真实 GFW/跨境阻断一定可绕过。

### 如需手动回滚

```bash
sudo iptables -S INPUT | grep lxt-v4
```

如果脚本异常终止未清理，依据这条输出找到完整规则，使用 `sudo iptables -D INPUT <规则编号>` 删除**对应的那一条实验规则**，不要 `iptables -F` 或清空其他规则。不要在朋友服务器上清空任何防火墙规则。

## F. 真实线路试验与结果解释

若真实线路本身存在 TCP 握手回程异常，**不加人工 DROP 规则**，启动服务端和客户端 Sidecar，直接运行 probe；记录全程日志。

- 真实 SYN-ACK 在物理链路上丢弃，但 UDP 抵达，且后续 TCP 数据可通过：有机会救援。
- 有状态中间设备未观察到 SYN-ACK，可能丢弃第三次握手 ACK/后续数据；这类封锁无法通过简单的 UDP 转送解决。
- 客户端/服务端处于 NAT、运营商 TCP 状态跟踪、严格 `rp_filter`、内核防火墙或云厂商网络层时，实验可能失败，即便两端 Linux TUN 注入正常。
- 如果没有真实 SYN-ACK 丢包，实验不能证明它对上周故障机制有效。

### 关键诊断命令（两端自己执行，注意保护公网 IP 与密钥）

```bash
sudo tcpdump -ni <NIC> '(tcp port 39090 or udp port 39091)'
sudo ss -tnp '( sport = :39090 or dport = :39090 )'
```

客户端需要检查 `[client-sidecar] pending SYN`（本地原始 TCP SYN）、`HELLO_ACK verified`、`injected SYN-ACK`，服务端看 `relayed SYN-ACK`，TCP 服务端看 `accepted`。不输出共享密钥。

### 目前还没解决的问题

- 这只是 IPv4 / 单个服务器 TCP 测试端口 / 低并发 PoC；不是多路复用代理、TLS 或完整安全传输协议。
- 只认证 UDP 救援报文，**TCP echo 明文**，不能用于个人隐私、账号、真实业务。服务端 echo 应当限时开放并设置 IP 白名单。
- UDP HELLO 维持 NAT 映射，同时服务器根据观察到的客户端公网 IPv4 关联真实 TCP SYN-ACK；多客户端共享出口公网 IP 不安全且可能映射错误，目前只用于单客户端测试。
- 如果公网 IP 在 UDP/TCP 之间不同，或 NAT/代理改写 TCP ISN，本版本不能映射，需重新设计。
- TUN 注入可能被 `rp_filter` 和主机防火墙限制；不要为实验直接关闭服务器全局安全策略。可先在隔离客户端 VM 内检查 `sysctl net.ipv4.conf.all.rp_filter` 和 `net.ipv4.conf.lxtrescue0.rp_filter`。
- 云服务商禁止 raw socket、虚拟网卡与高位端口的情况下，需调整测试环境，不能承诺可用。

官方参考：Linux [`packet(7)`](https://man7.org/linux/man-pages/man7/packet.7.html) 和 [TUN/TAP](https://www.kernel.org/doc/html/latest/networking/tuntap.html)。
