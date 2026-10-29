# fuckipv6 - Windows 使用指南

## 系统要求

- Windows 10/11 (64位)
- Rust 工具链（1.70+）
- 管理员权限

## 安装步骤

### 方法 1: 自动安装（推荐）

1. **以管理员身份运行 PowerShell**
   - 右键点击 PowerShell 图标
   - 选择"以管理员身份运行"

2. **运行安装脚本**
   ```powershell
   cd path\to\fuckipv6
   .\setup_windows.ps1
   ```

   或者双击运行 `setup_windows.bat`（会自动请求管理员权限）

3. **等待编译完成**

   脚本会自动：
   - 下载 WinTun 驱动 (wintun.dll)
   - 编译项目
   - 复制必要的文件到 `target\release\`

### 方法 2: 手动安装

1. **安装 Rust**
   ```
   https://rustup.rs/
   ```

2. **下载 WinTun**
   - 访问: https://www.wintun.net/
   - 下载最新版本 (例如 wintun-0.14.1.zip)
   - 解压并复制 `wintun\bin\amd64\wintun.dll` 到项目根目录

3. **编译项目**
   ```powershell
   cargo build --release
   ```

4. **复制 wintun.dll**
   ```powershell
   copy wintun.dll target\release\
   ```

## 使用方法

### 1. 生成配置文件

```powershell
cd target\release
.\fuckipv6.exe init-config
```

这会在 `%USERPROFILE%\.config\fuckipv6\` 创建默认配置文件。

### 2. 编辑配置文件

打开 `%USERPROFILE%\.config\fuckipv6\default.toml`，修改协调服务器地址：

```toml
[network]
coordinator_url = "ws://your-server-ip:6666"
```

### 3. 启动协调服务器（在服务器上）

```powershell
.\fuckipv6-coordinator.exe
```

### 4. 启动客户端（需要管理员权限）

```powershell
# 以管理员身份运行 PowerShell
cd target\release
.\fuckipv6.exe start
```

### 5. 查看状态

```powershell
# 查看版本
.\fuckipv6.exe version

# 查看连接的节点（Phase 2）
.\fuckipv6.exe peers

# 查看网络统计
.\fuckipv6.exe stats

# Ping 其他节点（Phase 2）
.\fuckipv6.exe ping <peer-ip>
```

## 常见问题

### 1. "需要管理员权限才能创建 TUN 设备"

**解决方法：**
- 右键点击 PowerShell 或 CMD
- 选择"以管理员身份运行"
- 重新执行命令

### 2. "无法加载 wintun.dll"

**原因：** wintun.dll 不在程序目录或系统路径中

**解决方法：**
```powershell
# 确保 wintun.dll 和 fuckipv6.exe 在同一目录
dir wintun.dll
dir fuckipv6.exe

# 如果缺少，重新运行安装脚本
.\setup_windows.ps1 -SkipWinTun:$false
```

### 3. "无法连接到协调服务器"

**检查项：**
- 确认协调服务器正在运行
- 检查防火墙设置（允许 TCP 6666 端口）
- 验证配置文件中的服务器地址

```powershell
# 测试连接
Test-NetConnection -ComputerName your-server-ip -Port 6666
```

### 4. 防火墙配置

```powershell
# 允许 UDP 流量（P2P 通信）
New-NetFirewallRule -DisplayName "fuckipv6 P2P" -Direction Inbound -Protocol UDP -Action Allow

# 允许 TCP 流量（信令服务器）
New-NetFirewallRule -DisplayName "fuckipv6 Signaling" -Direction Inbound -Protocol TCP -LocalPort 6666 -Action Allow
```

### 5. 清理残留的网络适配器

如果程序异常退出，可能会残留 TUN 适配器：

```powershell
# 查看网络适配器
Get-NetAdapter | Where-Object {$_.Name -like "*fuckipv6*"}

# 删除适配器（需要管理员权限）
Remove-NetAdapter -Name "fuckipv6" -Confirm:$false
```

或者使用图形界面：
1. 打开 `ncpa.cpl`（网络连接）
2. 找到 "fuckipv6" 适配器
3. 右键 -> 删除

## 网络配置验证

### 检查 IPv6 地址

```powershell
# 查看所有 IPv6 地址
ipconfig | findstr "IPv6"

# 应该能看到类似这样的输出：
#   IPv6 地址 . . . . . . . . . . . . : fd00::1001
```

### 测试连接

```powershell
# Ping 自己的虚拟 IP
ping -6 fd00::1001

# Ping 其他节点（假设对方是 fd00::1002）
ping -6 fd00::1002
```

### 查看路由表

```powershell
# 显示 IPv6 路由
netsh interface ipv6 show route

# 应该能看到 fd00::/64 的路由
```

## 卸载

1. **停止客户端**
   - 按 `Ctrl+C` 停止运行中的客户端

2. **删除网络适配器**
   ```powershell
   Remove-NetAdapter -Name "fuckipv6" -Confirm:$false
   ```

3. **删除配置文件**
   ```powershell
   Remove-Item -Recurse -Force "$env:USERPROFILE\.config\fuckipv6"
   ```

4. **删除程序文件**
   ```powershell
   cd ..
   Remove-Item -Recurse -Force fuckipv6
   ```

## 开发调试

### 启用详细日志

设置环境变量启用 debug 日志：

```powershell
$env:RUST_LOG="debug"
.\fuckipv6.exe start
```

### 查看网络流量

使用 Wireshark 捕获 TUN 设备流量：
1. 打开 Wireshark
2. 选择 "fuckipv6" 适配器
3. 使用过滤器: `ipv6`

## 性能优化

### 调整 MTU

编辑配置文件 `default.toml`：

```toml
[tun]
mtu = 1280  # IPv6 最小 MTU，可以尝试 1500
```

### 调整心跳间隔

```toml
[p2p]
heartbeat_interval = 30  # 秒，减少可以更快检测断线
```

## 已知限制

1. **仅支持 IPv6**
   - 当前版本只支持 IPv6 虚拟网络
   - IPv4 支持计划在 Phase 2

2. **NAT 穿透**
   - Phase 1 仅支持直连（局域网或公网IP）
   - NAT 打洞功能在 Phase 2

3. **加密传输**
   - Phase 1 使用明文 UDP
   - QUIC 加密在 Phase 2

## 技术支持

- **项目主页**: https://github.com/yourusername/fuckipv6
- **提交 Issue**: https://github.com/yourusername/fuckipv6/issues
- **查看日志**: 程序会在控制台输出详细日志

## 更新日志

### v0.1.0 (Phase 1 MVP)
- ✅ Windows TUN 支持（WinTun）
- ✅ IPv6 地址分配
- ✅ P2P UDP 连接
- ✅ WebSocket 信令
- ✅ 管理员权限检查
- ✅ 优雅退出处理

### 即将推出 (Phase 2)
- NAT 穿透
- QUIC 加密
- 中继服务
- Web 管理界面
