#!/bin/bash
set -e

HOST="${FUCKIPV6_DEPLOY_HOST:-relay.xlei.cc}"
USER="${FUCKIPV6_DEPLOY_USER:-root}"
if [ -z "${SSHPASS:-}" ]; then
  echo "SSHPASS must be provided through the environment." >&2
  exit 1
fi
SSH_CMD="sshpass -e ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 $USER@$HOST"
SCP_CMD="sshpass -e scp -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10"

echo "[1/3] 检查远程服务器 SSH 连通性..."
$SSH_CMD "echo '>>> SSH 连接成功: \$(uname -a)'"

echo "[2/3] 上传最新构建的 musl 节点程序..."
$SCP_CMD target/x86_64-unknown-linux-musl/release/fuckipv6 root@$HOST:/usr/local/bin/fuckipv6.new

echo "[3/3] 替换节点程序并重启系统服务..."
$SSH_CMD "bash -s" << 'REMOTESCRIPT'
set -e
# 停止旧进程
systemctl stop fuckipv6.service || true
pkill -9 fuckipv6 || true
ip link delete fuckipv6 2>/dev/null || true

# 替换可执行文件
chmod +x /usr/local/bin/fuckipv6.new
mv -f /usr/local/bin/fuckipv6.new /usr/local/bin/fuckipv6

# 重启唯一节点服务
systemctl daemon-reload
systemctl restart fuckipv6.service
sleep 1
systemctl is-active --quiet fuckipv6.service && echo "节点服务已启动"
REMOTESCRIPT

sleep 2
curl -fsS "http://$HOST:8080/api/status"
echo ""
echo "部署完成，请访问: http://$HOST:8080"
