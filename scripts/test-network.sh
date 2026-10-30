#!/bin/bash
#
# fuckipv6 网络测试脚本
# 用法: ./test-network.sh
#
# 此脚本会启动一个托管节点和两个成员节点，测试虚拟网络连接
#

set -e

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="$PROJECT_ROOT/target/release"
TEST_DIR="$PROJECT_ROOT/test_workspace"

echo -e "${GREEN}================================${NC}"
echo -e "${GREEN}fuckipv6 网络测试脚本${NC}"
echo -e "${GREEN}================================${NC}"
echo

# 清理函数
cleanup() {
    echo -e "\n${YELLOW}清理测试环境...${NC}"

    # 停止后台进程
    if [ -n "${HOST_PID:-}" ]; then
        kill "$HOST_PID" 2>/dev/null || true
        echo "✓ 已停止托管节点 (PID: $HOST_PID)"
    fi

    if [ -n "${NODE1_PID:-}" ]; then
        kill "$NODE1_PID" 2>/dev/null || true
        echo "✓ 已停止成员节点1 (PID: $NODE1_PID)"
    fi

    if [ -n "${NODE2_PID:-}" ]; then
        kill "$NODE2_PID" 2>/dev/null || true
        echo "✓ 已停止成员节点2 (PID: $NODE2_PID)"
    fi

    # 删除TUN设备
    sudo ip link delete fuckipv6-test1 2>/dev/null || true
    sudo ip link delete fuckipv6-test2 2>/dev/null || true

    echo -e "${GREEN}清理完成${NC}"
}

# 注册信号处理
trap cleanup EXIT INT TERM

# 检查root权限
if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}错误: 此脚本需要root权限来创建TUN设备${NC}"
    echo "使用: sudo $0"
    exit 1
fi

# 检查是否已编译
echo -e "${BLUE}[1/6]${NC} 检查编译文件..."
if [ ! -x "$BUILD_DIR/fuckipv6" ]; then
    echo -e "${YELLOW}⚠ 未找到节点程序，开始编译...${NC}"
    cd "$PROJECT_ROOT"
    cargo build --release
    echo -e "${GREEN}✓${NC} 编译完成"
else
    echo -e "${GREEN}✓${NC} 编译文件已存在"
fi

# 创建测试目录
echo -e "${BLUE}[2/6]${NC} 创建测试环境..."
rm -rf "$TEST_DIR"
mkdir -p "$TEST_DIR/host"
mkdir -p "$TEST_DIR/node1"
mkdir -p "$TEST_DIR/node2"
echo -e "${GREEN}✓${NC} 测试目录已创建: $TEST_DIR"

# 生成配置文件
echo -e "${BLUE}[3/6]${NC} 生成配置文件..."

# 成员节点1配置
cat > "$TEST_DIR/node1/config.toml" <<EOF
coordinator_url = "ws://localhost:6666"
private_key_path = "node1_key.pem"

[network]
virtual_network = "fd00::/64"

[tun]
name = "fuckipv6-test1"
mtu = 1400

[p2p]
listen_port = 7001
heartbeat_interval = 30

[log]
level = "info"
EOF

# 成员节点2配置
cat > "$TEST_DIR/node2/config.toml" <<EOF
coordinator_url = "ws://localhost:6666"
private_key_path = "node2_key.pem"

[network]
virtual_network = "fd00::/64"

[tun]
name = "fuckipv6-test2"
mtu = 1400

[p2p]
listen_port = 7002
heartbeat_interval = 30

[log]
level = "info"
EOF

echo -e "${GREEN}✓${NC} 配置文件已生成"

# 启动托管节点
echo -e "${BLUE}[4/6]${NC} 启动托管节点..."
cd "$TEST_DIR/host"
RUST_LOG=info "$BUILD_DIR/fuckipv6" host \
    --port 6666 \
    --relay-port 6667 \
    > host.log 2>&1 &
HOST_PID=$!

sleep 2

if ps -p "$HOST_PID" > /dev/null; then
    echo -e "${GREEN}✓${NC} 托管节点已启动 (PID: $HOST_PID)"
    echo "  监听地址: 0.0.0.0:6666"
else
    echo -e "${RED}✗${NC} 托管节点启动失败"
    cat host.log
    exit 1
fi

# 启动成员节点1
echo -e "${BLUE}[5/6]${NC} 启动成员节点..."

echo "启动成员节点1..."
cd "$TEST_DIR/node1"
RUST_LOG=info "$BUILD_DIR/fuckipv6" start --config config.toml > node1.log 2>&1 &
NODE1_PID=$!
sleep 3

if ps -p "$NODE1_PID" > /dev/null; then
    echo -e "${GREEN}✓${NC} 成员节点1已启动 (PID: $NODE1_PID)"
else
    echo -e "${RED}✗${NC} 成员节点1启动失败"
    cat node1.log
    exit 1
fi

# 启动成员节点2
echo "启动成员节点2..."
cd "$TEST_DIR/node2"
RUST_LOG=info "$BUILD_DIR/fuckipv6" start --config config.toml > node2.log 2>&1 &
NODE2_PID=$!
sleep 3

if ps -p "$NODE2_PID" > /dev/null; then
    echo -e "${GREEN}✓${NC} 成员节点2已启动 (PID: $NODE2_PID)"
else
    echo -e "${RED}✗${NC} 成员节点2启动失败"
    cat node2.log
    exit 1
fi

# 等待网络建立
echo
echo -e "${YELLOW}等待网络建立...${NC}"
sleep 5

# 测试连接
echo -e "${BLUE}[6/6]${NC} 测试网络连接..."

# 获取分配的IP地址
echo
echo "查找分配的虚拟IP地址..."

CLIENT1_IP=$(ip -6 addr show fuckipv6-test1 | grep -oP 'fd00::[0-9a-f:]+' | head -1)
CLIENT2_IP=$(ip -6 addr show fuckipv6-test2 | grep -oP 'fd00::[0-9a-f:]+' | head -1)

if [ -z "$CLIENT1_IP" ]; then
    echo -e "${RED}✗${NC} 无法获取客户端1的IP地址"
    echo "成员节点1日志:"
    tail -20 "$TEST_DIR/node1/node1.log"
    exit 1
fi

if [ -z "$CLIENT2_IP" ]; then
    echo -e "${RED}✗${NC} 无法获取客户端2的IP地址"
    echo "成员节点2日志:"
    tail -20 "$TEST_DIR/node2/node2.log"
    exit 1
fi

echo -e "${GREEN}✓${NC} 客户端1 IP: $CLIENT1_IP"
echo -e "${GREEN}✓${NC} 客户端2 IP: $CLIENT2_IP"

# Ping测试
echo
echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${YELLOW}开始Ping测试${NC}"
echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo

echo "测试: 客户端1 -> 客户端2 ($CLIENT1_IP -> $CLIENT2_IP)"
if ping6 -c 4 -W 2 $CLIENT2_IP > /dev/null 2>&1; then
    echo -e "${GREEN}✓${NC} Ping成功!"
    ping6 -c 4 $CLIENT2_IP | grep -E 'bytes from|packet loss|rtt'
else
    echo -e "${RED}✗${NC} Ping失败"
    echo
    echo "托管节点日志 (最后20行):"
    tail -20 "$TEST_DIR/host/host.log"
    echo
    echo "成员节点1日志 (最后20行):"
    tail -20 "$TEST_DIR/node1/node1.log"
    echo
    echo "成员节点2日志 (最后20行):"
    tail -20 "$TEST_DIR/node2/node2.log"
    exit 1
fi

echo
echo "测试: 客户端2 -> 客户端1 ($CLIENT2_IP -> $CLIENT1_IP)"
if ping6 -c 4 -W 2 $CLIENT1_IP > /dev/null 2>&1; then
    echo -e "${GREEN}✓${NC} Ping成功!"
    ping6 -c 4 $CLIENT1_IP | grep -E 'bytes from|packet loss|rtt'
else
    echo -e "${RED}✗${NC} Ping失败"
    exit 1
fi

# 测试结果总结
echo
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}✓ 所有测试通过！${NC}"
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo
echo "测试摘要:"
echo "  托管节点:   运行中 (PID: $HOST_PID)"
echo "  成员节点1:  运行中 (PID: $NODE1_PID) - $CLIENT1_IP"
echo "  成员节点2:  运行中 (PID: $NODE2_PID) - $CLIENT2_IP"
echo "  连接状态:   ✓ P2P连接成功"
echo
echo "日志文件:"
echo "  托管节点:   $TEST_DIR/host/host.log"
echo "  成员节点1:  $TEST_DIR/node1/node1.log"
echo "  成员节点2:  $TEST_DIR/node2/node2.log"
echo
echo "按Ctrl+C退出测试环境..."

# 保持运行，等待用户终止
while true; do
    sleep 1
done
