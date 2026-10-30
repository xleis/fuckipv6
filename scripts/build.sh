#!/bin/bash
#
# fuckipv6 编译脚本
# 用法: ./build.sh [release|debug]
#

set -e

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo -e "${GREEN}================================${NC}"
echo -e "${GREEN}fuckipv6 编译脚本${NC}"
echo -e "${GREEN}================================${NC}"
echo

# 解析参数
BUILD_MODE="${1:-release}"

if [ "$BUILD_MODE" != "release" ] && [ "$BUILD_MODE" != "debug" ]; then
    echo -e "${RED}错误: 无效的编译模式: $BUILD_MODE${NC}"
    echo "用法: $0 [release|debug]"
    exit 1
fi

cd "$PROJECT_ROOT"

# 检查Rust工具链
echo -e "${BLUE}[1/4]${NC} 检查Rust工具链..."
if ! command -v cargo &> /dev/null; then
    echo -e "${RED}✗${NC} 未安装Cargo"
    echo "请安装Rust工具链: https://rustup.rs/"
    exit 1
fi

RUST_VERSION=$(rustc --version)
echo -e "${GREEN}✓${NC} $RUST_VERSION"

# 检查依赖
echo -e "${BLUE}[2/4]${NC} 检查系统依赖..."

# Linux特定依赖
if [[ "$OSTYPE" == "linux-gnu"* ]]; then
    MISSING_DEPS=()

    # 检查pkg-config
    if ! command -v pkg-config &> /dev/null; then
        MISSING_DEPS+=("pkg-config")
    fi

    if [ ${#MISSING_DEPS[@]} -ne 0 ]; then
        echo -e "${YELLOW}⚠${NC} 缺少以下依赖: ${MISSING_DEPS[*]}"
        echo
        echo "安装命令:"
        echo "  Ubuntu/Debian: sudo apt-get install ${MISSING_DEPS[*]}"
        echo "  CentOS/RHEL:   sudo yum install ${MISSING_DEPS[*]}"
        echo "  Arch:          sudo pacman -S ${MISSING_DEPS[*]}"
        exit 1
    fi

    echo -e "${GREEN}✓${NC} 系统依赖已满足"
fi

# 清理旧的构建产物（可选）
if [ "$2" == "--clean" ]; then
    echo -e "${BLUE}[3/4]${NC} 清理旧的构建产物..."
    cargo clean
    echo -e "${GREEN}✓${NC} 清理完成"
else
    echo -e "${BLUE}[3/4]${NC} 跳过清理（使用 --clean 参数以清理）"
fi

# 编译
echo -e "${BLUE}[4/4]${NC} 开始编译..."
echo

if [ "$BUILD_MODE" == "release" ]; then
    echo "编译模式: Release (优化编译，速度较慢)"
    cargo build --release
    BUILD_DIR="target/release"
else
    echo "编译模式: Debug (快速编译，未优化)"
    cargo build
    BUILD_DIR="target/debug"
fi

echo
echo -e "${GREEN}================================${NC}"
echo -e "${GREEN}编译成功！${NC}"
echo -e "${GREEN}================================${NC}"
echo

# 显示编译结果
echo "编译产物:"
if [ -f "$BUILD_DIR/fuckipv6" ]; then
    NODE_SIZE=$(du -h "$BUILD_DIR/fuckipv6" | cut -f1)
    echo -e "  ${GREEN}✓${NC} 节点程序: $BUILD_DIR/fuckipv6 ($NODE_SIZE)"
else
    echo -e "  ${RED}✗${NC} 节点程序编译失败"
fi

echo
echo "下一步操作:"
echo "  测试: sudo ./scripts/test-network.sh"
echo "  安装节点: sudo ./scripts/install-node.sh"
echo
