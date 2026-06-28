import json
import time
import sys
import os
import csv
import argparse
import requests
from datetime import datetime, timezone
from pathlib import Path

PROXY_URL = "http://127.0.0.1:8000"
CHAT_URL = f"{PROXY_URL}/v1/chat/completions"
HEALTH_URL = f"{PROXY_URL}/health"

TEST_DIR = Path(__file__).parent / "test-results"
RUN_DIR = TEST_DIR / f"run-{datetime.now().strftime('%Y%m%d-%H%M%S')}"
LOG_DIR = RUN_DIR / "logs"

GENERATION_TIMEOUT = 300

MODELS = [
    {
        "id": "gemma-4-E4B",
        "port": 8003,
        "profile": None,
        "test_ctx": True,
    },
    {
        "id": "gemma4-26b-cerebellum",
        "port": 8004,
        "profile": None,
        "test_ctx": False,
    },
    {
        "id": "gemma4-26b-a4b-apex",
        "port": 8006,
        "profile": None,
        "test_ctx": False,
    },
    {
        "id": "Qwen3.6-APEX-uncensor",
        "port": 8005,
        "profile": "thinking-coding",
        "test_ctx": False,
    },
    {
        "id": "Qwen3.6-APEX-uncensor",
        "port": 8005,
        "profile": None,
        "test_ctx": False,
    },
]


def log(msg):
    ts = datetime.now().strftime("%H:%M:%S")
    print(f"[{ts}] {msg}", flush=True)


def log_result(name, ok, elapsed=None, info=None):
    status = "PASS" if ok else "FAIL"
    extra = f" [{elapsed:.1f}ms]" if elapsed else ""
    extra += f" {info}" if info else ""
    log(f"  {status}: {name}{extra}")


def model_key(model_id, profile):
    return f"{model_id}/{profile}" if profile else model_id


def requests_post_timed(url, json_data, timeout=GENERATION_TIMEOUT):
    start = time.perf_counter()
    resp = requests.post(url, json=json_data, timeout=timeout, stream=False)
    elapsed = (time.perf_counter() - start) * 1000
    return resp, elapsed


def health_check():
    try:
        resp = requests.get(HEALTH_URL, timeout=5)
        return resp.ok, resp.json() if resp.ok else None
    except Exception as e:
        return False, {"error": str(e)}


def port_open(port):
    try:
        resp = requests.get(f"{PROXY_URL}/health", timeout=5)
        if resp.ok:
            data = resp.json()
            for model_id, running in data.get("models", {}).items():
                if running:
                    cfg = next((m for m in MODELS if m["port"] == port), None)
                    if cfg and model_id == cfg["id"]:
                        return True
        return False
    except:
        return False


def unload_port(port):
    try:
        resp = requests.get(f"{PROXY_URL}/unload/{port}", timeout=10)
        elapsed = resp.elapsed.total_seconds() * 1000
        return resp.ok, elapsed, resp.json() if resp.ok else resp.text
    except Exception as e:
        return False, 0, str(e)


def wait_port_closed(port, timeout=30):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if not port_open(port):
            return True
        time.sleep(1)
    return False


def get_server_timing(resp):
    tps = None
    timing = resp.headers.get("Server-Timing", "")
    if not timing:
        timing = resp.headers.get("X-Server-Timing", "")
    for part in timing.split(","):
        part = part.strip()
        if "token" in part.lower() and "per" in part.lower():
            try:
                tps = float(part.split("=")[-1].replace("s", "").replace(" ", ""))
            except:
                pass
        if "tokens_per_second" in part.lower():
            try:
                tps = float(part.split("=")[-1].replace("s", "").replace(" ", ""))
            except:
                pass
    return tps


def extract_stats(resp_json):
    usage = resp_json.get("usage", {})
    prompt_tokens = usage.get("prompt_tokens") or usage.get("prompt_tokens_count")
    completion_tokens = usage.get("completion_tokens") or usage.get("completion_tokens_count") or usage.get("generated_tokens")
    total_tokens = usage.get("total_tokens")
    timing = resp_json.get("timings") or resp_json.get("generation_settings", {}).get("timings") or {}
    predicted_tps = timing.get("predicted_per_token_us") or timing.get("predict_per_token_us")
    prompt_tps = timing.get("prompt_per_token_us")
    predicted_ms = timing.get("predicted_ms") or timing.get("predict_ms")
    prompt_ms = timing.get("prompt_ms")
    return {
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "total_tokens": total_tokens,
        "predicted_tps": 1e6 / predicted_tps if predicted_tps else None,
        "prompt_tps": 1e6 / prompt_tps if prompt_tps else None,
        "predicted_ms": predicted_ms,
        "prompt_ms": prompt_ms,
    }


def call_model(model_id, profile, messages, max_tokens=50, stream=False, temperature=None):
    req_model = model_key(model_id, profile)
    payload = {
        "model": req_model,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": stream,
    }
    if temperature is not None:
        payload["temperature"] = temperature
    resp, elapsed = requests_post_timed(CHAT_URL, payload)
    return resp, elapsed


def wait_model_ready(model_id, profile, timeout=300):
    deadline = time.time() + timeout
    poll_msg = "Hello"
    while time.time() < deadline:
        try:
            resp, _ = call_model(model_id, profile, [{"role": "user", "content": poll_msg}], max_tokens=1)
            if resp.status_code == 200:
                return True
            if resp.status_code == 503:
                time.sleep(5)
                continue
            log(f"  Unexpected status {resp.status_code}, retrying...")
            time.sleep(5)
        except Exception as e:
            log(f"  Error waiting: {e}, retrying...")
            time.sleep(5)
    return False


def test_cold_start(model_id, profile, port):
    log("  [Cold Start] Sending first request (model will be loaded)...")
    start = time.perf_counter()
    first_token_time = None
    content = ""
    poll_start = start
    req_model = model_key(model_id, profile)
    poll_payload = {
        "model": req_model,
        "messages": [{"role": "user", "content": "Hello! Reply with just 'OK'."}],
        "max_tokens": 10,
    }

    deadline = time.time() + GENERATION_TIMEOUT
    while time.time() < deadline:
        try:
            resp = requests.post(CHAT_URL, json=poll_payload, timeout=30)
            elapsed = (time.perf_counter() - start) * 1000
            if resp.status_code == 200:
                data = resp.json()
                msg = data.get("choices", [{}])[0].get("message", {})
                content = msg.get("content", "")
                reasoning = msg.get("reasoning_content", "")
                if first_token_time is None:
                    first_token_time = elapsed
                ok = len(content or "") > 0 or len(reasoning or "") > 0
                return ok, {
                    "total_ms": round(elapsed, 2),
                    "first_token_ms": round(first_token_time, 2),
                    "content_length": len(content or ""),
                    "reasoning_length": len(reasoning or ""),
                }
            else:
                time.sleep(5)
        except Exception:
            time.sleep(5)

    total_ms = (time.perf_counter() - start) * 1000
    ok = len(content) > 0
    return ok, {
        "total_ms": round(total_ms, 2),
        "first_token_ms": round(first_token_time, 2) if first_token_time else None,
        "content_length": len(content),
    }


def test_smoke(model_id, profile):
    req_model = model_key(model_id, profile)
    payload = {
        "model": req_model,
        "messages": [{"role": "user", "content": "What is 2+2? Answer with just the number."}],
        "max_tokens": 10,
    }
    resp, elapsed = requests_post_timed(CHAT_URL, payload)
    if resp.status_code != 200:
        return False, {"status": resp.status_code, "elapsed_ms": round(elapsed, 2), "error": resp.text[:200]}

    try:
        data = resp.json()
        content = data.get("choices", [{}])[0].get("message", {}).get("content", "")
        reasoning = data.get("choices", [{}])[0].get("message", {}).get("reasoning_content", "")
        ok = len(content) > 0 or len(reasoning or "") > 0
        tps = get_server_timing(resp)
        stats = extract_stats(data)
        ct = stats.get("completion_tokens") if stats else None
        calc_tps = round(ct / (elapsed / 1000), 2) if ct and ct > 0 and elapsed > 0 else None
        return ok, {
            "elapsed_ms": round(elapsed, 2),
            "content": (content or reasoning or "")[:200],
            "tokens_per_sec": tps or calc_tps,
            "stats": stats,
        }
    except Exception as e:
        return False, {"error": str(e), "elapsed_ms": round(elapsed, 2)}


def test_speed(model_id, profile):
    req_model = model_key(model_id, profile)
    payload = {
        "model": req_model,
        "messages": [{"role": "user", "content": "Explain the concept of recursion in computer programming. Include a simple example with code."}],
        "max_tokens": 200,
    }
    resp, elapsed = requests_post_timed(CHAT_URL, payload)
    if resp.status_code != 200:
        return False, {"status": resp.status_code, "elapsed_ms": round(elapsed, 2), "error": resp.text[:200]}

    try:
        data = resp.json()
        content = data.get("choices", [{}])[0].get("message", {}).get("content", "")
        reasoning = data.get("choices", [{}])[0].get("message", {}).get("reasoning_content", "")
        ok = len(content) > 0 or len(reasoning or "") > 0
        tps = get_server_timing(resp)
        stats = extract_stats(data)

        result = {
            "elapsed_ms": round(elapsed, 2),
            "content_length": len(content),
            "reasoning_length": len(reasoning) if reasoning else 0,
            "tokens_per_sec": tps,
            "stats": stats,
        }

        if stats and stats.get("completion_tokens") and elapsed > 0:
            result["calc_tokens_per_sec"] = round(stats["completion_tokens"] / (elapsed / 1000), 2)

        return ok, result
    except Exception as e:
        return False, {"error": str(e), "elapsed_ms": round(elapsed, 2)}


def test_streaming(model_id, profile):
    req_model = model_key(model_id, profile)
    payload = {
        "model": req_model,
        "messages": [{"role": "user", "content": "Count 1 to 5, one per line."}],
        "max_tokens": 50,
        "stream": True,
    }

    start = time.perf_counter()
    first_token_ms = None
    content = ""
    reasoning = ""
    chunk_count = 0
    try:
        resp = requests.post(CHAT_URL, json=payload, timeout=GENERATION_TIMEOUT, stream=True)
        elapsed = (time.perf_counter() - start) * 1000
        for line in resp.iter_lines():
            if not line:
                continue
            decoded = line.decode("utf-8")
            if decoded.startswith("data: "):
                data_str = decoded[6:]
                if data_str == "[DONE]":
                    break
                try:
                    data = json.loads(data_str)
                    if first_token_ms is None:
                        first_token_ms = (time.perf_counter() - start) * 1000
                    chunk_count += 1
                    delta = data.get("choices", [{}])[0].get("delta", {})
                    c = delta.get("content", "")
                    r = delta.get("reasoning_content", "")
                    if c:
                        content += c
                    if r:
                        reasoning += r
                except:
                    pass
        resp.close()
        ok = (len(content) > 0 or len(reasoning) > 0) and chunk_count > 1
        return ok, {
            "elapsed_ms": round(elapsed, 2),
            "first_token_ms": round(first_token_ms, 2) if first_token_ms else None,
            "chunk_count": chunk_count,
            "content_length": len(content),
            "reasoning_length": len(reasoning),
        }
    except Exception as e:
        return False, {"error": str(e)}


def test_tool_call(model_id, profile):
    req_model = model_key(model_id, profile)
    payload = {
        "model": req_model,
        "messages": [{"role": "user", "content": "What's the weather in Paris? Use the get_weather tool."}],
        "max_tokens": 100,
        "tools": [{
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get weather for a location",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": {"type": "string", "description": "City name"},
                        "unit": {"type": "string", "enum": ["celsius", "fahrenheit"]},
                    },
                    "required": ["location"],
                },
            },
        }],
        "tool_choice": "auto",
    }

    resp, elapsed = requests_post_timed(CHAT_URL, payload)
    if resp.status_code != 200:
        return False, {"status": resp.status_code, "elapsed_ms": round(elapsed, 2), "error": resp.text[:200]}

    try:
        data = resp.json()
        msg = data.get("choices", [{}])[0].get("message", {})
        tool_calls = msg.get("tool_calls", [])
        content = msg.get("content", "")
        reasoning = msg.get("reasoning_content", "")
        has_tool = len(tool_calls) > 0
        ok = has_tool or len(content or "") > 0 or len(reasoning or "") > 0
        return ok, {
            "elapsed_ms": round(elapsed, 2),
            "has_tool_call": has_tool,
            "tool_calls": [
                {
                    "name": tc.get("function", {}).get("name"),
                    "args": tc.get("function", {}).get("arguments"),
                }
                for tc in tool_calls
            ],
            "content_length": len(content) if content else 0,
        }
    except Exception as e:
        return False, {"error": str(e), "elapsed_ms": round(elapsed, 2)}


def test_model_entry(model_cfg):
    model_id = model_cfg["id"]
    port = model_cfg["port"]
    profile = model_cfg.get("profile")
    mk = model_key(model_id, profile)
    model_alias = f"{model_id}" + (f"/{profile}" if profile else "")

    log(f"\n{'='*60}")
    log(f"Testing: {model_alias} (port {port})")
    log(f"{'='*60}")

    result = {
        "model_id": mk,
        "base_model_id": model_id,
        "profile": profile,
        "port": port,
        "started_utc": datetime.now(timezone.utc).isoformat(),
        "status": "in_progress",
        "health_before": None,
        "port_open_before": None,
        "pre_unload_ok": None,
        "pre_unload_elapsed_ms": None,
        "cold_start": None,
        "smoke": None,
        "speed": None,
        "stream": None,
        "tool_call": None,
        "unload_ok": None,
        "unload_elapsed_ms": None,
        "port_closed_after": None,
        "health_after": None,
        "error": None,
    }

    try:
        log("Phase 1: Pre-check")
        health_ok, health_data = health_check()
        result["health_before"] = health_ok
        if not health_ok:
            raise RuntimeError(f"Proxy not healthy: {health_data}")

        port_was_open = port_open(port)
        result["port_open_before"] = port_was_open

        if port_was_open:
            log("  Port already in use, unloading first...")
            unload_ok, unload_elapsed, msg = unload_port(port)
            result["pre_unload_ok"] = unload_ok
            result["pre_unload_elapsed_ms"] = round(unload_elapsed, 2)
            if unload_ok:
                wait_port_closed(port)
                log(f"  Pre-unloaded: {msg}")
            else:
                log(f"  Pre-unload skipped: {msg}")

        log("Phase 2: Cold start (load + first inference)")
        ok, data = test_cold_start(model_id, profile, port)
        result["cold_start"] = data
        data["ok"] = ok
        log_result("Cold start", ok, data.get("total_ms"), f"TTFT={data.get('first_token_ms'):.0f}ms" if data.get("first_token_ms") else None)

        if not ok:
            log("  Cold start timed out, aborting.")
            raise RuntimeError("Cold start failed - model did not respond")

        log("Phase 3: Smoke test")
        ok, data = test_smoke(model_id, profile)
        result["smoke"] = data
        tps = data.get("tokens_per_sec") or (data.get("stats", {}).get("predicted_tps"))
        log_result("Smoke", ok, data.get("elapsed_ms"), f"TPS={tps:.1f}" if tps else None)

        log("Phase 4: Speed test")
        ok, data = test_speed(model_id, profile)
        result["speed"] = data
        ct = data.get("calc_tokens_per_sec") or data.get("tokens_per_sec") or (data.get("stats", {}).get("predicted_tps"))
        log_result("Speed", ok, data.get("elapsed_ms"), f"TPS={ct:.1f}" if ct else f"len={data.get('content_length')}")

        log("Phase 5: Streaming test")
        ok, data = test_streaming(model_id, profile)
        result["stream"] = data
        log_result("Streaming", ok, data.get("elapsed_ms"), f"chunks={data.get('chunk_count')}")

        log("Phase 6: Tool call test")
        ok, data = test_tool_call(model_id, profile)
        result["tool_call"] = data
        log_result("Tool calls", ok, data.get("elapsed_ms"), f"tools={data.get('has_tool_call')}")

        log("Phase 7: Unload model")
        unload_ok, unload_elapsed, msg = unload_port(port)
        result["unload_ok"] = unload_ok
        result["unload_elapsed_ms"] = round(unload_elapsed, 2)
        log_result("Unload", unload_ok, unload_elapsed)

        if unload_ok:
            closed = wait_port_closed(port)
            result["port_closed_after"] = closed
            log(f"  Port closed: {closed}")

        health_ok2, _ = health_check()
        result["health_after"] = health_ok2

        result["status"] = "pass"
        log(f"\nResult: PASS")

    except Exception as e:
        log(f"\nResult: FAIL - {e}")
        result["status"] = "fail"
        result["error"] = str(e)

    result["finished_utc"] = datetime.now(timezone.utc).isoformat()
    return result


def save_result(model_result):
    LOG_DIR.mkdir(parents=True, exist_ok=True)

    safe_name = model_result["model_id"].replace("/", "_").replace("\\", "_")
    result_file = LOG_DIR / f"{safe_name}.json"
    with open(result_file, "w", encoding="utf-8") as f:
        json.dump(model_result, f, indent=2, ensure_ascii=False)

    latest_file = RUN_DIR / f"latest-{safe_name}.json"
    with open(latest_file, "w", encoding="utf-8") as f:
        json.dump(model_result, f, indent=2, ensure_ascii=False)

    log_details_path = LOG_DIR / f"{safe_name}-details.json"
    with open(log_details_path, "w", encoding="utf-8") as f:
        json.dump(model_result, f, indent=2, ensure_ascii=False)


def save_summary(all_results):
    summary = {
        "run_dir": str(RUN_DIR),
        "proxy_base": PROXY_URL,
        "started_utc": all_results[0]["started_utc"] if all_results else "",
        "total": len(all_results),
        "passed": sum(1 for r in all_results if r["status"] == "pass"),
        "failed": sum(1 for r in all_results if r["status"] == "fail"),
        "results": [
            {
                "model_id": r["model_id"],
                "base_model_id": r["base_model_id"],
                "profile": r["profile"],
                "port": r["port"],
                "started_utc": r["started_utc"],
                "finished_utc": r["finished_utc"],
                "status": r["status"],
                "error": r.get("error"),
                "cold_start_ms": (r.get("cold_start") or {}).get("total_ms"),
                "cold_start_ttft_ms": (r.get("cold_start") or {}).get("first_token_ms"),
                "smoke_elapsed_ms": (r.get("smoke") or {}).get("elapsed_ms"),
                "smoke_tps": (r.get("smoke") or {}).get("tokens_per_sec") or ((r.get("smoke") or {}).get("stats") or {}).get("predicted_tps"),
                "speed_elapsed_ms": (r.get("speed") or {}).get("elapsed_ms"),
                "speed_tps": (r.get("speed") or {}).get("calc_tokens_per_sec") or (r.get("speed") or {}).get("tokens_per_sec") or ((r.get("speed") or {}).get("stats") or {}).get("predicted_tps"),
                "unload_ok": r.get("unload_ok"),
                "unload_elapsed_ms": r.get("unload_elapsed_ms"),
            }
            for r in all_results
        ],
    }

    summary_file = RUN_DIR / "summary.json"
    with open(summary_file, "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=2, ensure_ascii=False)

    csv_file = RUN_DIR / "results.csv"
    if all_results:
        fieldnames = list(summary["results"][0].keys())
        with open(csv_file, "w", newline="", encoding="utf-8") as f:
            writer = csv.DictWriter(f, fieldnames=fieldnames)
            writer.writeheader()
            writer.writerows(summary["results"])

    results_json_file = RUN_DIR / "results.json"
    with open(results_json_file, "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=2, ensure_ascii=False)

    print(f"\n{'='*60}")
    print(f"SUMMARY: {summary['passed']}/{summary['total']} passed, {summary['failed']} failed")
    print(f"Results: {RUN_DIR}")
    print(f"{'='*60}")
    for r in summary["results"]:
        status_mark = "PASS" if r["status"] == "pass" else "FAIL"
        tps_info = f" TPS={r.get('speed_tps', 'N/A')}" if r.get('speed_tps') else ""
        print(f"  {status_mark}: {r['model_id']}{tps_info}")
    print()


def discover_models(proxy_url):
    """Fetch model list from proxy /v1/models."""
    try:
        r = requests.get(f"{proxy_url}/v1/models", timeout=5)
        r.raise_for_status()
        data = r.json()
        models = []
        for m in data.get("data", []):
            mid = m.get("id", "")
            if mid:
                models.append({"id": mid, "profile": None, "test_ctx": False})
        return models
    except Exception as e:
        print(f"Warning: auto-discovery failed ({e}), using hardcoded model list")
        return None


def parse_args():
    parser = argparse.ArgumentParser(description="wakeupLLM Model Test Suite")
    parser.add_argument("--proxy", default=PROXY_URL,
                        help=f"Proxy URL (default: {PROXY_URL})")
    parser.add_argument("--discover", action="store_true",
                        help="Auto-discover models from proxy /v1/models instead of hardcoded list")
    return parser.parse_args()


def main():
    args = parse_args()
    proxy_url = args.proxy
    chat_url = f"{proxy_url}/v1/chat/completions"
    health_url = f"{proxy_url}/health"

    global PROXY_URL, CHAT_URL, HEALTH_URL, MODELS
    PROXY_URL = proxy_url
    CHAT_URL = chat_url
    HEALTH_URL = health_url

    print(f"{'='*60}")
    print(f"wakeupLLM Model Test Suite")
    print(f"Proxy: {PROXY_URL}")
    print(f"Started: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    print(f"{'='*60}")

    health_ok, health_data = health_check()
    if not health_ok:
        print(f"ERROR: Proxy not reachable at {PROXY_URL}")
        print("Start wakeupLLM first, then run this script.")
        sys.exit(1)
    print(f"Proxy health: OK - models: {health_data.get('models', {})}")

    if args.discover:
        discovered = discover_models(PROXY_URL)
        if discovered:
            MODELS = discovered
        else:
            print("Auto-discovery failed, falling back to hardcoded model list")

    all_results = []
    for model_cfg in MODELS:
        result = test_model_entry(model_cfg)
        save_result(result)
        all_results.append(result)

        if result["status"] != "pass" and result.get("error"):
            log(f"  Error: {result['error']}")

    save_summary(all_results)


if __name__ == "__main__":
    main()
