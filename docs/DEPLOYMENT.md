# fuckipv6 部署指南

本文档提供详细的生产环境部署指南。

## 目录

- [系统要求](#系统要求)
- [服务器部署](#服务器部署)
- [客户端部署](#客户端部署)
- [安全加固](#安全加固)
- [监控和维护](#监控和维护)
- [故障排除](#故障排除)

---

## 系统要求

### 协调服务器

**硬件要求**:
- CPU: 1核心 (推荐2核心)
- 内存: 512MB (推荐1GB)
- 网络: 公网IPv6地址
- 磁盘: 1GB

**软件要求**:
- 操作系统: Linux (Ubuntu 20.04+, Debian 11+, CentOS 8+)
- 开放端口: TCP 6666 (WebSocket信令)
- IPv6: 必须启用

### 客户端

**硬件要求**:
- CPU: 1核心
- 内存: 128MB
- 网络: IPv6网络连接
- 磁盘: 100MB

**软件要求**:
- 操作系统: Linux、macOS 或 Windows
- 权限: Linux 需要 root 或 `CAP_NET_ADMIN`；macOS 需要 sudo/root；Windows 需要管理员权限和 WinTun
- IPv6: 必须启用

---

## 服务器部署

### 1. 准备服务器

```bash
# 更新系统
sudo apt update && sudo apt upgrade -y

# 安装必要工具
sudo apt install -y git curl build-essential pkg-config

# 验证IPv6可用
ip -6 addr show
ping6 google.com
```

### 2. 安装Rust工具链

```bash
# 安装rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 重新加载环境
source $HOME/.cargo/env

# 验证安装
rustc --version
cargo --version
```

### 3. 编译项目

```bash
# 克隆仓库
git clone https://github.com/yourusername/fuckipv6.git
cd fuckipv6

# 编译（Release模式）
./scripts/build.sh release

# 或手动编译
cargo build --release
```

### 4. 安装协调服务器

```bash
# 运行安装脚本
sudo ./scripts/install-coordinator.sh
```

该脚本会自动执行以下操作:
1. 创建系统用户 `fuckipv6`
2. 复制二进制文件到 `/opt/fuckipv6/`
3. 安装systemd服务
4. 配置防火墙（如果使用firewalld）

### 5. 配置防火墙

**使用firewalld**:
```bash
sudo firewall-cmd --permanent --add-port=6666/tcp
sudo firewall-cmd --reload
```

**使用iptables**:
```bash
sudo ip6tables -A INPUT -p tcp --dport 6666 -j ACCEPT
sudo ip6tables-save > /etc/iptables/rules.v6
```

**使用ufw**:
```bash
sudo ufw allow 6666/tcp
sudo ufw reload
```

### 6. 启动服务

```bash
# 启动服务
sudo systemctl start fuckipv6-coordinator

# 查看状态
sudo systemctl status fuckipv6-coordinator

# 查看日志
sudo journalctl -u fuckipv6-coordinator -f

# 设置开机自启
sudo systemctl enable fuckipv6-coordinator
```

### 7. 验证服务运行

```bash
# 检查端口监听
ss -tlnp | grep 6666

# 应该看到类似输出:
# LISTEN 0  128  [::]:6666  [::]:*  users:(("fuckipv6-coordinator",pid=1234,fd=5))

# 测试WebSocket连接（使用wscat）
npm install -g wscat
wscat -c "ws://[::1]:6666"
```

---

## 客户端部署

### 方法1: Linux 使用安装脚本（推荐）

```bash
# 1. 编译项目
./scripts/build.sh release

# 2. 运行安装脚本
sudo ./scripts/install-client.sh

# 3. 编辑配置文件
sudo nano /etc/fuckipv6/client.toml

# 修改 coordinator_url 为你的服务器地址:
# coordinator_url = "ws://[2001:db8::1]:6666"

# 4. 启动服务
sudo systemctl start fuckipv6-client

# 5. 查看状态
sudo systemctl status fuckipv6-client
```

### 方法2: 手动安装（Linux/macOS 通用）

```bash
# 1. 创建目录
sudo mkdir -p /opt/fuckipv6
sudo mkdir -p /etc/fuckipv6
sudo mkdir -p /var/log/fuckipv6

# 2. 复制二进制文件
sudo cp target/release/fuckipv6 /opt/fuckipv6/
sudo chmod +x /opt/fuckipv6/fuckipv6

# 3. 生成配置文件
/opt/fuckipv6/fuckipv6 init-config --output /etc/fuckipv6/client.toml

# 4. 编辑配置
sudo nano /etc/fuckipv6/client.toml

# 5. 手动启动测试
sudo /opt/fuckipv6/fuckipv6 start --config /etc/fuckipv6/client.toml
```

macOS 可直接使用 release 包或本机编译出的二进制，手动生成配置后通过 `sudo fuckipv6 start --config client.toml` 启动；Windows 请使用 Windows release 包中的 `setup_windows.ps1` 或管理员终端手动运行。

macOS release 包还包含 launchd 安装脚本：

```bash
sudo ./install-macos-client.sh
# 编辑 /usr/local/etc/fuckipv6/client.toml 后启动/重启
sudo launchctl kickstart -k system/cc.xlei.fuckipv6.client
```

macOS 协调服务器可使用：

```bash
sudo ./install-macos-coordinator.sh
sudo launchctl kickstart -k system/cc.xlei.fuckipv6.coordinator
```

### 配置说明

编辑 `/etc/fuckipv6/client.toml`:

```toml
# 必须修改: 协调服务器地址
coordinator_url = "ws://your-server.example.com:6666"

# 可选: 指定客户端ID（通常无需设置）
# client_id = "my-laptop"

[network]
virtual_network = "fd00::/64"

[tun]
name = "fuckipv6"
mtu = 1400

[p2p]
listen_port = 0  # 0 = 随机端口
heartbeat_interval = 30

[log]
level = "info"
```

启动时会校验配置值，包括 WebSocket URL、IPv6 CIDR、监听地址、MTU、超时、丢包率阈值和日志级别；部署前可先用目标配置启动一次确认没有校验错误。

---

## Docker 部署

### 协调服务器

```bash
# 构建镜像
docker build -f Dockerfile.coordinator -t fuckipv6-coordinator:local .

# 启动协调服务器
docker run -d \
  --name fuckipv6-coordinator \
  --restart unless-stopped \
  -p 6666:6666 \
  -p 8080:8080 \
  -v "$PWD/examples/coordinator.toml.example:/etc/fuckipv6/coordinator.toml:ro" \
  fuckipv6-coordinator:local
```

### docker compose

```bash
# 启动协调服务器
docker compose up -d coordinator

# 如需容器化客户端，先编辑 ./client.toml，然后启动 client profile
docker compose --profile client up -d client
```

容器化客户端当前面向 Linux，需要 `network_mode: host`、`NET_ADMIN` capability 和 `/dev/net/tun` 设备映射。生产环境更推荐客户端直接用系统服务部署，便于管理 TUN/utun/WinTun 设备和主机路由。

---

## 发布流程

GitHub Actions 提供两个工作流：

- `.github/workflows/ci.yml`：每次 push/PR 调用 `verify.yml`，在 Linux、Windows、macOS 执行发布门禁并聚合报告。
- `.github/workflows/release.yml`：推送 `v*` 标签或手动触发时先复用 `verify.yml`；三平台门禁通过后才打包 Linux x86_64、macOS x86_64、macOS arm64 和 Windows x86_64 产物，生成 SHA256 并上传到 GitHub Release。

发布门禁也可以在本机执行。正式检查默认要求干净工作区；禁止用 `--allow-dirty` 的产物作为发布依据。

```bash
python3 scripts/release-gate.py run --platform linux --print-plan
python3 scripts/release-gate.py run --platform linux --toolchain stable
```

Linux 门禁默认额外检查 musl 和 Windows GNU target，需提前安装对应 Rust target 及 MinGW。报告位于 `target/release-gate/linux.json`，完整日志和 Criterion 原始估计值位于 `target/release-gate/linux-artifacts/`。门禁报告只覆盖工程构建、测试、lint 和基准可执行性，不代表独立安全审计通过，也不构成超越 WireGuard 的性能证明。

创建发布：

```bash
git tag v0.1.0
git push origin v0.1.0
```

发布产物包括：

- `fuckipv6-linux-x86_64-<version>.tar.gz`
- `fuckipv6-macos-x86_64-<version>.tar.gz`
- `fuckipv6-macos-arm64-<version>.tar.gz`
- `fuckipv6-windows-x86_64-<version>.zip`
- 对应的 `.sha256` 校验文件

---

## 安全加固

### 1. 使用TLS/SSL (推荐生产环境)

**获取Let's Encrypt证书**:

```bash
# 安装certbot
sudo apt install -y certbot

# 获取证书
sudo certbot certonly --standalone -d vpn.example.com

# 证书位置:
# /etc/letsencrypt/live/vpn.example.com/fullchain.pem
# /etc/letsencrypt/live/vpn.example.com/privkey.pem
```

**修改systemd服务**:

编辑 `/etc/systemd/system/fuckipv6-coordinator.service`:

```ini
ExecStart=/opt/fuckipv6/fuckipv6-coordinator \
    --bind "[::]:6666" \
    --ip-pool "fd00::1000/120" \
    --tls-cert /etc/letsencrypt/live/vpn.example.com/fullchain.pem \
    --tls-key /etc/letsencrypt/live/vpn.example.com/privkey.pem
```

**客户端使用wss://**:

```toml
coordinator_url = "wss://vpn.example.com:6666"
```

### 2. 防火墙规则

**仅允许特定IP访问**:

```bash
# 仅允许特定客户端IP段
sudo ip6tables -A INPUT -p tcp --dport 6666 -s 2001:db8::/32 -j ACCEPT
sudo ip6tables -A INPUT -p tcp --dport 6666 -j DROP
```

### 3. 系统用户权限

协调服务器使用专用用户运行:

```bash
# 验证用户已创建
id fuckipv6

# 验证文件权限
ls -la /opt/fuckipv6
# 应该显示: drwxr-xr-x fuckipv6 fuckipv6
```

### 4. 日志审计

客户端和协调端都会读取配置文件中的 `[log].level`，可选值为 `trace`、`debug`、`info`、`warn`、`error`。如果设置了 `RUST_LOG`，环境变量优先。
两端使用同一套 tracing 初始化，日志格式包含统一的时间、级别、target 和消息字段，便于 systemd/journal 或外部采集器统一解析。

通过 systemd 启用详细日志:

```bash
# 编辑服务文件
sudo systemctl edit fuckipv6-coordinator

# 添加:
[Service]
Environment="RUST_LOG=info,fuckipv6=debug"

# 重新加载
sudo systemctl daemon-reload
sudo systemctl restart fuckipv6-coordinator
```

---

## 监控和维护

### 1. 日志管理

**查看实时日志**:
```bash
# 协调服务器
sudo journalctl -u fuckipv6-coordinator -f

# 客户端
sudo journalctl -u fuckipv6-client -f
```

**查看历史日志**:
```bash
# 最近100行
sudo journalctl -u fuckipv6-coordinator -n 100

# 今天的日志
sudo journalctl -u fuckipv6-coordinator --since today

# 特定时间段
sudo journalctl -u fuckipv6-coordinator --since "2024-01-01" --until "2024-01-02"
```

**日志轮转**:
```bash
# journald自动管理日志轮转
# 配置位置: /etc/systemd/journald.conf

# 限制日志大小
sudo systemctl edit systemd-journald

[Journal]
SystemMaxUse=1G
```

### 2. 性能监控

**检查连接状态**:
```bash
# 服务器端
ss -tlnp | grep fuckipv6

# 客户端
ip -6 addr show fuckipv6
ping6 fd00::1001  # ping其他客户端
```

**系统资源使用**:
```bash
# CPU和内存
top -p $(pidof fuckipv6-coordinator)

# 网络流量
iftop -i fuckipv6  # 需要安装iftop
```

### 3. 自动更新

**创建更新脚本** (`/opt/fuckipv6/update.sh`):

```bash
#!/bin/bash
set -e

cd /path/to/fuckipv6
git pull
cargo build --release

sudo systemctl stop fuckipv6-coordinator
sudo cp target/release/fuckipv6-coordinator /opt/fuckipv6/
sudo systemctl start fuckipv6-coordinator
```

**设置定期检查**:
```bash
# 添加到crontab
crontab -e

# 每天凌晨2点检查更新
0 2 * * * /opt/fuckipv6/update.sh >> /var/log/fuckipv6/update.log 2>&1
```

### 4. 备份

**备份关键文件**:
```bash
#!/bin/bash
# backup.sh

BACKUP_DIR="/var/backups/fuckipv6"
DATE=$(date +%Y%m%d)

mkdir -p "$BACKUP_DIR"

# 备份配置
tar czf "$BACKUP_DIR/config-$DATE.tar.gz" /etc/fuckipv6/

# 备份密钥
tar czf "$BACKUP_DIR/keys-$DATE.tar.gz" /var/lib/fuckipv6/*.pem

# 保留最近30天的备份
find "$BACKUP_DIR" -name "*.tar.gz" -mtime +30 -delete
```

---

## 故障排除

### 问题1: 客户端无法连接到服务器

**症状**: 客户端日志显示 "Failed to connect to coordinator"

**解决方案**:
```bash
# 1. 检查服务器是否运行
sudo systemctl status fuckipv6-coordinator

# 2. 检查端口是否监听
ss -tlnp | grep 6666

# 3. 检查防火墙
sudo firewall-cmd --list-all
sudo ip6tables -L -n

# 4. 测试网络连接
ping6 your-server.example.com
telnet your-server.example.com 6666

# 5. 检查服务器日志
sudo journalctl -u fuckipv6-coordinator -n 100
```

### 问题2: TUN设备创建失败

**症状**: "创建 TUN 设备失败"、"权限不足"、"设备已存在" 或 "无法执行 ip 命令"

**解决方案**:
```bash
# 1. 检查TUN模块是否加载
lsmod | grep tun

# 如果没有，手动加载
sudo modprobe tun

# 2. 检查设备权限
ls -l /dev/net/tun

# 应该显示: crw-rw-rw- 1 root root

# 3. 确保以root运行
whoami  # 应该显示 root

# 4. 检查现有TUN设备
ip link show type tun

# 如果 fuckipv6 设备已存在，清理残留设备
sudo ip link delete fuckipv6

# 5. 检查 iproute2 是否安装
ip -V
```

### 问题3: 节点间无法通信

**症状**: ping6 其他节点超时

**解决方案**:
```bash
# 1. 检查虚拟IP是否分配
ip -6 addr show fuckipv6

# 2. 检查路由表
ip -6 route show

# 3. 检查P2P连接状态
# 查看客户端日志中的 "add_peer" 信息
sudo journalctl -u fuckipv6-client | grep "add_peer"

# 4. 测试P2P端口
# 在客户端1:
sudo lsof -i UDP

# 5. 抓包分析
sudo tcpdump -i fuckipv6 -n icmp6
```

### 问题4: 性能差/延迟高

**症状**: ping延迟 >100ms

**解决方案**:
```bash
# 1. 检查MTU设置
ip link show fuckipv6
# 应该显示 mtu 1400

# 2. 调整MTU
sudo ip link set fuckipv6 mtu 1280

# 3. 检查系统网络性能
ping6 -c 10 -s 1200 目标IP
# 观察丢包率

# 4. 检查CPU使用
top -p $(pidof fuckipv6)

# 5. 启用debug日志分析
sudo systemctl edit fuckipv6-client
# Environment="RUST_LOG=debug"
```

### 问题5: 服务意外停止

**症状**: systemctl显示服务inactive

**解决方案**:
```bash
# 1. 查看崩溃原因
sudo journalctl -u fuckipv6-coordinator --since "1 hour ago"

# 2. 检查系统资源
free -h
df -h

# 3. 检查OOM killer日志
dmesg | grep -i "killed process"

# 4. 增加内存限制（systemd）
sudo systemctl edit fuckipv6-coordinator
[Service]
MemoryMax=1G

# 5. 启用核心转储
sudo systemctl edit fuckipv6-coordinator
[Service]
LimitCORE=infinity
```

---

## 常见部署场景

### 场景1: 家庭组网

**网络拓扑**:
```
Internet
   |
[家用路由器] (有公网IPv6)
   |
[协调服务器] (树莓派/NAS)
   |
   +--- [客户端1] (笔记本)
   +--- [客户端2] (台式机)
   +--- [客户端3] (手机)
```

**部署步骤**:
1. 在路由器或NAS上部署协调服务器
2. 使用内网地址: `ws://[fd00::1]:6666`
3. 所有客户端连接到该地址

### 场景2: 云服务器中转

**网络拓扑**:
```
[云服务器] (公网IPv6)
   |
   +--- [家庭网络客户端]
   +--- [公司网络客户端]
   +--- [移动设备客户端]
```

**部署步骤**:
1. 在云服务器部署协调服务器
2. 配置TLS证书
3. 使用公网地址: `wss://vpn.example.com:6666`
4. 配置域名DNS AAAA记录

### 场景3: 企业内网

**网络拓扑**:
```
[办公室服务器] (内网)
   |
   +--- [开发机1]
   +--- [开发机2]
   +--- [测试服务器]
```

**部署步骤**:
1. 在内网服务器部署协调服务器
2. 使用内网IPv6地址
3. 配置防火墙仅允许内网访问
4. 集成到企业认证系统（Phase 2功能）

---

## 性能调优

### 1. 内核参数优化

编辑 `/etc/sysctl.conf`:

```ini
# 增加UDP缓冲区
net.core.rmem_max = 26214400
net.core.wmem_max = 26214400

# 启用IPv6转发（如果需要）
net.ipv6.conf.all.forwarding = 1

# 增加文件描述符限制
fs.file-max = 65536
```

应用配置:
```bash
sudo sysctl -p
```

### 2. systemd服务限制

编辑服务文件:
```ini
[Service]
LimitNOFILE=65536
LimitNPROC=4096
```

### 3. 编译优化

使用更激进的优化:

编辑 `Cargo.toml`:
```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
```

---

## 总结

本部署指南涵盖了从基础安装到生产环境优化的完整流程。主要要点:

✅ **服务器端**: 使用systemd管理服务，配置防火墙和TLS
✅ **客户端**: 确保root权限，正确配置TUN设备
✅ **安全**: 使用TLS加密，限制访问IP，定期更新
✅ **监控**: 配置日志记录，监控资源使用
✅ **维护**: 定期备份配置和密钥，建立更新流程

如有问题，请参考故障排除章节或提交Issue到GitHub仓库。
