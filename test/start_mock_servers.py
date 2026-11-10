#!/usr/bin/env python3
"""
启动多个mock服务器实例的脚本
- 端口8000: 100%正常响应
- 端口8001: 50%正常响应, 50%错误响应
"""

import subprocess
import sys
import os
import time
import signal
from pathlib import Path

# 获取当前脚本所在目录
script_dir = Path(__file__).parent
mock_server_path = script_dir / "mock_server.py"

# 存储子进程
processes = []

def signal_handler(signum, frame):
    """处理中断信号，优雅关闭所有子进程"""
    print("\n正在关闭所有mock服务器...")
    for process in processes:
        if process.poll() is None:  # 进程仍在运行
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
    print("所有服务器已关闭")
    sys.exit(0)

def start_server(port, env_vars, description):
    """启动单个mock服务器实例"""
    print(f"启动服务器 - {description} (端口: {port})")
    
    # 设置环境变量
    env = os.environ.copy()
    env.update(env_vars)
    
    # 修改mock_server.py以支持端口参数
    cmd = [
        sys.executable, 
        str(mock_server_path),
        "--port", str(port)
    ]
    
    try:
        process = subprocess.Popen(
            cmd,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            universal_newlines=True,
            bufsize=1
        )
        processes.append(process)
        return process
    except Exception as e:
        print(f"启动服务器失败 (端口 {port}): {e}")
        return None

def main():
    """主函数"""
    # 注册信号处理器
    signal.signal(signal.SIGINT, signal_handler)
    signal.signal(signal.SIGTERM, signal_handler)
    
    print("=== Mock服务器启动脚本 ===")
    
    # 检查mock_server.py是否存在
    if not mock_server_path.exists():
        print(f"错误: 找不到 {mock_server_path}")
        sys.exit(1)
    
    # 配置服务器实例
    servers = [
        {
            "port": 8000,
            "env": {
                "MOCK_SERVER_NORMAL": "1",
                "MOCK_SERVER_ERROR": "0", 
                "MOCK_SERVER_TIMEOUT": "0"
            },
            "description": "100%正常响应"
        },
        {
            "port": 8001,
            "env": {
                "MOCK_SERVER_NORMAL": "0.5",
                "MOCK_SERVER_ERROR": "0.5",
                "MOCK_SERVER_TIMEOUT": "0"
            },
            "description": "50%正常响应, 50%错误响应"
        }
    ]
    
    # 启动所有服务器
    for server_config in servers:
        process = start_server(
            server_config["port"],
            server_config["env"],
            server_config["description"]
        )
        if process is None:
            print("启动失败，正在清理...")
            signal_handler(None, None)
    
    print(f"\n所有服务器已启动! 共 {len(processes)} 个实例")
    print("服务器信息:")
    print("- 端口8000: http://localhost:8000 (100%正常响应)")
    print("- 端口8001: http://localhost:8001 (50%正常, 50%错误)")
    print("\n按 Ctrl+C 停止所有服务器")
    
    # 监控所有进程
    try:
        while True:
            # 检查是否有进程意外退出
            for i, process in enumerate(processes):
                if process.poll() is not None:
                    print(f"服务器 {i+1} 意外退出 (返回码: {process.returncode})")
                    # 读取输出
                    if process.stdout:
                        output = process.stdout.read()
                        if output:
                            print(f"输出: {output}")
            
            # 如果所有进程都退出了，则退出主程序
            if all(p.poll() is not None for p in processes):
                print("所有服务器都已退出")
                break
                
            time.sleep(1)
            
    except KeyboardInterrupt:
        signal_handler(signal.SIGINT, None)

if __name__ == "__main__":
    main()
