# OpenCode Koji Plugin Plan

**Goal:** Create an OpenCode plugin that auto-discovers models from koji's `/v1/models` endpoint and provides model configuration (context limits, capabilities, etc.)

**Architecture:** 
- npm package `opencode-koji` that follows the opencode plugin interface
- Uses the `config` hook to enhance opencode's provider configuration with discovered models
- Supports custom koji endpoint via config or auto-detection on default port 11434
- Parses `/v1/models` response to extract model IDs and additional metadata

**Tech Stack:** TypeScript, npm package `@opencode-ai/plugin`, OpenCode Plugin API

---

## Context

Koji is a local AI server that provides an OpenAI-compatible API. When running `koji serve`, it exposes `/v1/models` which lists available models. This plugin will:
1. Auto-detect koji running on default port 11434
2. Query `/v1/models` to discover available models  
3. Provide model configuration including context limits, capabilities, etc.
4. Allow custom endpoint configuration via opencode.json

This eliminates the need for manual model declaration in opencode.json when using koji.

---

### Task 1: Create Plugin Project Structure

**Context:**
Initialize a new npm package for the opencode-koji plugin. This creates the foundation for all plugin functionality.

**Files:**
- Create: `opencode-koji/package.json`
- Create: `opencode-koji/tsconfig.json`
- Create: `opencode-koji/src/index.ts`
- Create: `opencode-koji/src/types/index.ts`
- Create: `opencode-koji/src/utils/koji-api.ts`
- Create: `opencode-koji/src/plugin/config-hook.ts`
- Create: `opencode-koji/src/plugin/index.ts`

**What to implement:**
1. Create `package.json` with:
   - name: `opencode-koji`
   - type: `module`
   - main: `./src/index.ts`
   - exports: `{ ".": "./src/index.ts" }`
   - dependencies: `@opencode-ai/plugin`
   - keywords: `opencode`, `koji`, `plugin`, `local-llm`, `openai-compatible`

2. Create `tsconfig.json` with:
   - target: `ES2022`
   - module: `ESNext`
   - moduleResolution: `bundler`
   - strict mode enabled

3. Create `src/index.ts` that exports `KojiPlugin` from `./plugin`

4. Create `src/types/index.ts` with interfaces:
   ```typescript
   interface KojiModel {
     id: string
     object: string
     created: number
     owned_by: string
     // Koji-specific extensions
     context_limit?: number
     capabilities?: {
       tool_call?: boolean
       completion?: boolean
       embedding?: boolean
     }
   }
   
   interface KojiModelsResponse {
     object: string
     data: KojiModel[]
   }
   
   interface KojiProviderConfig {
     npm?: string
     name?: string
     options?: {
       baseURL?: string
       apiKey?: string
     }
     models?: Record<string, any>
   }
   ```

**Steps:**
- [ ] Create directory structure for opencode-koji plugin
- [ ] Initialize package.json with proper metadata and dependencies
- [ ] Create tsconfig.json for TypeScript compilation
- [ ] Create type definitions in src/types/index.ts
- [ ] Run `npm install` to install dependencies
- [ ] Verify project structure is correct

---

### Task 2: Implement Koji API Client

**Context:**
Create utility functions to interact with koji's OpenAI-compatible API. The main endpoint is `/v1/models` which returns available models. Koji may provide additional metadata about model capabilities.

**Files:**
- Modify: `opencode-koji/src/utils/koji-api.ts`

**What to implement:**
Create `src/utils/koji-api.ts` with functions:

1. `normalizeBaseURL(url: string)` - Remove trailing slashes and `/v1` suffix
2. `buildAPIURL(baseURL: string, endpoint?: string)` - Build full API URL
3. `checkKojiHealth(baseURL: string)` - Check if koji is running (GET /v1/models with timeout)
4. `discoverKojiModels(baseURL: string)` - Fetch models from `/v1/models`
5. `autoDetectKoji()` - Scan common ports (11434, 8080) for koji instance
6. `parseModelCapabilities(model: KojiModel)` - Extract capabilities from model metadata

**What NOT to change:**
- Do not assume all models have extended metadata
- Handle missing fields gracefully (make everything optional)
- Do not make assumptions about koji version - support both basic and extended responses

**Steps:**
- [ ] Create src/utils directory
- [ ] Implement normalizeBaseURL function
- [ ] Implement buildAPIURL function
- [ ] Implement checkKojiHealth with 3s timeout
- [ ] Implement discoverKojiModels with proper error handling
- [ ] Implement autoDetectKoji scanning ports 11434 and 8080
- [ ] Add JSDoc comments for all exported functions
- [ ] Add unit tests for API utilities

---

### Task 3: Implement Config Enhancement Hook

**Context:**
The config hook is called when opencode loads configuration. This hook will:
1. Check if koji provider is already configured
2. If not, auto-detect koji on default port
3. Query `/v1/models` to discover models
4. Enhance the config with discovered models

**Files:**
- Create: `opencode-koji/src/plugin/config-hook.ts`

**What to implement:**
`createConfigHook(client, toastNotifier)` that returns a config handler function:

```typescript
export function createConfigHook(client: PluginInput['client'], toastNotifier: ToastNotifier) {
  return async (config: any) => {
    // If config already has koji provider, use its baseURL
    // Otherwise auto-detect and create provider
    
    // Query /v1/models and merge discovered models into config.provider.koji.models
    
    // Handle errors gracefully - don't block opencode startup
  }
}
```

**Config merging logic:**
- Preserve manually configured models
- Add discovered models that don't conflict
- Log discovered model count
- Warn if no models found (koji might be offline)

**What NOT to change:**
- Don't modify config if no koji instance found
- Don't override manually configured model settings
- Don't block opencode startup on errors

**Steps:**
- [ ] Create src/plugin directory
- [ ] Implement createConfigHook function
- [ ] Handle auto-detection fallback
- [ ] Implement model discovery and config merging
- [ ] Add error handling with timeouts (max 5s wait for discovery)
- [ ] Log discovered model count at INFO level
- [ ] Run tests to verify config hook

---

### Task 4: Implement Main Plugin Entry Point

**Context:**
Create the main plugin export that implements the OpenCode Plugin interface. The plugin returns hooks for config modification.

**Files:**
- Modify: `opencode-koji/src/plugin/index.ts`

**What to implement:**
```typescript
export const KojiPlugin: Plugin = async (input: PluginInput) => {
  const { client } = input
  const toastNotifier = new ToastNotifier(client)

  return {
    config: createConfigHook(client, toastNotifier),
  }
}
```

**What NOT to change:**
- Don't add unnecessary hooks (event, chat.params) unless needed
- Keep plugin minimal and focused

**Steps:**
- [ ] Implement KojiPlugin in src/plugin/index.ts
- [ ] Create ToastNotifier for user feedback (stub implementation OK)
- [ ] Export main plugin as default
- [ ] Verify plugin matches @opencode-ai/plugin interface

---

### Task 5: Add Tests and Documentation

**Context:**
Add basic tests to verify plugin works correctly and document usage.

**Files:**
- Create: `opencode-koji/test/plugin.test.ts`
- Create: `opencode-koji/README.md`

**What to implement:**
1. Test config hook with mock koji responses
2. Test model discovery parsing
3. Test auto-detection logic
4. README with installation and usage instructions

**README content:**
```markdown
# OpenCode Koji Plugin

Auto-discovers models from koji local AI server.

## Installation

Add to opencode.json:
```json
{
  "plugin": ["opencode-koji"]
}
```

## Configuration

Koji auto-detects on default port 11434. Or configure manually:

```json
{
  "provider": {
    "koji": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "Koji",
      "options": {
        "baseURL": "http://localhost:11434/v1"
      }
    }
  }
}
```
```

**Steps:**
- [ ] Create vitest.config.ts for testing
- [ ] Create test/plugin.test.ts with mock fetch and config objects
- [ ] Test auto-detection and model discovery
- [ ] Test config merging preserves manual settings
- [ ] Create README.md with installation instructions
- [ ] Run tests and verify all pass

---

## Acceptance Criteria

- [ ] Plugin can be installed via `npm install opencode-koji`
- [ ] Plugin auto-detects koji on default port 11434
- [ ] Plugin discovers models from `/v1/models` endpoint
- [ ] Discovered models appear in opencode's `/models` list
- [ ] Manual model configuration is preserved (not overwritten)
- [ ] Plugin handles koji being offline gracefully (no errors)
- [ ] Tests pass and README documents usage

---

## Related Issues

- opencode issue #6231: Auto-discover models from OpenAI-compatible provider endpoints
- opencode issue #18219: Allow dynamic model passthrough without explicit declaration