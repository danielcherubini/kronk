# Migration Guide

## v2.0 — Renamed to `koji`

The project was renamed from `kronk` to `koji` because the `kronk` name was
already taken by another similar project. The rename is a hard break with no
backward-compatibility shims, with one exception: the binary auto-migrates
existing user data on first run.

### What changed

- **CLI binary** is now `koji` (was `kronk`).
- **Data directory** is now `~/.config/koji` on Linux and `%APPDATA%\koji`
  on Windows (was `~/.config/kronk` / `%APPDATA%\kronk`).
- **SQLite database** inside the data directory is now `koji.db` (was
  `kronk.db`).
- **HTTP management API** routes moved from `/kronk/v1/*` to `/koji/v1/*`.
- **Environment variables** with the `KRONK_*` prefix are no longer read.
  Use `KOJI_*` instead if you set any of these manually (the binary itself
  does not require any env vars).
- **Linux systemd unit** is now `koji.service` (was `kronk.service`).
  Per-server units are now named `koji-<name>` (was `kronk-<name>`).
- **Windows service** is registered as `koji` with display name `Koji` (was
  `kronk` / `Kronk`).
- **Windows firewall rule** is labelled `Koji: <name>` (was `Kronk: <name>`).
- **Deprecated `kronk proxy start` subcommand** has been removed. Use
  `koji serve` instead.
- **Project layout:** crates are now `koji-core`, `koji-cli`, `koji-mock`,
  `koji-web` (were `kronk-*`).
- **Repository URL:** `https://github.com/danielcherubini/koji`.

### Automatic data directory migration

On first run, `koji` checks whether the legacy `kronk` data directory
exists. If it does and the new `koji` directory does **not**, the binary
renames the directory in place and renames `kronk.db` → `koji.db` inside
it. All models, configs, model cards, backends, logs, and database history
are preserved.

If both the legacy and new directories exist, the migration is skipped and
the new directory is used as-is.

### Manual migration steps

1. **Uninstall the old service.** The rename changes service and firewall
   rule names, so the new binary cannot manage services that were installed
   by `kronk`. Before upgrading, run the old `kronk service remove` (and
   `kronk service stop` first if it is running). On Windows, also check
   `services.msc` and the firewall for any leftover `kronk` / `Kronk`
   entries.
2. **Install `koji`.** Use the new installer, `cargo install`, or the deb /
   rpm package.
3. **Start the new service.** Run `koji service install` followed by
   `koji service start`.
4. **Update any external clients** that called `/kronk/v1/*` to use
   `/koji/v1/*`. The OpenAI-compatible routes (`/v1/chat/completions`,
   `/v1/models`, etc.) are unchanged.

---

# Migration from `profiles` to Model Cards

## Overview

Koji is transitioning from the `profiles/` directory-based sampling configuration to a model-card-centric approach. This document explains the migration and provides usage examples.

## Key Changes

### Before: `profiles/` Directory

Previously, sampling presets were stored in `profiles/` as separate TOML files:

```text
~/.config/koji/
├── profiles/
│   ├── coding.toml
│   ├── chat.toml
│   ├── analysis.toml
│   └── creative.toml
└── configs/
    └── bartowski--OmniCoder-8B.toml
```

Each profile file contained sampling parameters that were hardcoded to default values:

```toml
# ~/.config/koji/profiles/coding.toml
[profile.coding]
temperature = 0.7
top_p = 0.95
repetition_penalty = 1.1
max_tokens = 2048
context_size = 8192
```

### After: Model Card Sampling

Now, sampling parameters are stored directly in model cards within `configs/`. The `profiles/` directory is no longer used and will be automatically cleaned up.

```toml
# ~/.config/koji/configs/bartowski--OmniCoder-8B.toml
[metadata]
name = "bartowski/OmniCoder-8B"
version = "1.0"

[sampling.coding]
temperature = 0.7
top_p = 0.95
repetition_penalty = 1.1
max_tokens = 2048

[sampling.chat]
temperature = 0.9
top_p = 0.9
repetition_penalty = 1.05
max_tokens = 4096

[sampling.analysis]
temperature = 0.5
top_p = 0.95
repetition_penalty = 1.15
max_tokens = 8192

[sampling.creative]
temperature = 1.1
top_p = 0.9
repetition_penalty = 1.0
max_tokens = 2048
```

## Usage Examples

### 1. Pull a Model with Sampling Presets

When pulling a model, Koji will automatically detect if a model card exists and copy the sampling templates from the config:

```bash
# Pull a model - sampling templates are automatically copied
koji model pull bartowski/OmniCoder-8B-GGUF
```

The new model card will include sampling presets for `coding`, `chat`, `analysis`, and `creative` profiles.

### 2. Set Profile Defaults

Define default sampling values for new models in your config:

```toml
# ~/.config/koji/config.toml
[sampling_templates.coding]
temperature = 0.7
top_p = 0.95
repetition_penalty = 1.1
max_tokens = 2048

[sampling_templates.chat]
temperature = 0.9
top_p = 0.9
repetition_penalty = 1.05
max_tokens = 4096

[sampling_templates.analysis]
temperature = 0.5
top_p = 0.95
repetition_penalty = 1.15
max_tokens = 8192

[sampling_templates.creative]
temperature = 1.1
top_p = 0.9
repetition_penalty = 1.0
max_tokens = 2048
```

### 3. Custom Profiles (Deprecated)

Custom profiles can still be defined in `config.custom_profiles` and will be migrated into model cards:

```toml
# ~/.config/koji/config.toml
[custom_profiles.fast]
temperature = 0.8
top_p = 0.95
repetition_penalty = 1.05
max_tokens = 2048
```

After migration, this becomes part of the model card under `[sampling.fast]`.

### 4. List Available Profiles

The profiles command now shows only the built-in profiles:

```bash
koji profile list
```

Output:
```text
coding   Temperature: 0.7, Top P: 0.95, Repetition Penalty: 1.1, Max Tokens: 2048
chat     Temperature: 0.9, Top P: 0.9, Repetition Penalty: 1.05, Max Tokens: 4096
analysis Temperature: 0.5, Top P: 0.95, Repetition Penalty: 1.15, Max Tokens: 8192
creative Temperature: 1.1, Top P: 0.9, Repetition Penalty: 1.0, Max Tokens: 2048
```

### 5. Set Profile for a Model

```bash
# Set coding profile for a specific server
koji profile set my-server coding
```

### 6. Clear Profile for a Server

```bash
# Clear the sampling profile for a specific server
koji profile clear my-server
```

## Migration Process

When you start using Koji, the migration happens automatically:

1. **Detection**: Koji detects existing `profiles/` and `custom_profiles` entries
2. **Migration**: Profiles are copied into model cards in `configs/`
3. **Cleanup**: Empty `profiles/` directory is removed
4. **Update**: Configuration references are updated

## Configuration Changes

### Before Migration

```toml
[profile.coding]
temperature = 0.7
top_p = 0.95
repetition_penalty = 1.1
max_tokens = 2048

[profile.chat]
temperature = 0.9
top_p = 0.9
repetition_penalty = 1.05
max_tokens = 4096
```

### After Migration

```toml
[sampling.coding]
temperature = 0.7
top_p = 0.95
repetition_penalty = 1.1
max_tokens = 2048

[sampling.chat]
temperature = 0.9
top_p = 0.9
repetition_penalty = 1.05
max_tokens = 4096
```

## Benefits

- **Centralized Configuration**: All sampling parameters are now in model cards
- **Automatic Migration**: No manual intervention needed
- **Cleaner Configuration**: No more scattered profile files
- **Better Organization**: Profiles are tied to specific models

## Migration Notes

- Existing `profiles/` files will be automatically migrated
- Empty `profiles/` directories will be cleaned up
- Custom profiles from `config.custom_profiles` are migrated
- Model cards are stored in `~/.config/koji/configs/<company>--<model>.toml`
- The `profiles/` directory is no longer used and will be removed

## Troubleshooting

If you encounter issues during migration:

1. Check that model cards exist in `configs/`
2. Verify `config.toml` has `[sampling_templates]` section
3. Ensure no `profiles/` files conflict with model cards
4. Run `koji config show` to verify configuration state

## Future Plans

The `profiles/` directory will be completely removed in future releases. All sampling parameters should be defined in model cards or the `[sampling_templates]` section.

For questions or issues, refer to the Koji documentation or open an issue on GitHub.