#!/bin/bash
set -e

HOST="relay.xlei.cc"
USER="root"
PASS="lds112.."
SSH_CMD="sshpass -p $PASS ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 $USER@$HOST"
SCP_CMD="sshpass -p $PASS scp -o StrictHostKeyChecking=no -o ConnectTimeout=10"

echo "[1/4] 检查远程服务器 SSH 连通性..."
$SSH_CMD "echo '>>> SSH 连接成功: \$(uname -a)'"

echo "[2/4] 上传最新构建的 musl 二进制文件..."
$SCP_CMD target/x86_64-unknown-linux-musl/release/fuckipv6-coordinator root@$HOST:/usr/local/bin/fuckipv6-coordinator.new
$SCP_CMD target/x86_64-unknown-linux-musl/release/fuckipv6 root@$HOST:/usr/local/bin/fuckipv6.new

echo "[3/4] 替换二进制并重启系统服务与客户端..."
$SSH_CMD "bash -s" << 'REMOTESCRIPT'
set -e
# 停止旧进程
systemctl stop fuckipv6-coordinator.service || true
pkill -9 fuckipv6 || true
ip link delete fuckipv6 2>/dev/null || true

# 替换可执行文件
chmod +x /usr/local/bin/fuckipv6-coordinator.new /usr/local/bin/fuckipv6.new
mv -f /usr/local/bin/fuckipv6-coordinator.new /usr/local/bin/fuckipv6-coordinator
mv -f /usr/local/bin/fuckipv6.new /usr/local/bin/fuckipv6

# 重启协调服务
systemctl daemon-reload
systemctl restart fuckipv6-coordinator.service
sleep 1
systemctl is-active --quiet fuckipv6-coordinator.service && echo "协调器服务已启动"

# 启动客户端 client1
if [ -f /root/client1.toml ]; then
  nohup /usr/local/bin/fuckipv6 start --config /root/client1.toml > /root/client1.log 2>&1 &
  echo "客户端 client1 已在后台启动"
fi
REMOTESCRIPT

echo "[4/4] 验证服务状态..."
sleep 2
curl -s http://$HOST:8080/api/status
echo ""
echo "部署完成！请访问: http://$HOST:8080"
