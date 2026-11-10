import os
from fastapi import FastAPI, HTTPException, Request
from urllib.parse import urlparse
import time
import random
import argparse
import logging
from collections import deque

# 配置日志
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

# 解析命令行参数来设置概率
parser = argparse.ArgumentParser(description="FastAPI响应模拟服务")
parser.add_argument("--normal", type=float, default=0.5, 
                   help="正常响应的概率 (0-1之间)")
parser.add_argument("--error", type=float, default=0.3, 
                   help="错误响应的概率 (0-1之间)")
parser.add_argument("--timeout", type=float, default=0.2, 
                   help="超时响应的概率 (0-1之间)")
parser.add_argument("--port", type=int, default=8000,
                   help="服务器监听端口 (默认: 8000)")
parser.add_argument("--qps-limit", type=int, default=0,
                   help="QPS限制，0表示无限制 (默认: 0)")
args = parser.parse_args()

# 获取环境变量，如果存在则优先使用
normal_env = os.getenv("MOCK_SERVER_NORMAL")
error_env = os.getenv("MOCK_SERVER_ERROR")
timeout_env = os.getenv("MOCK_SERVER_TIMEOUT")
qps_limit_env = os.getenv("MOCK_SERVER_QPS_LIMIT")

# 如果环境变量存在，则使用环境变量，否则使用命令行参数
if normal_env is not None:
    normal_prob_val = float(normal_env)
else:
    normal_prob_val = args.normal

if error_env is not None:
    error_prob_val = float(error_env)
else:
    error_prob_val = args.error

if timeout_env is not None:
    timeout_prob_val = float(timeout_env)
else:
    timeout_prob_val = args.timeout

# 确保概率总和为1，如果不为1则进行归一化处理
total = normal_prob_val + error_prob_val + timeout_prob_val
normal_prob = normal_prob_val / total
error_prob = error_prob_val / total
timeout_prob = timeout_prob_val / total

# QPS限制相关
qps_limit = args.qps_limit
if qps_limit_env is not None:
    qps_limit = int(qps_limit_env)
request_timestamps = deque()

app = FastAPI(title="响应情况模拟服务")

@app.middleware("http")
async def log_requests(request: Request, call_next):
    """中间件：将请求信息合并成一行打印"""
    # 基础请求信息
    method = request.method
    url = str(request.url)
    path = urlparse(url).path  # 仅提取路径部分
    
    # 检查并发限制
    if qps_limit > 0:
        # 如果当前并发数超过了限制，则返回429错误
        if len(request_timestamps) >= qps_limit:
            raise HTTPException(
                status_code=429,
                detail={
                    "status": "error",
                    "message": "并发请求数超过限制",
                    "error_code": "TOO_MANY_REQUESTS",
                    "timestamp": time.time(),
                    "concurrency_limit": qps_limit
                }
            )
        
        # 记录当前请求（增加并发计数）
        request_timestamps.append(time.time())
    
    # 提取关键请求头
    headers = dict(request.headers)
    content_type = headers.get("content-type", "unknown")
    user_agent = headers.get("user-agent", "unknown")
    
    # 处理请求体
    body = "no_body"
    if request.method in ["POST", "PUT", "PATCH"]:
        try:
            body_bytes = await request.body()
            body = body_bytes.decode('utf-8', errors='replace')  # 处理编码问题
            # 重新设置请求体供后续使用
            request._body = body_bytes
        except Exception as e:
            body = f"body_read_error: {str(e)}"
    
    # 合并成一行日志
    log_line = (
        f"REQUEST: method={method}, path={path}, "
        f"content_type={content_type}, user_agent={user_agent}, "
        f"body={body}"
    )
    print(log_line)
    
    try:
        # 继续处理请求
        response = await call_next(request)
        return response
    finally:
        # 请求完成后，移除记录（减少并发计数）
        if qps_limit > 0 and request_timestamps:
            request_timestamps.popleft()

import json
import asyncio
from fastapi.responses import StreamingResponse

@app.get("/normal", summary="正常返回接口")
def normal_response():
    """正常返回示例"""
    response_data = {
        "id": "chatcmpl-mock-123",
        "object": "chat.completion",
        "created": int(time.time()),
        "model": "mock-model",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "这是一个正常响应的示例。"
                },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 5,
            "completion_tokens": 7,
            "total_tokens": 12
        }
    }
    return response_data

async def stream_chat_generator():
    """一个生成器，模拟流式聊天响应"""
    full_content = "This is a simulated streaming response from the mock server, sending data chunk by chunk to demonstrate live updates."
    words = full_content.split()
    
    for i, word in enumerate(words):
        chunk = {
            "id": "chatcmpl-mock-123",
            "object": "chat.completion.chunk",
            "created": int(time.time()),
            "model": "mock-model",
            "choices": [
                {
                    "index": 0,
                    "delta": {"content": f" {word}" if i > 0 else word},
                    "finish_reason": None
                }
            ]
        }
        yield f"data: {json.dumps(chunk)}\n\n"
        await asyncio.sleep(0.01)  # 模拟网络延迟

    # 发送带有 finish_reason 的最终块
    final_chunk = {
        "id": "chatcmpl-mock-123",
        "object": "chat.completion.chunk",
        "created": int(time.time()),
        "model": "mock-model",
        "choices": [
            {
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }
        ]
    }
    yield f"data: {json.dumps(final_chunk)}\n\n"
    yield "data: [DONE]\n\n"

@app.get("/error", summary="报错返回接口")
def error_response():
    """错误返回示例"""
    raise HTTPException(
        status_code=400,
        detail={
            "status": "error",
            "message": "这是一个错误响应的示例。",
            "error_code": "BAD_REQUEST",
            "timestamp": time.time()
        }
    )

@app.get("/timeout", summary="超时模拟接口")
def timeout_simulation():
    """超时响应示例"""
    time.sleep(2)  # 人为制造延迟
    return {"message": "这是一条超时响应示例"}

async def slow_stream_generator():
    """模拟慢速流式响应，用于测试超时"""
    # 先发送一些正常数据
    initial_words = ["This", "is", "a", "slow", "streaming"]
    
    for i, word in enumerate(initial_words):
        chunk = {
            "id": "chatcmpl-slow-123",
            "object": "chat.completion.chunk", 
            "created": int(time.time()),
            "model": "mock-model",
            "choices": [
                {
                    "index": 0,
                    "delta": {"content": f" {word}" if i > 0 else word},
                    "finish_reason": None
                }
            ]
        }
        yield f"data: {json.dumps(chunk)}\n\n"
        await asyncio.sleep(0.5)  # 每个词间隔0.5秒
    
    # 然后长时间停顿，模拟服务器卡住的情况
    print("🐌 Mock server: Starting long pause to trigger timeout...")
    await asyncio.sleep(30)  # 30秒停顿，应该触发客户端超时
    
    # 这部分代码在正常超时测试中不应该被执行到
    final_chunk = {
        "id": "chatcmpl-slow-123", 
        "object": "chat.completion.chunk",
        "created": int(time.time()),
        "model": "mock-model",
        "choices": [
            {
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }
        ]
    }
    yield f"data: {json.dumps(final_chunk)}\n\n"
    yield "data: [DONE]\n\n"

@app.post("/api/v1/completions", summary="随机响应接口")
async def random_response(request: Request):
    """
    根据请求体中的 'stream' 字段，随机触发正常、错误、超时或流式响应。
    概率可配置。
    """
    try:
        body = await request.json()
        is_streaming = body.get("stream", False)
    except json.JSONDecodeError:
        is_streaming = False

    # 生成0-1之间的随机数
    rand = random.random()
    
    # 根据概率决定返回类型
    if rand < normal_prob:
        # 成功响应
        if is_streaming:
            return StreamingResponse(stream_chat_generator(), media_type="text/event-stream")
        else:
            return normal_response()
    elif rand < normal_prob + error_prob:
        # 错误响应
        return error_response()
    else:
        # 超时响应
        if is_streaming:
            return StreamingResponse(slow_stream_generator(), media_type="text/event-stream")
        else:
            return timeout_simulation()
    
@app.post("/api/v1/chat/completions", summary="随机响应接口")
async def chat_completions_response(request: Request):
    """
    根据请求体中的 'stream' 字段，随机触发正常、错误、超时或流式响应。
    概率可配置。
    """
    try:
        body = await request.json()
        is_streaming = body.get("stream", False)
    except json.JSONDecodeError:
        is_streaming = False

    # 生成0-1之间的随机数
    rand = random.random()
    
    # 根据概率决定返回类型
    if rand < normal_prob:
        # 成功响应
        if is_streaming:
            return StreamingResponse(stream_chat_generator(), media_type="text/event-stream")
        else:
            return normal_response()
    elif rand < normal_prob + error_prob:
        # 错误响应
        return error_response()
    else:
        # 超时响应
        if is_streaming:
            return StreamingResponse(slow_stream_generator(), media_type="text/event-stream")
        else:
            return timeout_simulation()

@app.post("/api/v1/chat/completions/slow", summary="专门的慢速流式响应接口")
async def slow_chat_completions():
    """专门用于测试超时的慢速流式响应"""
    return StreamingResponse(slow_stream_generator(), media_type="text/event-stream")

@app.get("/health", summary="健康检查接口")
def health_check():
    """健康检查示例"""
    return {"status": "ok", "message": "服务正常运行中"}

if __name__ == "__main__":
    import uvicorn
    env_info = " (来自环境变量)" if normal_env is not None or error_env is not None or timeout_env is not None else " (来自命令行参数)"
    qps_env_info = " (来自环境变量)" if qps_limit_env is not None else " (来自命令行参数)"
    print(f"服务启动配置 - 端口: {args.port}, 正常概率: {normal_prob:.2f}, 错误概率: {error_prob:.2f}, 超时概率: {timeout_prob:.2f}{env_info}")
    if qps_limit > 0:
        print(f"QPS限制已启用: {qps_limit} 请求/秒{qps_env_info}")
    else:
        print("QPS限制: 无限制")
    uvicorn.run("mock_server:app", host="0.0.0.0", port=args.port, workers=10)
