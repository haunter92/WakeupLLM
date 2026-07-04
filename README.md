# wakeupLLM

A lightweight GUI proxy that manages multiple `llama-server` instances on demand. Each model starts on its own port when first requested and stays warm until the idle timeout expires.

## Why you should give this a try

**One-click multi-model management.** Start, stop, and switch between multiple LLMs from a single GUI — no terminal, no scripting. Each model runs on its own port and auto-unloads after an idle timeout to free VRAM.

**Runs on consumer hardware.** All numbers below were measured on a single machine with **8 GB VRAM and 64 GB RAM**. No cloud instance, no H100, no special drivers.

**Near-instant warmup.** Once a model has been loaded, restarting it takes **~500 ms** — the model is ready before you finish your sentence. Streaming first-token latency is as low as **53 ms** on gemma-4-E4B (Q4_K_M) model.

**Real throughput on modest hardware.** Measured token generation speeds:

|         Model	     | Cold Start | TPS  |  TTFT | Unload |
|--------------------------|------------|------|-------|--------|
|gemma-4-E4B (Q4_K_M)	     |   8.2s     | 59.1 |  53ms |  0.4s  |
|gemma4-26B-A4B cerebellum |   16.4s    | 32.2 | 224ms |  3.0s  |
|gemma4-26B-A4B apex	     |   21.5s    | 30.5 | 253ms |  3.9s  |
|Qwen3.6-35B-A3B (APEX)    |   11.3s    | 43.3 | 226ms |  3.3s  |
|Qwen3.6-35B-A3B thinking  |   21.2s    | 43.9 | 226ms |  3.5s  |

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

### Key llama-server flags

|     Flag      |                             Description                          |  
|---------------|------------------------------------------------------------------|
| `-ngl N`      | Number of layers to offload to GPU (99 = all)                    |
| `-fa`         | Flash Attention                                                  |
| `-c N`        | Context size                                                     |
| `-ctk TYPE`   | KV cache type for K (`turbo4`, `turbo3_tcq`, `turbo2_tcq`, etc.) |
| `-ctv TYPE`   | KV cache type for V (same options)                               |
| `--port PORT` | Listening port                                                   |

### Speculative Decoding

|        Flag        |                                             Description                                                   |
|--------------------|-----------------------------------------------------------------------------------------------------------|
| `-md FILE`         | Draft model for speculative decoding                                                                      |
| `--spec-type TYPE` | Speculative type: `draft-simple`, `draft-eagle3`, `draft-mtp`, `ngram-simple`, `copyspec`, `dflash`, etc. |
| `--draft-max N`    | Number of draft tokens (default: 16)                                                                      |
| `-ctkd TYPE`       | KV cache type for draft model K                                                                           |
| `-ctvd TYPE`       | KV cache type for draft model V                                                                           |

Example with MTP speculative decoding:
```json
{
  "arguments": "-m model.gguf -ngl 99 -fa --spec-type draft-mtp -md draft-mtp.gguf -ctk turbo4 -ctv turbo4 --port {port}"
}
```

### Vision + MTP GPU Swap

When VRAM is insufficient for both MTP draft and vision encoder (mmproj):
```json
{
  "arguments": "-m model.gguf -ngl 99 -fa --mmproj mmproj.gguf --spec-type draft-mtp --mmproj-gpu-swap --port {port}"
}
```

### Unified KV Cache

Optimized for single-slot servers — unifies KV buffer across all sequences:
```json
{
  "arguments": "-m model.gguf -ngl 99 -fa -kvu --port {port}"
}
```
## TurboQuant KV Cache

`llama-server.exe` is built from [beellama.cpp](https://github.com/Anbeeld/beellama.cpp) — a fork with **Trellis-Coded Quantization (TCQ)** for KV cache compression.

### Available KV Cache Types

|     Type        |  bpv |                       Description                       |
|-----------------|------|---------------------------------------------------------|
| `turbo4`        | 4.25 | Lossless, ~3.8x compression, virtually no quality loss  |
| `turbo3_tcq`    | 3.25 | Best quality at 3-bit, beats FP16 at short context      |
| `turbo2_tcq`    | 2.25 | Maximum compression, ~7x KV cache compression           |
| `turbo3`        | 3.25 | Scalar quantization (no TCQ), faster encode             |
| `turbo2`        | 2.25 | Scalar quantization 2-bit                               |
| `turbo8`        | 8.25 | 8-bit KV cache (FWHT + uniform grid)                    |

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

## Distribution

Copy these files to the target machine:

```
wakeupllm.exe        (main app)
llama-server.exe     (inference server, same folder)
model-config.json    (your model definitions)
```

The `executable` field supports both relative paths (resolved from app directory) and absolute paths.

### llama-server.exe

Binary is built from [beellama.cpp](https://github.com/Anbeeld/beellama.cpp) with the following options:
- CUDA backend, PTX virtual arch (universal GPU support)
- Flash Attention, TurboQuant KV cache
- UPX compressed (~57 MB)

Requires: [CUDA toolkit](https://developer.nvidia.com/cuda-toolkit) must be installed (cublas64_13.dll, nvcudart_hybrid64.dll).
## Troubleshooting

- **Model fails to start**: Check `wakeupllm.log` in `%TEMP%`
- **Port conflict**: Ensure no other service uses your configured ports
- **VRAM issues**: Use `--n-cpu-moe` for MoE models; adjust `--n-gpu-layers`
- **Tray icon not visible**: Ensure `src-tauri/icons/icon256.png` exists
