#!/bin/bash
#
# fuckipv6 客户端安装脚本
# 用法: sudo ./install-client.sh
#

set -e

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}================================${NC}"
echo -e "${GREEN}fuckipv6 客户端安装脚本${NC}"
echo -e "${GREEN}================================${NC}"
echo

# 检查root权限
if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}错误: 请使用root权限运行此脚本${NC}"
    echo "使用: sudo $0"
    exit 1
fi

# 检查系统
if [ ! -f /etc/os-release ]; then
    echo -e "${RED}错误: 无法识别的操作系统${NC}"
    exit 1
fi

source /etc/os-release
echo -e "检测到操作系统: ${GREEN}$PRETTY_NAME${NC}"

# 检查是否已编译
if [ ! -f "../target/release/fuckipv6" ]; then
    echo -e "${RED}错误: 找不到编译后的二进制文件${NC}"
    echo "请先运行: cargo build --release"
    exit 1
fi

# 创建安装目录
echo -e "\n${YELLOW}[1/5]${NC} 创建安装目录..."
mkdir -p /opt/fuckipv6
mkdir -p /etc/fuckipv6
mkdir -p /var/log/fuckipv6

# 复制二进制文件
echo -e "${YELLOW}[2/5]${NC} 复制二进制文件..."
cp ../target/release/fuckipv6 /opt/fuckipv6/
chmod +x /opt/fuckipv6/fuckipv6

# 生成配置文件
echo -e "${YELLOW}[3/5]${NC} 生成配置文件..."
if [ ! -f /etc/fuckipv6/client.toml ]; then
    /opt/fuckipv6/fuckipv6 init-config --output /etc/fuckipv6/client.toml
    echo -e "${GREEN}✓${NC} 配置文件已生成: /etc/fuckipv6/client.toml"
    echo -e "${YELLOW}⚠ 请编辑配置文件，修改 coordinator_url${NC}"
else
    echo -e "${YELLOW}⚠${NC} 配置文件已存在，跳过生成"
fi

# 安装systemd服务
echo -e "${YELLOW}[4/5]${NC} 安装systemd服务..."
if [ -f "./fuckipv6-client.service" ]; then
    cp ./fuckipv6-client.service /etc/systemd/system/
    systemctl daemon-reload
    echo -e "${GREEN}✓${NC} Systemd服务已安装"
else
    echo -e "${YELLOW}⚠${NC} 未找到服务文件，跳过"
fi

# 创建符号链接
echo -e "${YELLOW}[5/5]${NC} 创建符号链接..."
ln -sf /opt/fuckipv6/fuckipv6 /usr/local/bin/fuckipv6
echo -e "${GREEN}✓${NC} 符号链接已创建: /usr/local/bin/fuckipv6"

echo
echo -e "${GREEN}================================${NC}"
echo -e "${GREEN}安装完成！${NC}"
echo -e "${GREEN}================================${NC}"
echo
echo "下一步操作:"
echo "1. 编辑配置文件:"
echo "   sudo nano /etc/fuckipv6/client.toml"
echo
echo "2. 启动服务:"
echo "   sudo systemctl start fuckipv6-client"
echo
echo "3. 查看状态:"
echo "   sudo systemctl status fuckipv6-client"
echo
echo "4. 查看日志:"
echo "   sudo journalctl -u fuckipv6-client -f"
echo
echo "5. 开机自启:"
echo "   sudo systemctl enable fuckipv6-client"
echo
