#!/bin/bash
#
# fuckipv6 协调服务器安装脚本
# 用法: sudo ./install-coordinator.sh
#

set -e

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}================================${NC}"
echo -e "${GREEN}fuckipv6 协调服务器安装脚本${NC}"
echo -e "${GREEN}================================${NC}"
echo

# 检查root权限
if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}错误: 请使用root权限运行此脚本${NC}"
    echo "使用: sudo $0"
    exit 1
fi

# 检查是否已编译
if [ ! -f "../target/release/fuckipv6-coordinator" ]; then
    echo -e "${RED}错误: 找不到编译后的二进制文件${NC}"
    echo "请先运行: cargo build --release"
    exit 1
fi

# 创建系统用户
echo -e "\n${YELLOW}[1/6]${NC} 创建系统用户..."
if ! id -u fuckipv6 > /dev/null 2>&1; then
    useradd -r -s /bin/false -d /opt/fuckipv6 fuckipv6
    echo -e "${GREEN}✓${NC} 用户 fuckipv6 已创建"
else
    echo -e "${YELLOW}⚠${NC} 用户 fuckipv6 已存在"
fi

# 创建安装目录
echo -e "${YELLOW}[2/6]${NC} 创建安装目录..."
mkdir -p /opt/fuckipv6
mkdir -p /etc/fuckipv6
mkdir -p /var/log/fuckipv6

# 复制二进制文件
echo -e "${YELLOW}[3/6]${NC} 复制二进制文件..."
cp ../target/release/fuckipv6-coordinator /opt/fuckipv6/
chmod +x /opt/fuckipv6/fuckipv6-coordinator

# 设置权限
echo -e "${YELLOW}[4/6]${NC} 设置权限..."
chown -R fuckipv6:fuckipv6 /opt/fuckipv6
chown -R fuckipv6:fuckipv6 /var/log/fuckipv6

# 安装systemd服务
echo -e "${YELLOW}[5/6]${NC} 安装systemd服务..."
if [ -f "./fuckipv6-coordinator.service" ]; then
    cp ./fuckipv6-coordinator.service /etc/systemd/system/
    systemctl daemon-reload
    echo -e "${GREEN}✓${NC} Systemd服务已安装"
else
    echo -e "${YELLOW}⚠${NC} 未找到服务文件，跳过"
fi

# 配置防火墙（如果使用firewalld）
echo -e "${YELLOW}[6/6]${NC} 配置防火墙..."
if command -v firewall-cmd &> /dev/null; then
    firewall-cmd --permanent --add-port=6666/tcp || true
    firewall-cmd --reload || true
    echo -e "${GREEN}✓${NC} 防火墙已配置（开放TCP 6666端口）"
else
    echo -e "${YELLOW}⚠${NC} 未检测到firewalld，请手动配置防火墙"
    echo "   需要开放端口: TCP 6666"
fi

# 创建符号链接
ln -sf /opt/fuckipv6/fuckipv6-coordinator /usr/local/bin/fuckipv6-coordinator

echo
echo -e "${GREEN}================================${NC}"
echo -e "${GREEN}安装完成！${NC}"
echo -e "${GREEN}================================${NC}"
echo
echo "服务器信息:"
echo "  监听地址: [::]:6666"
echo "  IP地址池: fd00::1000/120"
echo
echo "下一步操作:"
echo "1. 启动服务:"
echo "   sudo systemctl start fuckipv6-coordinator"
echo
echo "2. 查看状态:"
echo "   sudo systemctl status fuckipv6-coordinator"
echo
echo "3. 查看日志:"
echo "   sudo journalctl -u fuckipv6-coordinator -f"
echo
echo "4. 开机自启:"
echo "   sudo systemctl enable fuckipv6-coordinator"
echo
echo "5. 测试连接:"
echo "   ss -tlnp | grep 6666"
echo
