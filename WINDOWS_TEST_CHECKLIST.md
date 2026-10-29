# Windows 测试清单

在 Windows 上进行第一次测试前，请按照此清单逐项确认。

## 环境准备

### ✅ 基础环境
- [ ] Windows 10 或 Windows 11（推荐 Windows 11）
- [ ] 已安装 Rust 工具链（`cargo --version` 能正常显示）
- [ ] 拥有管理员权限
- [ ] 关闭或配置防火墙允许程序通信

### ✅ 网络环境
- [ ] 至少准备 2 台 Windows 机器（或 1 台 + 虚拟机）
- [ ] 确保机器之间网络互通
- [ ] 如果使用云服务器作为协调器，确保端口 6666 开放

## 编译步骤

### ✅ 下载依赖
```powershell
# 1. 克隆或下载项目
cd fuckipv6

# 2. 以管理员身份运行 PowerShell

# 3. 运行安装脚本
.\setup_windows.ps1
```

**检查点：**
- [ ] wintun.dll 已下载到项目根目录
- [ ] 编译成功，无错误
- [ ] target\release\ 目录包含：
  - [ ] fuckipv6.exe
  - [ ] fuckipv6-coordinator.exe
  - [ ] wintun.dll

### ✅ 常见编译问题

**问题 1: "cargo: 命令未找到"**
```powershell
# 解决：重新安装 Rust
# 下载并运行: https://rustup.rs/
```

**问题 2: 编译卡住或很慢**
```powershell
# 解决：清理并重试
cargo clean
cargo build --release
```

**问题 3: wintun.dll 下载失败**
```powershell
# 手动下载
# 1. 访问: https://www.wintun.net/
# 2. 下载 wintun-0.14.1.zip
# 3. 解压并复制 wintun\bin\amd64\wintun.dll 到项目根目录
# 4. 重新运行: .\setup_windows.ps1 -SkipWinTun
```

## 协调服务器测试

### ✅ 启动服务器

在**服务器机器**上（或其中一台客户端机器）：

```powershell
cd target\release
.\fuckipv6-coordinator.exe
```

**预期输出：**
```
[INFO] 启动协调服务器
[INFO] WebSocket 服务器监听: 0.0.0.0:6666
```

**检查点：**
- [ ] 无错误信息
- [ ] 显示监听端口 6666
- [ ] 程序持续运行（不退出）

### ✅ 测试服务器连通性

在**客户端机器**上测试：

```powershell
# 替换 SERVER_IP 为服务器的实际 IP
Test-NetConnection -ComputerName SERVER_IP -Port 6666
```

**预期结果：**
- [ ] TcpTestSucceeded: True

**如果连接失败：**
```powershell
# 在服务器上添加防火墙规则
New-NetFirewallRule -DisplayName "fuckipv6 Coordinator" -Direction Inbound -Protocol TCP -LocalPort 6666 -Action Allow
```

## 客户端测试

### ✅ 生成配置文件

在**每台客户端机器**上：

```powershell
cd target\release
.\fuckipv6.exe init-config
```

**检查点：**
- [ ] 显示配置文件路径
- [ ] 文件已创建：`%USERPROFILE%\.config\fuckipv6\default.toml`

### ✅ 修改配置

编辑配置文件：
```powershell
notepad %USERPROFILE%\.config\fuckipv6\default.toml
```

**必须修改：**
```toml
[network]
coordinator_url = "ws://SERVER_IP:6666"  # 改为服务器实际IP
```

**检查点：**
- [ ] coordinator_url 已正确设置
- [ ] 保存并关闭文件

### ✅ 启动客户端 A

在**第一台客户端机器**上（以管理员身份）：

```powershell
# 确保在管理员 PowerShell 中
cd target\release
.\fuckipv6.exe start
```

**预期输出（关键行）：**
```
[INFO] 启动客户端: ...
[INFO] 客户端 ID: xxxxx
[INFO] 正在连接协调服务器: ws://SERVER_IP:6666
[INFO] 已连接到协调服务器
[INFO] 等待获取虚拟 IP 地址...
[INFO] 已分配虚拟 IP: fd00::1001
[INFO] 正在创建 Windows TUN 设备: fuckipv6
[INFO] WinTun 驱动已加载
[INFO] WinTun 会话已启动
[INFO] TUN 设备已创建: fuckipv6
[INFO] 客户端启动成功，进入主循环
```

**检查点：**
- [ ] 无错误信息
- [ ] 已分配虚拟 IP（记录下来，例如 fd00::1001）
- [ ] TUN 设备创建成功
- [ ] 程序持续运行

**常见错误排查：**

**错误 1: "需要管理员权限"**
```powershell
# 解决：重新以管理员身份打开 PowerShell
# 右键 PowerShell 图标 -> "以管理员身份运行"
```

**错误 2: "无法加载 wintun.dll"**
```powershell
# 检查文件
dir wintun.dll
# 如果不存在，从项目根目录复制
copy ..\..\wintun.dll .
```

**错误 3: "无法连接到协调服务器"**
```powershell
# 检查配置
type %USERPROFILE%\.config\fuckipv6\default.toml | findstr coordinator_url

# 测试连接
Test-NetConnection -ComputerName SERVER_IP -Port 6666
```

### ✅ 验证网络配置

在客户端 A 上，打开**新的** PowerShell 窗口：

```powershell
# 查看虚拟 IP
ipconfig | findstr "fd00"

# 预期输出：
#   IPv6 地址 . . . . . . . . . . . . : fd00::1001

# 查看路由
netsh interface ipv6 show route | findstr fd00

# 预期输出：
#   发布  否              手动    256     fd00::/64                    fuckipv6

# 查看 TUN 设备
Get-NetAdapter | Where-Object {$_.Name -like "*fuckipv6*"}

# 预期输出：
# Name          InterfaceDescription          ifIndex Status
# ----          --------------------          ------- ------
# fuckipv6      WinTun Userspace Tunnel       XX      Up
```

**检查点：**
- [ ] 能看到虚拟 IP (fd00::1001)
- [ ] 能看到路由 (fd00::/64)
- [ ] TUN 设备状态为 Up

### ✅ 测试本机 Ping

```powershell
# Ping 自己的虚拟 IP
ping -6 fd00::1001
```

**预期输出：**
```
正在 Ping fd00::1001 ...
来自 fd00::1001 的回复: 时间<1ms
来自 fd00::1001 的回复: 时间<1ms
...
```

**检查点：**
- [ ] Ping 成功
- [ ] 延迟 < 1ms

**如果 Ping 失败：**
```powershell
# 检查防火墙（允许 ICMPv6）
netsh advfirewall firewall add rule name="ICMPv6" protocol=icmpv6:any,any dir=in action=allow
```

### ✅ 启动客户端 B

在**第二台客户端机器**上重复上述步骤：
1. 生成配置
2. 修改 coordinator_url
3. 启动客户端

**检查点：**
- [ ] 客户端 B 启动成功
- [ ] 获得不同的虚拟 IP（记录下来，例如 fd00::1002）

### ✅ 查看服务器日志

在**服务器**上，应该看到类似输出：
```
[INFO] 新客户端连接: xxxxx
[INFO] 客户端已注册: xxxxx, IP: fd00::1001
[INFO] 新客户端连接: yyyyy
[INFO] 客户端已注册: yyyyy, IP: fd00::1002
```

**检查点：**
- [ ] 两个客户端都显示已注册
- [ ] IP 地址正确

### ✅ 查看客户端 A 日志

在客户端 A 上，应该看到：
```
[INFO] 节点上线: yyyyy (fd00::1002)
[INFO] 添加对等节点: fd00::1002 (地址: xxx.xxx.xxx.xxx:xxxx)
```

**检查点：**
- [ ] 看到客户端 B 上线通知
- [ ] 对等节点已添加

## P2P 连接测试

### ✅ 跨节点 Ping 测试

**在客户端 A 上：**
```powershell
# Ping 客户端 B
ping -6 fd00::1002
```

**预期输出：**
```
正在 Ping fd00::1002 ...
来自 fd00::1002 的回复: 时间=XXms
来自 fd00::1002 的回复: 时间=XXms
...

Ping 统计信息:
    数据包: 已发送 = 4，已接收 = 4，丢失 = 0 (0% 丢失)
```

**检查点：**
- [ ] Ping 成功
- [ ] 0% 丢包
- [ ] 延迟合理（局域网 < 10ms，广域网 < 100ms）

**在客户端 B 上：**
```powershell
# Ping 客户端 A
ping -6 fd00::1001
```

**检查点：**
- [ ] 双向 Ping 都成功

### ✅ 大包传输测试

```powershell
# 发送大包（1200 字节）
ping -6 -l 1200 fd00::1002

# 发送 100 个包
ping -6 -n 100 fd00::1002
```

**检查点：**
- [ ] 大包传输成功
- [ ] 100 个包丢包率 < 1%

### ✅ 查看客户端日志

在客户端 A 和 B 的日志中，应该看到：
```
[DEBUG] TUN packet: fd00::1001 -> fd00::1002, XXX bytes
[DEBUG] Received packet from fd00::1002 -> fd00::1001
```

**检查点：**
- [ ] 能看到数据包收发记录
- [ ] 数据包方向正确

## 性能测试（可选）

### ✅ 带宽测试

使用 iperf3（需要单独安装）：

**在客户端 B 上：**
```powershell
# 安装 iperf3（使用 Chocolatey）
choco install iperf3

# 启动服务器
iperf3 -s -B fd00::1002
```

**在客户端 A 上：**
```powershell
# 测试上传带宽
iperf3 -c fd00::1002 -V

# 测试下载带宽
iperf3 -c fd00::1002 -V -R
```

**检查点：**
- [ ] 带宽 > 10 Mbps（局域网应该更高）
- [ ] 无错误

### ✅ 延迟测试

```powershell
# 持续 Ping 100 次
ping -6 -n 100 fd00::1002

# 查看统计信息
# 最短 = XXms，最长 = XXms，平均 = XXms
```

**检查点（局域网）：**
- [ ] 平均延迟 < 5ms
- [ ] 最大延迟 < 10ms

**检查点（广域网）：**
- [ ] 平均延迟 < 50ms
- [ ] 最大延迟 < 100ms

## 稳定性测试

### ✅ 长时间运行

```powershell
# 持续 Ping 10 分钟（600 次，每秒 1 次）
ping -6 -n 600 -w 1000 fd00::1002
```

**检查点：**
- [ ] 丢包率 < 0.5%
- [ ] 客户端程序未崩溃
- [ ] 延迟稳定

### ✅ 重连测试

**步骤：**
1. 停止客户端 B（Ctrl+C）
2. 观察客户端 A 和服务器日志
3. 重新启动客户端 B
4. Ping 测试

**检查点：**
- [ ] 客户端 A 收到下线通知
- [ ] 客户端 B 重连成功
- [ ] Ping 恢复正常

### ✅ 优雅退出测试

**在客户端上：**
1. 按 `Ctrl+C`
2. 观察日志

**预期输出：**
```
[INFO] 收到停止信号，正在优雅退出...
[INFO] 正在关闭信令连接...
[INFO] 正在删除 TUN 设备: fuckipv6
[INFO] TUN 设备已删除
[INFO] 客户端已停止
```

**检查点：**
- [ ] 程序正常退出（不是崩溃）
- [ ] TUN 设备已删除
- [ ] 无残留进程

**验证清理：**
```powershell
# 检查 TUN 设备
Get-NetAdapter | Where-Object {$_.Name -like "*fuckipv6*"}
# 应该为空

# 检查路由
netsh interface ipv6 show route | findstr fd00
# 应该没有 fd00 相关路由
```

## 多客户端测试（3+ 节点）

如果有 3 台或更多机器：

### ✅ 启动客户端 C

**检查点：**
- [ ] 客户端 C 获得新的 IP（例如 fd00::1003）
- [ ] 客户端 A 和 B 都收到客户端 C 上线通知

### ✅ 全连通性测试

**从客户端 A：**
```powershell
ping -6 fd00::1002  # Ping B
ping -6 fd00::1003  # Ping C
```

**从客户端 B：**
```powershell
ping -6 fd00::1001  # Ping A
ping -6 fd00::1003  # Ping C
```

**从客户端 C：**
```powershell
ping -6 fd00::1001  # Ping A
ping -6 fd00::1002  # Ping B
```

**检查点：**
- [ ] 所有节点之间都能 Ping 通
- [ ] 延迟合理
- [ ] 无丢包

## 测试完成！

如果所有检查点都通过，恭喜你！fuckipv6 在 Windows 上运行正常 🎉

## 问题报告

如果遇到问题，请收集以下信息：

### 系统信息
```powershell
# Windows 版本
systeminfo | findstr /B /C:"OS Name" /C:"OS Version"

# Rust 版本
cargo --version
rustc --version

# 网络配置
ipconfig /all
```

### 日志
```powershell
# 启用详细日志
$env:RUST_LOG="debug"
.\fuckipv6.exe start > log.txt 2>&1
```

### 提交 Issue
访问 https://github.com/yourusername/fuckipv6/issues 并附上：
- [ ] 系统信息
- [ ] 完整日志
- [ ] 重现步骤
- [ ] 预期行为 vs 实际行为

---

**祝测试顺利！**
