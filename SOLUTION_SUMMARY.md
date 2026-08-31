# Solution Summary: Zed Groovy LSP Configuration Fixes

## Problem

The Groovy language server was not loading in Zed, and even when it did, `JsonSlurper` could not be resolved because the LSP never received the classpath configuration from `settings.json`.

## Root Causes

### 1. Language server not linked to language (`extension.toml`)

**File:** `extension.toml`

The language server was declared with `language` (singular string) instead of `languages` (plural array). Zed does not recognize the `language` key, so the LSP was never associated with the Groovy language.

```toml
# Before (broken):
[language_servers.groovy]
name = "Groovy"
language = "Groovy"

# After (fixed):
[language_servers.groovy]
name = "Groovy"
languages = ["Groovy"]
```

### 2. Wrong WASM build target (`Cargo.toml` + build)

**File:** `Cargo.toml` (no change needed, but build target matters)

The `zed_extension_api` requires building with `wasm32-wasip2`, not `wasm32-wasip1`. Building with the wrong target produces a WASM binary that Zed cannot read metadata from, resulting in `version: null` in the extension registry and the extension failing to load.

```bash
# Before (broken):
cargo build --release --target wasm32-wasip1

# After (fixed):
cargo build --release --target wasm32-wasip2
```

### 3. Missing workspace configuration passthrough (`src/lib.rs`)

**File:** `src/lib.rs`

The Groovy extension did not implement `language_server_workspace_configuration()`. Without this method, Zed sends an empty `{}` to the LSP via `workspace/didChangeConfiguration`, so user settings (classpath, java.home, etc.) never reach the language server.

The Groovy LSP reads configuration from `workspace/didChangeConfiguration` params:
```java
// GroovyServices.java
private void updateClasspath(JsonObject settings) {
    if (settings.has("groovy") && settings.get("groovy").isJsonObject()) {
        // reads groovy.classpath array
    }
}
```

**Fix:** Implemented `language_server_workspace_configuration` to read user settings via `LspSettings::for_worktree()` and pass them to the LSP:

```rust
fn language_server_workspace_configuration(
    &mut self,
    language_server_id: &LanguageServerId,
    worktree: &Worktree,
) -> zed::Result<Option<zed::serde_json::Value>> {
    let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)
        .ok()
        .and_then(|lsp_settings| lsp_settings.settings);
    Ok(settings)
}
```

## Settings Configuration (`settings.json`)

The `settings.json` configuration for the Groovy LSP was already correct:

```json
{
    "lsp": {
        "groovy": {
            "settings": {
                "groovy": {
                    "classpath": [
                        "/Users/edv/.local/share/mise/installs/groovy/4.0.26/lib/groovy-json-4.0.26.jar"
                    ]
                }
            }
        }
    }
}
```

The `settings` value is read by `LspSettings::for_worktree()` and sent to the LSP via `workspace/didChangeConfiguration`.

## Data Flow After Fix

1. Zed reads `settings.json` and finds `lsp.groovy.settings`
2. Zed calls `language_server_workspace_configuration()` on the Groovy extension
3. Extension reads settings via `LspSettings::for_worktree("groovy", worktree)`
4. Extension returns `{"groovy": {"classpath": ["...groovy-json-4.0.26.jar"]}}`
5. Zed sends this via `workspace/didChangeConfiguration` to the Groovy LSP
6. LSP's `updateClasspath()` parses the classpath and loads `groovy-json-4.0.26.jar`
7. `JsonSlurper` becomes resolvable

## Files Changed

| File | Change |
|------|--------|
| `extension.toml` | `language` → `languages` (plural array) |
| `src/lib.rs` | Added `language_server_workspace_configuration()` method + `settings::LspSettings` import |
| `Cargo.toml` | No change (API stays at `0.6.0`) |
| `Cargo.lock` | No change |

## Build & Deploy

```bash
# Build
cargo build --release --target wasm32-wasip2

# Deploy
cp target/wasm32-wasip2/release/zed_groovy.wasm \
   ~/Library/Application\ Support/Zed/extensions/installed/groovy/extension.wasm

# Restart Zed
```

## Key Learnings

1. **Zed extension manifest:** `languages` (plural, array) is the correct key for linking language servers to languages in `extension.toml`. The singular `language` key is ignored.

2. **WASM build target:** The `zed_extension_api` requires `wasm32-wasip2`, not `wasm32-wasip1`. Using the wrong target causes Zed to fail to read WASM metadata.

3. **Extension-provided LSP settings:** Unlike built-in LSPs, extension-provided language servers do not automatically receive user settings from `settings.json`. The extension must implement `language_server_workspace_configuration()` to forward settings to the LSP.

4. **Groovy LSP configuration format:** The LSP expects `{"groovy": {"classpath": [...]}}` via `workspace/didChangeConfiguration`. The `settings` value in `settings.json` must match this structure.
