# fuckipv6 Windows 版本发布说明

## 版本信息

- **版本**: v0.1.0 (Phase 1 MVP)
- **发布日期**: 2026-01-01
- **平台**: Windows 10/11 (64-bit)
- **编译方式**: Linux 交叉编译 (MinGW)

## 下载

📦 **发布包**: `fuckipv6-windows-v0.1.0.zip` (5.5 MB)

**SHA256 校验和**:
```
b978e72d43caf7752de654dcc674f432addbb4bb725416157aa337265054037a
```

## 包含文件

```
fuckipv6-windows/
├── fuckipv6.exe                    (8.2 MB) - 客户端
├── fuckipv6-coordinator.exe        (6.9 MB) - 服务器
├── setup_windows.ps1               - 自动安装脚本
├── setup_windows.bat               - 脚本启动器
├── README.txt                      - 快速说明
├── README_WINDOWS.md               - 完整文档
├── QUICKSTART_WINDOWS.md           - 快速开始指南
└── WINDOWS_TEST_CHECKLIST.md       - 测试清单
```

## ⚠️ 重要提示

**wintun.dll 未包含在发布包中**，需要手动下载：

1. 访问 https://www.wintun.net/
2. 下载 wintun-0.14.1.zip
3. 解压并复制 `wintun\bin\amd64\wintun.dll` 到程序目录

或者运行 `setup_windows.ps1` 会自动下载。

## 快速开始

### 1. 下载 WinTun
```powershell
# 方法 1: 运行安装脚本（自动下载）
.\setup_windows.ps1

# 方法 2: 手动下载
# 访问 https://www.wintun.net/ 并下载 wintun.dll
```

### 2. 生成配置
```powershell
.\fuckipv6.exe init-config
```

### 3. 启动服务器
```powershell
.\fuckipv6-coordinator.exe
```

### 4. 启动客户端（需要管理员权限）
```powershell
# 以管理员身份运行 PowerShell
.\fuckipv6.exe start
```

### 5. 测试连接
```powershell
ping -6 fd00::1002
```

详细步骤请参考 `QUICKSTART_WINDOWS.md`

## 新功能

### ✅ 完整的 Windows 支持

- **WinTun 驱动集成**: 使用官方 WinTun 驱动，高性能、低延迟
- **自动配置**: 自动配置 IPv6 地址和路由
- **管理员权限检查**: 自动检测并提示管理员权限
- **优雅退出**: Ctrl+C 正确清理所有资源

### ✅ 核心功能

- IPv6 虚拟局域网
- P2P UDP 连接
- WebSocket 信令协调
- Ed25519 身份认证
- 自动对等节点发现

## 已知限制

### Phase 1 限制

1. **仅支持直连**
   - 不支持 NAT 穿透（Phase 2）
   - 需要公网 IP 或局域网环境

2. **无加密传输**
   - 数据包使用明文 UDP（Phase 2 将支持 QUIC）

3. **单客户端实例**
   - 每台机器只能运行一个客户端

4. **需要管理员权限**
   - 创建 TUN 设备需要管理员权限

### 平台限制

- ✅ Windows 10/11 (64-bit)
- ❌ Windows 7/8/8.1
- ❌ 32-bit 系统

## 系统要求

### 最低要求

- Windows 10 (64-bit) 或更高版本
- 50 MB 可用磁盘空间
- 管理员权限
- wintun.dll

### 推荐配置

- Windows 11 (64-bit)
- 100 MB 可用磁盘空间
- 千兆网卡（局域网测试）

## 性能指标

### 预期性能（局域网）

- 延迟: < 5ms
- 吞吐量: > 100 Mbps
- CPU 占用: < 5% (空闲)
- 内存占用: < 50 MB

### 预期性能（广域网）

- 延迟: < 50ms (取决于物理距离)
- 吞吐量: > 10 Mbps
- 丢包率: < 1%

## 常见问题

### Q1: "需要管理员权限"

**解决方法**:
- 右键 PowerShell 图标
- 选择"以管理员身份运行"

### Q2: "无法加载 wintun.dll"

**解决方法**:
```powershell
# 确保 wintun.dll 在程序目录
dir wintun.dll
dir fuckipv6.exe

# 如果缺少，重新下载
.\setup_windows.ps1 -SkipWinTun:$false
```

### Q3: Ping 不通其他节点

**解决方法**:
```powershell
# 1. 检查虚拟 IP
ipconfig | findstr fd00

# 2. 检查路由
netsh interface ipv6 show route | findstr fd00

# 3. 允许 ICMPv6
netsh advfirewall firewall add rule name="ICMPv6" protocol=icmpv6:any,any dir=in action=allow
```

更多问题请参考 `README_WINDOWS.md`

## 路线图

### Phase 2（即将推出）

- ✨ NAT 穿透（UDP 打洞）
- 🔒 QUIC 加密传输
- 🔄 中继服务（Relay）
- 🌐 Web 管理界面
- 📊 连接质量监控

### Phase 3（计划中）

- 🚀 性能优化
- 📱 移动端支持
- 🔧 高级配置选项
- 📈 详细统计信息

## 测试状态

### ✅ Linux 环境

- 编译测试: ✅ 通过
- 功能测试: ✅ 通过
- P2P 连接: ✅ 通过

### ⏳ Windows 环境

- 编译测试: ✅ 通过（交叉编译）
- 功能测试: ⏳ 待测试
- P2P 连接: ⏳ 待测试

**请使用 `WINDOWS_TEST_CHECKLIST.md` 进行完整测试并反馈问题**

## 贡献

欢迎提交问题和改进建议！

- **项目主页**: https://github.com/yourusername/fuckipv6
- **提交问题**: https://github.com/yourusername/fuckipv6/issues
- **贡献代码**: https://github.com/yourusername/fuckipv6/pulls

## 许可证

与主项目相同的许可证。

## 鸣谢

- [WinTun](https://www.wintun.net/) - Windows TUN 驱动
- [wintun crate](https://crates.io/crates/wintun) - Rust 绑定
- [MinGW-w64](https://www.mingw-w64.org/) - 交叉编译工具链

---

**编译信息**:
- 编译器: rustc 1.85+ (nightly)
- 目标: x86_64-pc-windows-gnu
- 优化: --release
- 编译环境: Arch Linux
- 交叉编译器: MinGW-w64 GCC 15.2.0

**技术栈**:
- Rust async/await (tokio)
- WinTun 驱动
- WebSocket (tokio-tungstenite)
- Bincode 序列化
- Ed25519 加密

---

**祝使用愉快！**
