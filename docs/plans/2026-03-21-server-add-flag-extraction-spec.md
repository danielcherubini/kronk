# Server Add/Edit Flag Extraction - Spec

**Date:** 2026-03-21
**Reviewed:** 2026-03-21
**Status:** DONE

## Problem

`koji server add` and `koji server edit` treat all arguments after the backend path as raw backend args, dumping them into `ModelConfig.args`. This means koji-specific flags like `--model`, `--profile`, and `--quant` are passed directly to the backend (e.g. llama-server) instead of being stored in the proper `ModelConfig` struct fields.

Example command:
```bash
koji server add qwen35 /path/to/llama-server --model unsloth/Qwen3.5-0.8B --profile chat
```

Produces this broken config:
```toml
[models.qwen35]
backend = "llama_server"
args = ["--model", "unsloth/Qwen3.5-0.8B", "--profile", "chat"]
# model = None, quant = None, profile = None
enabled = true
```

The correct config (which `koji model create` produces) should be:
```toml
[models.qwen35]
backend = "llama_server"
model = "unsloth/Qwen3.5-0.8B"
quant = "Q4_K_M"
profile = "chat"
args = ["--host", "0.0.0.0"]
enabled = true
```

### Consequences

1. `build_full_args()` checks `server.model` and `server.quant` to resolve the GGUF path from the model card — both are `None`, so no resolution happens.
2. `--model unsloth/Qwen3.5-0.8B` is passed raw to llama-server, which expects a file path and will fail.
3. `--profile chat` is a koji concept — llama-server doesn't understand it.
4. Context length (`-c`) and GPU layers (`-ngl`) from the model card are not injected.

## Goals

1. **Extract koji flags** from the raw arg vec in `server add` and `server edit`, setting the proper `ModelConfig` struct fields.
2. **Interactive quant selection** when `--model` is provided but `--quant` is not (matching `model create` behavior).
3. **Validate model card existence** when `--model` references a model card (contains `/`, no `.gguf` extension).
4. **Preserve raw backend args** — any flags that aren't koji-specific pass through to `ModelConfig.args` unchanged.
5. **Apply to all entry points** — `server add`, `server edit`, and their hidden top-level aliases (`Commands::Add`, `Commands::Update`).

## Design

### Flag Extraction Logic

A new helper function `extract_koji_flags(args: Vec<String>) -> ExtractedFlags` will parse the raw arg vec and separate koji-specific flags from backend-passthrough args.

**Koji-specific flags to extract:**

| Flag | Destination | Notes |
|------|------------|-------|
| `--model <value>` | `ModelConfig.model` | Only when value looks like a model card ref (contains `/`, no `.gguf`). If value looks like a file path (has `.gguf` ext, or is an absolute path without `/` in the repo-id sense), leave it in remaining args as `-m`. |
| `--profile <value>` | `ModelConfig.profile` | Parsed via `Profile::from_str`. Always extracted — not a backend flag. |
| `--quant <value>` | `ModelConfig.quant` | Always extracted — not a backend flag. |
| `--port <value>` | `ModelConfig.port` | Always extracted — koji-managed port assignment. |
| `--ctx <value>` | `ModelConfig.context_length` | Always extracted — koji injects `-c` at runtime from this or the model card. |

**Flags left in remaining args (passthrough to backend):**
- `-m <path>` (short form — ambiguous, treat as backend flag)
- `--model <path.gguf>` (file path to a GGUF — backend flag)
- `--host`, `-ngl`, `-c`, `--threads`, etc. (backend flags)
- Any unrecognized flags

### Model Card Reference Detection

A `--model` value is treated as a model card reference when:
- It contains exactly one `/` (e.g. `unsloth/Qwen3.5-0.8B`)
- It does NOT end with `.gguf`
- It is NOT an absolute filesystem path

Otherwise it's treated as a backend file path and left in the passthrough args.

### Quant Selection

When `--model` is extracted as a model card reference and `--quant` is not provided:
1. Look up the model card via `ModelRegistry::find(model_id)`
2. If the model card has exactly one quant, auto-select it
3. If multiple quants, show an interactive picker (same as `model create`)
4. If no quants, error with a message to pull the model first

When `--quant` is explicitly provided, validate it exists in the model card.

### Model Card Validation

When `--model` is extracted:
- Verify the model card exists in `configs/`
- Verify the selected quant's GGUF file exists on disk
- Error with helpful messages if either is missing (e.g. "Run `koji model pull ...` first")

### Server Edit Behavior

For `server edit`, extracted flags **overwrite** existing `ModelConfig` fields. If a koji flag is not provided in the new command, the existing value is preserved (not cleared to `None`). This lets users run `koji server edit mymodel llama-server --profile coding` to change just the profile without re-specifying `--model` and `--quant`.

Implementation: `cmd_server_edit` must load the existing `ModelConfig` first, call `extract_koji_flags()` on the new command, then selectively merge — only updating fields that were explicitly provided. The current implementation overwrites `backend` and `args` wholesale; the new implementation must preserve `model`, `quant`, `profile`, `port`, `context_length`, and `source` unless explicitly overridden.

### Port Handling

`--port` is extracted into `ModelConfig.port`, which koji uses for port assignment when starting the backend. The `build_full_args()` function does **not** inject `--port` into backend args — koji manages port binding separately (the proxy routes to backends by port). Extracting `--port` prevents it from being passed as a raw arg to the backend, which is correct since koji controls port allocation.

### Source Field

When `--model` is extracted as a model card reference, `ModelConfig.source` is also set to the model ID (matching `model create` behavior). This provides consistency for `koji status` display.

### Output Messages

Both commands will print detailed output matching `model create`:
```text
Oh yeah, it's all coming together.

  Name:      qwen35
  Model:     unsloth/Qwen3.5-0.8B
  Quant:     Q4_K_M
  GGUF:      /home/daniel/.config/koji/models/unsloth/Qwen3.5-0.8B/Qwen3.5-0.8B-Q4_K_M.gguf
  Profile:   chat
  Backend:   llama_server (/path/to/llama-server)
```

When no model card is involved (raw backend usage), output stays minimal as today.

### Shared Helper Refactor

The backend path resolution logic (absolutizing paths, deriving backend keys, checking for existing backends) is duplicated between `cmd_server_add` and `cmd_server_edit`. This will be extracted into a shared helper `resolve_backend()` to reduce duplication. Both commands and their top-level aliases (`Commands::Add`, `Commands::Update`) delegate to the same underlying functions, so the fix covers all entry points.

### What This Does NOT Change

- `build_full_args()` logic — already works correctly with the struct fields
- `model create` command — already handles this correctly
- Model card structure / profile resolution — no changes
- Config file format — no schema changes
- Backend passthrough semantics — unrecognized flags still pass through

## Edge Cases

1. `--model` without a value → error: "Missing value for --model"
2. `--quant` without `--model` → warning printed, quant stored but won't resolve at runtime
3. `--model` referencing a nonexistent model card → error: "Model 'X' not found. Run `koji model pull` first."
4. `--model` value has `.gguf` extension → treated as backend file path, left in args
5. `server edit` with no koji flags → only updates backend/args, preserves existing model/quant/profile
6. Both `--model` (as model card) and `-m` (as backend flag) in same command → `--model` extracted, `-m` left in args (unlikely but handled)
