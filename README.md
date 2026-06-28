# wakeupLLM

A lightweight GUI proxy that manages multiple `llama-server` instances on demand. Each model starts on its own port when first requested and stays warm until the idle timeout expires.

## Why you should give this a try

**One-click multi-model management.** Start, stop, and switch between multiple LLMs from a single GUI — no terminal, no scripting. Each model runs on its own port and auto-unloads after an idle timeout to free VRAM.

**Runs on consumer hardware.** All numbers below were measured on a single machine with **8 GB VRAM and 64 GB RAM**. No cloud instance, no H100, no special drivers.

**Near-instant warmup.** Once a model has been loaded, restarting it takes **~500 ms** — the model is ready before you finish your sentence. Streaming first-token latency is as low as **53 ms** on gemma-4-E4B (Q4_K_M) model.

**Real throughput on modest hardware.** Measured token generation speeds:

|         Model	     | Cold Start | TPS  |  TTFT |
|--------------------------|------------|------|-------|
|gemma-4-E4B (Q4_K_M)	     |   5.9s     | 59.1 |  53ms |
|gemma4-26B-A4B cerebellum |   16.4s    | 32.2 | 224ms |
|gemma4-26B-A4B apex	     |   21.5s    | 30.5 | 253ms |
|Qwen3.6-35B-A3B (APEX)    |   11.3s    | 43.3 | 226ms |
|Qwen3.6-35B-A3B thinking  |   21.2s    | 43.9 | 226ms |

All 4 models run simultaneously, each serving requests independently.

**OpenAI-compatible API.** Drop-in replacement for anything that speaks the OpenAI chat format — Continue, Cursor, Claude, or your own scripts. Just point your client at `http://127.0.0.1:8000` and pick a model.

## Architecture

```
┌───────────────────────┐
│    Your App / Client  │  →  http://127.0.0.1:8000/v1/...
└──────────┬────────────┘
           │
┌──────────▼─────────────────┐
│    wakeupLLM Proxy         │  (egui GUI + TCP proxy)
│  port 8000                 │
│  - health → /health        │
│  - model list → /v1/models │
│  - unload → /unload/:port  │
│  - all other → forward     │
└──────┬─────────┬───────────┘
       │         │
┌──────▼───┐ ┌───▼──────┐
│ Model A  │ │ Model B  │   llama-server instances
│ port 8003│ │ port 8004│   (started on demand)
└──────────┘ └──────────┘
```

## Quick Start

1. Copy the distribution folder to the target machine:
   - `wakeupllm.exe`
   - `llama-server.exe` (same folder)
   - `model-config.json`
2. Edit `model-config.json` — set paths to your GGUF files (see [Configuration](#configuration))
3. Run `wakeupllm.exe`
4. Click **Start** on a model or send a request to `http://127.0.0.1:8000` — the model starts automatically

## Configuration

All settings are in `model-config.json` (next to the exe).

### Global Fields

| Field                  | Type   | Default |           Description              |
|------------------------|--------|---------|------------------------------------|
| `out_port`             | number | `8000`  | Port the proxy listens on          |
| `idle_timeout_seconds` | number | `0`     | Auto-unload after idle (0 = never) |

### Model Entry Fields

| Field        | Required |                      Description                     |
|--------------|----------|------------------------------------------------------|
| `id`         |   yes    | Unique ID, used in the `model` field of API requests |
| `port`       |   yes    | Port for this model's llama-server instance          |
| `executable` |   yes    | Path to `llama-server` (relative to exe or absolute) |
| `arguments`  |   yes    | CLI args passed to `llama-server`                    |
| `name`       |   no     | Display name in GUI                                  |
| `alias`      |   no     | Short alias                                          |
| `profiles`   |   no     | Named parameter presets (see below)                  |

> **Note:** Each model must use a different `port`. Make sure ports don't conflict with other services.

### Adding a New Model

Add an entry to the `models` array:

```json
{
  "id": "my-new-model",
  "name": "My New Model",
  "alias": "nm",
  "port": 8010,
  "executable": "llama-server.exe",
  "arguments": [
    "--model", "/path/to/models/model.gguf",
    "--alias", "my-new-model",
    "--ctx-size", "32768",
    "--n-gpu-layers", "999",
    "--flash-attn", "on",
    "--cache-type-k", "q8_0",
    "--cache-type-v", "q8_0",
    "--perf"
  ]
}
```

Then restart the app. The new model appears in the GUI.

**Minimal arguments** — only these are truly required:

```json
"arguments": [
  "--model", "/path/to/models/model.gguf",
  "--ctx-size", "4096",
  "--n-gpu-layers", "999"
]
```

Other flags (`--flash-attn`, `--cache-type-k`, `--jinja`, etc.) are optional — add as needed.

### Using Profiles

Profiles let you save parameter presets and switch between them in the GUI:

```json
"profiles": [
  { "name": "creative", "temperature": 1.0, "top_p": 0.95 },
  { "name": "precise", "temperature": 0.3, "top_p": 0.8, "top_k": 40 }
]
```

Available profile fields: `temperature`, `top_p`, `top_k`, `presence_penalty`.

### Example Config

See `model-config.example.json` for a full working example.

## API Endpoints

| Endpoint               | Method |          Description         |
|------------------------|--------|------------------------------|
| `/health`              |  GET   | Proxy status + models online |
| `/v1/models`           |  GET   | List configured models       |
| `/v1/chat/completions` |  POST  | Forward to selected model    |
| `/unload/{port}`       |  GET   | Stop a specific model        |

All OpenAI-compatible endpoints (`/v1/chat/completions`, `/v1/completions`, etc.) are forwarded to the destination model. The model is selected via the `model` field in the JSON body.

**Example request:**

```bash
curl http://127.0.0.1:8000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"my-new-model","messages":[{"role":"user","content":"hello"}]}'
```

## Building from Source

**Prerequisites:** [Rust toolchain](https://rustup.rs/) (>=1.77)

```powershell
.\build.ps1
```

This builds `wakeupllm.exe` and copies `llama-server.exe` alongside it automatically.

Output: `wakeupllm.exe` + `llama-server.exe` + `model-config.json` in the project root.

## Distribution

Copy these files to the target machine:

```
wakeupllm.exe        (main app)
llama-server.exe     (inference server, same folder)
model-config.json    (your model definitions)
```

The `executable` field supports both relative paths (resolved from app directory) and absolute paths.

## Test Suite

```powershell
python test-models.py                    # test all configured models
python test-models.py --proxy http://127.0.0.1:8080  # custom proxy
python test-models.py --discover         # auto-discover from proxy
```

Results are saved to `test-results/run-*/`.

## Troubleshooting

- **Model fails to start**: Check `wakeupllm.log` in `%TEMP%`
- **Port conflict**: Ensure no other service uses your configured ports
- **VRAM issues**: Use `--n-cpu-moe` for MoE models; adjust `--n-gpu-layers`
- **Tray icon not visible**: Ensure `src-tauri/icons/icon256.png` exists
