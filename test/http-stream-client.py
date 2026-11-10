import requests

import requests
import json

def openai_stream_chat(url: str, data: dict, api_key: str=""):
    """
    处理类似OpenAI的流式聊天完成API
    """
    headers = {
        "Content-Type": "application/json",
        "Authorization": f"Bearer {api_key}"
    }
    
    data = data
    
    try:
        response = requests.post(
            url, 
            headers=headers, 
            json=data, 
            stream=True,
            timeout=60
        )
        
        response.raise_for_status()
        
        print("开始接收流式响应:")
        for line in response.iter_lines():
            if line:
                line = line.decode('utf-8')
                if line.startswith('data: '):
                    data_str = line[6:]  # 去掉 'data: ' 前缀
                    
                    if data_str == '[DONE]':
                        print("\n流式响应结束")
                        break
                    
                    try:
                        data_obj = json.loads(data_str)
                        # 提取内容
                        choices = data_obj.get('choices', [])
                        if choices:
                            delta = choices[0].get('delta', {})
                            content = delta.get('content', '')
                            if content:
                                print(content, end='', flush=True)
                    except json.JSONDecodeError as e:
                        print(f"JSON解析错误: {e}, 原始数据: {data_str}")
                        
    except Exception as e:
        print(f"错误: {e}")

# 使用示例
url = "http://0.0.0.0:8000/api/v1/chat/completions"
data = {
        "model": "mock/lsp",
        "messages": [
            {
                "role": "user",
                "content": "hello world"
            }
        ],
        "stream": True
    }

openai_stream_chat(url, data)