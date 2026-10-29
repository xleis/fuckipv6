================================================================================
  fuckipv6 - Windows 版本 (Phase 1 MVP)
================================================================================

这是 fuckipv6 的 Windows 预编译版本。

================================================================================
快速开始
================================================================================

1. 下载 WinTun 驱动
   ----------------
   由于许可证限制，wintun.dll 不包含在此发布包中。

   请按以下步骤手动下载：

   a) 访问: https://www.wintun.net/
   b) 下载 wintun-0.14.1.zip（或最新版本）
   c) 解压 ZIP 文件
   d) 复制以下文件到本目录：
      wintun\bin\amd64\wintun.dll -> 这里

   或者，运行 setup_windows.ps1 脚本会自动下载。

2. 生成配置文件
   -----------
   以管理员身份运行 PowerShell，然后执行：

   > .\fuckipv6.exe init-config

   这会在 %USERPROFILE%\.config\fuckipv6\ 创建配置文件。

3. 编辑配置
   -------
   打开 %USERPROFILE%\.config\fuckipv6\default.toml
   修改 coordinator_url 为你的服务器地址：

   [network]
   coordinator_url = "ws://YOUR_SERVER_IP:6666"

4. 启动服务器（在一台机器上）
   ----------------------
   > .\fuckipv6-coordinator.exe

5. 启动客户端（需要管理员权限）
   --------------------------
   以管理员身份运行 PowerShell：

   > .\fuckipv6.exe start

6. 测试连接
   -------
   在另一台机器上重复步骤 2-5，然后：

   > ping -6 fd00::1002

================================================================================
文件说明
================================================================================

fuckipv6.exe                - 客户端程序
fuckipv6-coordinator.exe    - 协调服务器
setup_windows.ps1           - 自动下载 wintun.dll 的脚本
setup_windows.bat           - 脚本启动器
README_WINDOWS.md           - 完整的 Windows 使用指南
QUICKSTART_WINDOWS.md       - 5分钟快速开始指南
WINDOWS_TEST_CHECKLIST.md   - 完整的测试清单

缺少的文件（需要你下载）：
wintun.dll                  - WinTun 驱动（从 https://www.wintun.net/ 下载）

================================================================================
系统要求
================================================================================

- Windows 10/11 (64-bit)
- 管理员权限
- wintun.dll (从 wintun.net 下载)

================================================================================
常见问题
================================================================================

Q: "需要管理员权限才能创建 TUN 设备"
A: 右键 PowerShell -> "以管理员身份运行"

Q: "无法加载 wintun.dll"
A: 确保 wintun.dll 与 fuckipv6.exe 在同一目录

Q: Ping 不通其他节点
A: 检查防火墙设置，允许 ICMPv6：
   > netsh advfirewall firewall add rule name="ICMPv6" protocol=icmpv6:any,any dir=in action=allow

详细的故障排除请参考 README_WINDOWS.md

================================================================================
获取帮助
================================================================================

查看完整文档:       README_WINDOWS.md
快速开始指南:       QUICKSTART_WINDOWS.md
完整测试清单:       WINDOWS_TEST_CHECKLIST.md

项目主页:   https://github.com/yourusername/fuckipv6
提交问题:   https://github.com/yourusername/fuckipv6/issues

================================================================================
版本信息
================================================================================

版本:       Phase 1 MVP
编译日期:   2026-01-01
编译环境:   Linux cross-compile (MinGW)

特性:
- WinTun TUN 设备支持
- IPv6 虚拟局域网
- P2P UDP 连接
- WebSocket 信令
- Ed25519 认证

即将推出 (Phase 2):
- NAT 穿透
- QUIC 加密
- 中继服务
- Web 管理界面

================================================================================
