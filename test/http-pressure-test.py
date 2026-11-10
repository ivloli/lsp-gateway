import aiohttp
import asyncio
import argparse
import time
import json
from typing import List, Dict, Tuple
import itertools

from torch import conv1d

def generate_real_body(model: str, content: str):
    return {
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": content
            }
        ],
        "stream": False
    }

def generate_fake_body(model: str = "mock/lsp") -> Dict:
    return {
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": "hello world"
            }
        ],
        "stream": False
    }

async def send_post(session: aiohttp.ClientSession, url: str, body: Dict, verbose: bool = False, conv_id=None) -> Tuple[bool, float]:
    """Send a single POST request and return (success_status, request_duration).
    
    Success Rule: Only HTTP 200 status code is considered successful.
    Any exceptions (network errors, timeouts, etc.) are considered failures.
    """
    start_time = time.time()
    
    # Prepare headers
    headers = {}
    if conv_id:
        headers['X-Conversation-Id'] = conv_id
    
    try:
        async with session.post(url, json=body, headers=headers) as response:
            # Read response content
            response_text = await response.text()
            request_duration = time.time() - start_time
            
            # Print response content if verbose mode is enabled
            if verbose:
                print(f"Response Status: {response.status}")
                print(f"Response Content: {response_text}")
            
            # Core logic: Success = HTTP 200 status code
            return (response.status == 200, request_duration)
                
    except Exception as e:
        # Catch all exceptions (connection errors, timeouts, invalid responses)
        request_duration = time.time() - start_time
        if verbose:
            print(f"Request failed with exception: {str(e)}")
        return (False, request_duration)

def load_inputs_from_file_generator(file_path: str):
    """Generator that yields inputs from a file where each line is a JSON object with a 'rawinput' and 'convid' field."""
    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            for line_num, line in enumerate(f, 1):
                line = line.strip()
                if not line:
                    continue  # Skip empty lines
                
                try:
                    json_obj = json.loads(line)
                    if 'rawInput' in json_obj:
                        if 'conversationId' in json_obj and json_obj['conversationId']:
                            content = json_obj['rawInput']
                            conv_id = json_obj['conversationId']
                            yield content, conv_id
                        else:
                            print(f"Warning: Line {line_num} missing 'conversationId', skipping: {line}")
                    else:
                        print(f"Warning: Line {line_num} does not contain 'rawInput' skipping: {line}")
                except json.JSONDecodeError as e:
                    print(f"Warning: Invalid JSON on line {line_num}: {e}")
    except FileNotFoundError:
        print(f"Error: File '{file_path}' not found.")
        raise
    except Exception as e:
        print(f"Error reading file '{file_path}': {e}")
        raise

async def single_worker(url: str, total_requests: int, concurrency: int, model: str = "mock/lsp", 
                       verbose: bool = False, inputs_file=None) -> List[Tuple[bool, float]]:
    """Single worker coroutine to handle all concurrent requests.
    
    Uses a semaphore to control maximum concurrent requests, ensuring we don't
    overwhelm the target server or exhaust local resources.
    """
    results: List[Tuple[bool, float]] = []
    # Semaphore to limit concurrent requests (matches user-specified concurrency)
    semaphore = asyncio.Semaphore(concurrency)
    
    # Create a cycle iterator for inputs if provided
    input_cycle = None
    if inputs_file:
        try:
            inputs = load_inputs_from_file_generator(inputs_file)
            input_cycle = itertools.cycle(inputs)
        except Exception:
            print(f"Warning: Could not load inputs from '{inputs_file}', using default content")
            input_cycle = None
    
    async with aiohttp.ClientSession() as session:
        # Wrapped task with semaphore control
        async def bounded_request_task(request_idx: int):
            async with semaphore:
                if input_cycle:
                    # Get next input from the cycle
                    try:
                        content, conv_id = next(input_cycle)
                        body = generate_real_body(model, content)
                    except StopIteration:
                        # This shouldn't happen since we're using cycle, but just in case
                        body = generate_fake_body(model)
                else:
                    # Use default fake body if no inputs file provided
                    body = generate_fake_body(model)
                    conv_id = f"default-{request_idx}"
                
                return await send_post(session, url, body, verbose, conv_id)
        
        # Create all request tasks (total = user-specified total_requests)
        request_tasks = [bounded_request_task(i) for i in range(total_requests)]
        # Execute all tasks concurrently and collect results
        results = await asyncio.gather(*request_tasks)
    
    return results


async def run_benchmark(url: str, total_requests: int, concurrency: int, model: str = "mock/lsp", 
                       verbose: bool = False, inputs_file=None) -> Tuple[int, int, float, float, float, float]:
    """Run the full benchmark and return key metrics.
    
    Returns: (successful_requests, total_requests, total_time, avg_success_duration, 
             min_success_duration, max_success_duration)
    """
    # Record benchmark start time
    benchmark_start = time.time()
    
    # Execute all requests via single worker
    raw_results = await single_worker(url, total_requests, concurrency, model, verbose, inputs_file)
    
    # Calculate total benchmark duration
    total_benchmark_time = time.time() - benchmark_start
    
    # Extract and calculate metrics
    successful_count = sum(1 for is_success, _ in raw_results if is_success)
    # Only include durations from successful requests (more meaningful for analysis)
    success_durations = [
        duration for is_success, duration in raw_results 
        if is_success and duration > 0
    ]
    
    # Calculate duration statistics (handle edge case: no successful requests)
    avg_duration = sum(success_durations) / len(success_durations) if success_durations else 0.0
    min_duration = min(success_durations) if success_durations else 0.0
    max_duration = max(success_durations) if success_durations else 0.0
    
    return (successful_count, total_requests, total_benchmark_time, avg_duration, min_duration, max_duration)


def main():
    # Parse command-line arguments for benchmark configuration
    parser = argparse.ArgumentParser(description="Async POST Request Benchmark Tool (aiohttp)")
    parser.add_argument(
        '--url', 
        type=str, 
        default='http://localhost:8000/data', 
        help='Target API endpoint URL (default: http://localhost:8080/data)'
    )
    parser.add_argument(
        '--total', 
        type=int, 
        default=100, 
        help='Total number of requests to send (default: 1000)'
    )
    parser.add_argument(
        '--concurrency', 
        type=int, 
        default=5, 
        help='Maximum number of concurrent requests (default: 50)'
    )
    parser.add_argument(
        '--model',
        type=str,
        default='mock/lsp',
        help='Model parameter in request body (default: mock/lsp)'
    )
    parser.add_argument(
        '--verbose',
        action='store_true',
        help='Print response content for each request'
    )
    parser.add_argument(
        '--inputs-file',
        type=str,
        help='Path to file containing JSON objects (one per line) with "rawinput" and "convid" fields to use as content and conversation ID'
    )
    args = parser.parse_args()
    
    # Print benchmark initialization info
    print("=" * 60)
    print("Async POST Benchmark Started")
    print("=" * 60)
    print(f"Target URL:        {args.url}")
    print(f"Total Requests:    {args.total}")
    print(f"Max Concurrency:   {args.concurrency}")
    print(f"Model Parameter:   {args.model}")
    print(f"Verbose Mode:      {'Enabled' if args.verbose else 'Disabled'}")
    print(f"Inputs File:       {args.inputs_file or 'Not specified (using default content)'}")
    print(f"Success Rule:      Only HTTP 200 status code = Successful")
    print("-" * 60)
    
    try:
        # Get async event loop and run benchmark
        event_loop = asyncio.get_event_loop()
        (success_count, total_count, total_time, avg_dur, min_dur, max_dur) = event_loop.run_until_complete(
            run_benchmark(args.url, args.total, args.concurrency, args.model, args.verbose, args.inputs_file)
        )
        
        # Calculate derived metrics
        success_rate = (success_count / total_count) * 100 if total_count > 0 else 0.0
        qps = total_count / total_time if total_time > 0 else 0.0  # QPS = Requests per Second
        
        # Print formatted benchmark results
        print("\n[Benchmark Results]")
        print("-" * 60)
        print(f"Total Requests:    {total_count}")
        print(f"Successful:        {success_count}")
        print(f"Failed:            {total_count - success_count}")
        print(f"Success Rate:      {success_rate:.2f}%")
        print(f"Total Time:        {total_time:.2f} seconds")
        print(f"QPS (Avg):         {qps:.2f} requests/second")
        print("\n[Successful Request Duration (Seconds)]")
        print(f"Average:           {avg_dur:.4f}")
        print(f"Minimum:           {min_dur:.4f}")
        print(f"Maximum:           {max_dur:.4f}")
        print("=" * 60)
        
    except KeyboardInterrupt:
        print("\n[Benchmark Interrupted] User pressed Ctrl+C")
    except Exception as e:
        print(f"\n[Benchmark Error] Unexpected issue occurred: {str(e)}")


if __name__ == "__main__":
    main()