# wakeupLLM

A lightweight GUI proxy that manages multiple `llama-server` instances on demand. Each model starts on its own port when first requested and stays warm until the idle timeout expires.

## Architecture

```
┌───────────────────────┐
│    Your App / Client   │  →  http://127.0.0.1:8000/v1/...
└──────────┬────────────┘
           │
┌──────────▼────────────┐
│    wakeupLLM Proxy     │  (egui GUI + TCP proxy)
│  port 8000             │
│  - health → /health    │
│  - model list → /v1/models
│  - unload → /unload/:port
│  - all other → forward │
└──────┬─────────┬──────┘
       │         │
┌──────▼──┐ ┌───▼──────┐
│ Model A  │ │ Model B  │   llama-server instances
│ port 8003│ │ port 8004│   (started on demand)
└─────────┘ └──────────┘
```

## API Endpoints

| Endpoint | Method | Description |
|---|---|---|
| `/health` | GET | Proxy status + models online |
| `/v1/models` | GET | List configured models |
| `/v1/chat/completions` | POST | Forward to selected model |
| `/unload/{port}` | GET | Stop a specific model |

All OpenAI-compatible endpoints (`/v1/chat/completions`, `/v1/completions`, etc.) are forwarded to the destination model. The model is selected via the `model` field in the JSON body.

## Installation

### Prerequisites

- [Rust toolchain](https://rustup.rs/) (≥1.77)
- [llama.cpp](https://github.com/ggml-org/llama.cpp) built with `llama-server` binary
- GGUF model files

### Build

```bash
cd proxy/wakeupLLM
cargo build --release
```

The binary is produced at `src-tauri/target/release/wakeupllm.exe`.

### Configure

1. Copy `model-config.example.json` → `model-config.json`
2. Edit `model-config.json`:
   - Set `out_port` (default: 8000)
   - Set `idle_timeout_seconds` (0 = never unload)
   - Add model entries with:
     - `id` — unique identifier (used in `/v1/chat/completions` body)
     - `port` — upstream port
     - `executable` — path to `llama-server`
     - `arguments` — CLI args for `llama-server`
     - `profiles` — (optional) named presets for temperature/top_p/etc.

## Usage

1. Run `wakeupllm.exe`
2. The GUI shows configured models with status badges (active/inactive)
3. Click **Start** (▶) to launch a model or **Stop** (⏹) to unload
4. The **Unload All** button stops all running instances
5. A system tray icon allows quick access

When a client sends a request to the proxy for a model that is not running, the proxy starts `llama-server` and waits for a healthy `/health` response before forwarding (prevents 503 errors).

## Test Suite

```bash
# Test all models defined in model-config.json
python test-models.py

# Test with custom proxy URL
python test-models.py --proxy http://127.0.0.1:8080

# Auto-discover models from the proxy
python test-models.py --discover
```

Results are saved to `test-results/run-*/`.

## Distribution

`wakeupLLM` bundles `llama-server.exe` by locating it relative to its own executable. To distribute:

1. Build wakeupLLM: `cargo build --release`
2. Copy binaries together:
   ```powershell
   .\deploy.ps1
   ```
   Or manually copy `llama-server.exe` into the same folder as `wakeupLLM.exe`.
3. Include `model-config.json` with relative paths (e.g. `"executable": "llama-server.exe"`)
4. Ship the folder: `wakeupLLM.exe` + `llama-server.exe` + `model-config.json`

The `executable` field in `model-config.json` supports both absolute and relative paths. Relative paths are resolved from the directory containing `wakeupLLM.exe`.

## Troubleshooting

- **Model fails to start**: Check `wakeupllm.log` in `%TEMP%` for details
- **Port conflict**: Ensure no other service uses ports 8000-8006
- **VRAM issues**: Use `--n-cpu-moe` for MoE models; adjust `--n-gpu-layers`
- **Tray icon not visible**: Ensure `src-tauri/icons/icon256.png` exists
