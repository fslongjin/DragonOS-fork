#!/bin/bash
# 连接到DragonOS串口终端

ARCH=${ARCH:-x86_64}
SOCKET_PATH="bin/tmp/hypervisor/serial-${ARCH}.sock"
PROJECT_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd)

if [ ! -S "$PROJECT_ROOT/$SOCKET_PATH" ]; then
    echo "错误: 串口socket不存在: $PROJECT_ROOT/$SOCKET_PATH"
    echo "请确保DragonOS虚拟机正在运行 (make run-nographic)"
    exit 1
fi

echo "连接到DragonOS串口终端..."
echo "Socket路径: $PROJECT_ROOT/$SOCKET_PATH"
echo "提示: 使用 Ctrl+] 退出 (如果使用socat)"
echo ""

# 尝试使用socat
if command -v socat &> /dev/null; then
    socat - "UNIX-CONNECT:$PROJECT_ROOT/$SOCKET_PATH"
elif command -v nc &> /dev/null && nc -h 2>&1 | grep -q "unix"; then
    nc -U "$PROJECT_ROOT/$SOCKET_PATH"
else
    echo "错误: 需要安装 socat 或支持unix socket的nc"
    echo "安装socat: sudo apt-get install socat"
    exit 1
fi


