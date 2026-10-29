# Windows 快速开始指南

## 5分钟快速测试

### 前置条件

确保你已经安装：
- ✅ Windows 10/11
- ✅ Rust (从 https://rustup.rs/ 安装)
- ✅ 管理员权限

### 步骤 1: 下载并编译

```powershell
# 以管理员身份运行 PowerShell
cd path\to\fuckipv6

# 运行安装脚本
.\setup_windows.ps1
```

脚本会自动：
1. 下载 wintun.dll
2. 编译项目
3. 复制文件到 target\release\

### 步骤 2: 生成配置

```powershell
cd target\release

# 生成默认配置文件
.\fuckipv6.exe init-config
```

配置文件位置: `%USERPROFILE%\.config\fuckipv6\default.toml`

### 步骤 3: 启动服务器（在一台机器上）

```powershell
# 启动协调服务器
.\fuckipv6-coordinator.exe
```

你应该看到：
```
启动协调服务器...
WebSocket 服务器监听: 0.0.0.0:6666
```

### 步骤 4: 启动客户端 A（在第一台机器）

```powershell
# 编辑配置文件，设置服务器地址
notepad %USERPROFILE%\.config\fuckipv6\default.toml

# 修改这一行（把 SERVER_IP 改成服务器的实际 IP）:
# coordinator_url = "ws://SERVER_IP:6666"

# 启动客户端（需要管理员权限）
.\fuckipv6.exe start
```

你应该看到：
```
启动客户端: ...
客户端 ID: xxxxx
已连接到协调服务器
已分配虚拟 IP: fd00::1001
TUN 设备已创建: fuckipv6
客户端启动成功，进入主循环
```

### 步骤 5: 启动客户端 B（在第二台机器）

在另一台 Windows 机器上重复步骤 1-4。

第二个客户端会得到不同的 IP，例如 `fd00::1002`

### 步骤 6: 测试连接！

在客户端 A 上：
```powershell
# Ping 客户端 B 的虚拟 IP
ping -6 fd00::1002
```

在客户端 B 上：
```powershell
# Ping 客户端 A 的虚拟 IP
ping -6 fd00::1001
```

如果 ping 通了，恭喜！你已经成功建立了 IPv6 虚拟局域网！🎉

## 故障排除

### "需要管理员权限"
- 右键 PowerShell → "以管理员身份运行"

### "无法加载 wintun.dll"
```powershell
# 检查文件是否存在
dir wintun.dll
dir fuckipv6.exe

# 确保它们在同一目录
copy wintun.dll target\release\
```

### "无法连接到协调服务器"
```powershell
# 测试网络连通性
Test-NetConnection -ComputerName SERVER_IP -Port 6666

# 检查防火墙
netsh advfirewall firewall show rule name=all | findstr 6666
```

### Ping 不通
1. **检查两个客户端是否都连接到服务器**
   - 查看服务器日志，应该显示两个客户端已注册

2. **检查虚拟 IP 配置**
   ```powershell
   ipconfig | findstr fd00
   ```
   应该看到分配的虚拟 IP

3. **检查路由表**
   ```powershell
   netsh interface ipv6 show route | findstr fd00
   ```
   应该看到 fd00::/64 的路由

4. **检查防火墙（允许 ICMPv6）**
   ```powershell
   # 允许 ICMPv6 Echo Request
   netsh advfirewall firewall add rule name="ICMPv6 Echo Request" protocol=icmpv6:128,any dir=in action=allow

   # 允许 ICMPv6 Echo Reply
   netsh advfirewall firewall add rule name="ICMPv6 Echo Reply" protocol=icmpv6:129,any dir=in action=allow
   ```

## 网络拓扑示例

### 场景 1: 本地测试（单机双客户端）

```
┌─────────────────────────────┐
│      Windows PC (你的电脑)    │
│                             │
│  ┌──────────────────────┐   │
│  │  Coordinator         │   │
│  │  127.0.0.1:6666      │   │
│  └──────────────────────┘   │
│           ↑                 │
│           │ WebSocket       │
│      ┌────┴────┐            │
│      │         │            │
│  ┌───▼──┐  ┌──▼───┐        │
│  │ 客户端A│  │客户端B│        │
│  │fd00::│  │fd00::│        │
│  │ 1001 │  │ 1002 │        │
│  └──────┘  └──────┘        │
│      ↓         ↓            │
│  ┌──────────────────┐      │
│  │   TUN 设备        │      │
│  │   fuckipv6       │      │
│  └──────────────────┘      │
└─────────────────────────────┘
```

启动方法：
```powershell
# 终端 1 (协调服务器)
.\fuckipv6-coordinator.exe

# 终端 2 (客户端 A) - 以管理员运行
.\fuckipv6.exe start

# 终端 3 (客户端 B) - 以管理员运行
# 需要修改配置使用不同的 TUN 设备名
# 或者使用不同的配置文件
.\fuckipv6.exe start --config another-config.toml
```

### 场景 2: 局域网测试

```
┌──────────────┐         ┌──────────────┐
│ Windows PC 1 │         │ Windows PC 2 │
│              │         │              │
│ Coordinator  │◄────────┤  客户端 B    │
│ + 客户端 A    │ LAN     │  fd00::1002  │
│ fd00::1001   │         │              │
│              │         │              │
└──────────────┘         └──────────────┘
     ↑                        ↑
     │                        │
     └────── UDP P2P ─────────┘
```

### 场景 3: 互联网测试

```
┌──────────────────┐
│   云服务器        │
│ (公网 IP)        │
│                  │
│  Coordinator     │
│  :6666           │
└────────┬─────────┘
         │
    Internet
         │
    ┌────┴──────┐
    │           │
┌───▼──┐    ┌──▼───┐
│家庭A  │    │家庭B  │
│(NAT) │    │(NAT) │
│      │    │      │
│客户端A│    │客户端B│
└──────┘    └──────┘
```

## 高级配置

### 自定义 TUN 设备名称

编辑 `default.toml`:
```toml
[tun]
name = "my_vpn"  # 默认是 "fuckipv6"
mtu = 1280
```

### 自定义监听端口

```toml
[p2p]
listen_port = 0  # 0 表示随机端口
```

### 调整日志级别

```powershell
# 设置环境变量
$env:RUST_LOG="debug"  # 或 info, warn, error

# 启动客户端
.\fuckipv6.exe start
```

## 下一步

恭喜完成快速开始！现在你可以：

1. 📖 阅读完整文档: `README_WINDOWS.md`
2. 🔧 探索更多功能: `.\fuckipv6.exe --help`
3. 🐛 报告问题: https://github.com/yourusername/fuckipv6/issues
4. 🚀 等待 Phase 2 功能:
   - NAT 穿透
   - QUIC 加密
   - Web 管理界面

## 清理

如果想卸载，运行：

```powershell
# 停止客户端 (Ctrl+C)

# 删除 TUN 设备
Remove-NetAdapter -Name "fuckipv6" -Confirm:$false

# 删除配置
Remove-Item -Recurse "$env:USERPROFILE\.config\fuckipv6"
```

---

**问题？** 查看 [README_WINDOWS.md](README_WINDOWS.md) 获取详细故障排除指南。
