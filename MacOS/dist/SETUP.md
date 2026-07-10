# wakeupLLM — macOS Setup Guide

## Quick Start

### 1. Install from DMG

Open `wakeupLLM_0.1.0_macOS.dmg` and drag `wakeupLLM.app` into your `Applications` folder.

### 2. First Run

```bash
open /Applications/wakeupLLM.app
```

On first launch, macOS may show an unidentified developer warning. To bypass:

```bash
xattr -d com.apple.quarantine /Applications/wakeupLLM.app
```

Then open it again from Finder or:

```bash
open /Applications/wakeupLLM.app
```

### 3. Configure Models

The app looks for `model-config.json` in the following locations (in order):

1. `~/Library/Application Support/wakeupLLM/model-config.json`
2. `wakeupLLM.app/Contents/Resources/model-config.json`
3. Current working directory

A template is bundled inside the `.app` bundle at `Contents/Resources/model-config.json`.

Copy and edit it:

```bash
cp /Applications/wakeupLLM.app/Contents/Resources/model-config.json ~/model-config.json
# edit to point to your GGUF models and llama-server paths
```

See the [Configuration](#configuration) section for details.

---

## Configuration

### model-config.json

Key fields:

| Field | Description | Example |
|---|---|---|
| `out_port` | Proxy listening port | `8000` |
| `idle_timeout_seconds` | Auto-unload after idle (0 = never) | `300` |
| `models[].id` | Unique model ID (used in API requests) | `"qwen3.5-27b"` |
| `models[].port` | Upstream port for llama-server | `8003` |
| `models[].executable` | Path to llama-server binary | `"llama-server"` |
| `models[].executable` | Path to llama-server (full path if not adjacent) | `"/usr/local/bin/llama-server"` |
| `models[].arguments` | CLI arguments passed to llama-server | see below |

Example entry:

```json
{
  "id": "qwen3.5-27b",
  "port": 8003,
  "executable": "llama-server",
  "arguments": [
    "--model", "/path/to/models/Qwen3.5-27B-Q6_K.gguf",
    "--ngl", "99",
    "--fa",
    "--ctx-size", "8192"
  ]
}
```

### Profile (optional)

Profiles let you define quick preset switches for sampling parameters. They are referenced by appending `/profile-name` to the model ID in API requests.

Example:

```json
{
  "id": "qwen3.5-27b",
  "port": 8003,
  "executable": "llama-server",
  "arguments": ["--model", "...", "--ngl", "99"],
  "profiles": [
    { "name": "creative", "temperature": 1.2, "top_p": 0.95 },
    { "name": "precise",   "temperature": 0.3, "top_p": 0.8 }
  ]
}
```

Request: `{"model": "qwen3.5-27b/creative", ...}` applies the creative profile.

---

## Running

### From the Dock

Simply click the app icon in the Dock.

### From Terminal

```bash
open /Applications/wakeupLLM.app
```

Or pass a custom config:

```bash
/Applications/wakeupLLM.app/Contents/MacOS/wakeupllm
```

### Testing

```bash
curl http://127.0.0.1:8000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "qwen3.5-27b", "messages": [{"role": "user", "content": "hello"}]}'
```

---

## Features

- **Warmup**: Click the 🔥 button next to any model to load it into memory.
- **Auto-idle unload**: Models idle longer than `idle_timeout_seconds` are automatically unloaded to free GPU/CPU memory.
- **Scheduler**: Automatically manages GPU memory across models. If a new model doesn't fit, it evicts the least recently used one.
- **Profiles**: Preset sampling parameters (temperature, top_p, top_k, etc.) selectable per model.
- **System tray**: Appears in the menu bar. Right-click for quick actions (Minimize, Unload All, Refresh, Quit).
- **CPU / RAM / GPU monitoring**: Bottom bar shows real-time system stats. GPU stats require an NVIDIA GPU with NVML.

---

## File Locations

| File | Purpose | Location |
|---|---|---|
| `wakeupLLM.app` | Application bundle | `/Applications/` |
| `model-config.json` | Model definitions | `Contents/Resources/` inside `.app` |
| `model-metrics.json` | GGUF header cache (auto-generated) | `Contents/Resources/` inside `.app` |
| `profile-selections.json` | Saved profile preferences (auto-generated) | `Contents/Resources/` inside `.app` |
| `running-models.json` | Auto-warmup state (auto-generated) | `Contents/Resources/` inside `.app` |

---

## Troubleshooting

### "App is damaged and can't be opened"

Remove quarantine attribute:

```bash
xattr -d com.apple.quarantine /Applications/wakeupLLM.app
```

### "Cannot load model-config.json"

Make sure the config file exists. A template is bundled inside the app, or create one manually using the format above.

### "Port already in use"

Change ports in `model-config.json`. Default proxy port is `8000`.

### Model not responding after warmup

Check `~/wakeupllm.log` for error messages.

### "Metal API not supported"

Your Mac may be too old. Use CPU-only mode when building llama-server:

```bash
cmake -B build -DGGML_METAL=OFF -DLLAMA_BUILD_SERVER=ON
```

### Model loading slow on Apple Silicon

Use `--ngl 99` to offload all layers to GPU. The Metal backend on M-series chips is fast.
