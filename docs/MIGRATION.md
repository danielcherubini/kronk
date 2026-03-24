# Migration from `profiles.d` to Model Cards

## Overview

Kronk is transitioning from the `profiles.d/` directory-based sampling configuration to a model-card-centric approach. This document explains the migration and provides usage examples.

## Key Changes

### Before: `profiles.d/` Directory

Previously, sampling presets were stored in `profiles.d/` as separate TOML files:

```
~/.config/kronk/
├── profiles.d/
│   ├── coding.toml
│   ├── chat.toml
│   ├── analysis.toml
│   └── creative.toml
└── configs.d/
    └── bartowski--OmniCoder-8B.toml
```

Each profile file contained sampling parameters that were hardcoded to default values:

```toml
# ~/.config/kronk/profiles.d/coding.toml
[profile.coding]
temperature = 0.7
top_p = 0.95
repetition_penalty = 1.1
max_tokens = 2048

[profile.coding]
context_size = 8192
```

### After: Model Card Sampling

Now, sampling parameters are stored directly in model cards within `configs.d/`. The `profiles.d/` directory is no longer used and will be automatically cleaned up.

```toml
# ~/.config/kronk/configs.d/bartowski--OmniCoder-8B.toml
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

When pulling a model, Kronk will automatically detect if a model card exists and copy the sampling templates from the config:

```bash
# Pull a model - sampling templates are automatically copied
kronk model pull bartowski/OmniCoder-8B-GGUF
```

The new model card will include sampling presets for `coding`, `chat`, `analysis`, and `creative` profiles.

### 2. Set Profile Defaults

Define default sampling values for new models in your config:

```toml
# ~/.config/kronk/config.toml
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
# ~/.config/kronk/config.toml
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
kronk profile list
```

Output:
```
coding   Temperature: 0.7, Top P: 0.95, Repetition Penalty: 1.1, Max Tokens: 2048
chat     Temperature: 0.9, Top P: 0.9, Repetition Penalty: 1.05, Max Tokens: 4096
analysis Temperature: 0.5, Top P: 0.95, Repetition Penalty: 1.15, Max Tokens: 8192
creative Temperature: 1.1, Top P: 0.9, Repetition Penalty: 1.0, Max Tokens: 2048
```

### 5. Set Profile for a Model

```bash
# Set coding profile as default for this model
kronk profile set coding
```

### 6. Remove Profile

```bash
# Remove a profile (deprecated, will error with unknown names)
kronk profile remove nonexistent_profile
```

## Migration Process

When you start using Kronk, the migration happens automatically:

1. **Detection**: Kronk detects existing `profiles.d/` and `custom_profiles` entries
2. **Migration**: Profiles are copied into model cards in `configs.d/`
3. **Cleanup**: Empty `profiles.d/` directory is removed
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

- Existing `profiles.d/` files will be automatically migrated
- Empty `profiles.d/` directories will be cleaned up
- Custom profiles from `config.custom_profiles` are migrated
- Model cards are stored in `~/.config/kronk/configs.d/<company>--<model>.toml`
- The `profiles.d/` directory is no longer used and will be removed

## Troubleshooting

If you encounter issues during migration:

1. Check that model cards exist in `configs.d/`
2. Verify `config.toml` has `[sampling_templates]` section
3. Ensure no `profiles.d/` files conflict with model cards
4. Run `kronk config show` to verify configuration state

## Future Plans

The `profiles.d/` directory will be completely removed in future releases. All sampling parameters should be defined in model cards or the `[sampling_templates]` section.

For questions or issues, refer to the Kronk documentation or open an issue on GitHub.