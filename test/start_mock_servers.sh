#!/bin/bash

# Mock服务器启动脚本
# 启动两个mock服务器实例，配置不同的响应概率

echo "=== 启动Mock服务器 ==="

# 获取脚本所在目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MOCK_SERVER_PATH="$SCRIPT_DIR/mock_server.py"

# 检查mock_server.py是否存在
if [ ! -f "$MOCK_SERVER_PATH" ]; then
    echo "错误: 找不到 $MOCK_SERVER_PATH"
    exit 1
fi

# 存储进程ID
PIDS=()

# 清理函数
cleanup() {
    echo ""
    echo "正在关闭所有mock服务器..."
    for pid in "${PIDS[@]}"; do
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid"
        fi
    done
    wait
    echo "所有服务器已关闭"
    exit 0
}

# 设置信号处理
trap cleanup SIGINT SIGTERM

echo "启动服务器1 - 端口8000 (100%正常响应)"
MOCK_SERVER_NORMAL=1 MOCK_SERVER_ERROR=0 MOCK_SERVER_TIMEOUT=0 python3.11 "$MOCK_SERVER_PATH" --port 8000 &
PIDS+=($!)

echo "启动服务器2 - 端口8001 (50%正常响应, 50%错误响应)"
MOCK_SERVER_NORMAL=0.5 MOCK_SERVER_ERROR=0.5 MOCK_SERVER_TIMEOUT=0 python3.11 "$MOCK_SERVER_PATH" --port 8001 &
PIDS+=($!)

echo ""
echo "所有服务器已启动!"
echo "服务器信息:"
echo "- 端口8000: http://localhost:8000 (100%正常响应)"
echo "- 端口8001: http://localhost:8001 (50%正常, 50%错误)"
echo ""
echo "按 Ctrl+C 停止所有服务器"

# 等待所有后台进程
wait
