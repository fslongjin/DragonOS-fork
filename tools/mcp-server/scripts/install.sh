#!/bin/bash
# MCP服务器安装脚本

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MCP_SERVER_DIR="$(dirname "$SCRIPT_DIR")"

echo "安装DragonOS MCP服务器..."
echo ""

# 检查Python版本
if ! command -v python3 &> /dev/null; then
    echo "错误: 未找到python3，请先安装Python 3.8+"
    exit 1
fi

PYTHON_VERSION=$(python3 --version | cut -d' ' -f2 | cut -d'.' -f1,2)
REQUIRED_VERSION="3.8"

if [ "$(printf '%s\n' "$REQUIRED_VERSION" "$PYTHON_VERSION" | sort -V | head -n1)" != "$REQUIRED_VERSION" ]; then
    echo "错误: 需要Python 3.8+，当前版本: $PYTHON_VERSION"
    exit 1
fi

echo "Python版本: $(python3 --version)"

# 创建虚拟环境（可选）
if [ "$1" == "--venv" ]; then
    echo "创建虚拟环境..."
    python3 -m venv venv
    source venv/bin/activate
    echo "虚拟环境已激活"
fi

# 安装依赖
echo "安装依赖..."
cd "$MCP_SERVER_DIR"
pip install -r requirements.txt

echo ""
echo "安装完成！"
echo ""
echo "使用方法:"
echo "  1. 配置Cursor MCP服务器（参考README.md）"
echo "  2. 运行 'make connect-serial' 连接到DragonOS终端"
echo "  3. AI助手现在可以使用MCP工具与DragonOS交互"


