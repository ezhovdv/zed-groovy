# Groovy LSP Classpath: Wildcard Support & Auto-Detection

## Problem Statement

The current `.zed/settings.json` lists 13 individual JAR paths in the Groovy LSP classpath. This is fragile — new JARs require manual edits, and the configuration doesn't adapt to different Groovy versions or installations.

## Research Findings

### 1. Groovy LSP Already Supports Wildcard Directory Syntax

The `CompilationUnitFactory.getClasspathList()` method in the Groovy LSP source code handles a trailing `*` on directory paths:

```java
if (entry.endsWith("*")) {
    entry = entry.substring(0, entry.length() - 1);
    mustBeDirectory = true;
}
// ... then lists all *.jar files in that directory
```

**This means**: A classpath entry like `/path/to/groovy/lib/*` tells the LSP to include all `*.jar` files in that directory. No Java-style `*.jar` glob — just a directory path ending with `*`.

**Key detail**: This is a Groovy LSP feature, not a Zed feature. The `zed_extension_api` passes classpath strings as-is — no glob expansion happens in Zed or the extension.

### 2. Zed Extension API `LspSettings`

The `zed_extension_api::settings::LspSettings` struct has:
- `settings: Option<Value>` — passed to the LSP via `workspace/didChangeConfiguration`
- `initialization_options: Option<Value>` — passed during LSP initialization

The current extension reads settings via `LspSettings::for_worktree()` and returns them in `language_server_workspace_configuration()`. The `settings.json` values flow through as-is.

### 3. No Extension Code Changes Required for Wildcard Support

Since the Groovy LSP natively handles `*` suffix expansion, simply changing `.zed/settings.json` from explicit JAR list to a wildcard directory path is sufficient.

## Design Decisions

### Approach: Auto-Detect Groovy Path + Wildcard

**Core idea**: The extension auto-detects the Groovy installation path at runtime and builds a wildcard classpath (`$GROOVY_HOME/lib/*`). Users can override with explicit classpath in `settings.json`.

### Auto-Detection Fallback Chain

1. **`GROOVY_HOME` environment variable** — Most standard way. Check `std::env::var("GROOVY_HOME")`.
2. **Shell command `which groovy`** — Run `which groovy` to discover the installation path, then derive `GROOVY_HOME` from the symlink target.
3. **Common installation paths** — Check well-known locations:
   - `/usr/local/opt/groovy/libexec/lib/*` (Homebrew)
   - `/opt/homebrew/opt/groovy/libexec/lib/*` (Homebrew Apple Silicon)
   - `~/.sdkman/candidates/groovy/current/lib/*` (SDKMAN)
   - `~/.local/share/mise/installs/groovy/*/lib/*` (mise)
4. **Fallback** — If all fail, return empty classpath with a warning logged via `zed::log()`.

### Override Behavior

- Auto-detected classpath is the **default** (base layer).
- If `settings.json` contains `lsp.groovy.settings.groovy.classpath`, it **overrides** the auto-detected path entirely.
- No merging — either auto-detect OR manual config.

### Target `settings.json` Format

After the change, `.zed/settings.json` should look like:

```jsonc
{
  "lsp": {
    "groovy": {
      "settings": {
        "groovy": {
          // Option A: Leave classpath empty/absent — extension auto-detects
          // Option B: Override with explicit path + wildcard
          "classpath": [
            "/Users/edv/.local/share/mise/installs/groovy/4.0.26/lib/*"
          ]
        }
      }
    }
  }
}
```

## Implementation Plan

### File: `src/lib.rs`

**Changes to `language_server_workspace_configuration()`:**

1. Read user settings from `LspSettings::for_worktree()`.
2. If user provided an explicit `classpath` array, use it as-is (pass through to LSP).
3. If no user-provided classpath, auto-detect:
   a. Check `GROOVY_HOME` env var → build `["$GROOVY_HOME/lib/*"]`
   b. Run `which groovy` → derive path → build classpath
   c. Check common paths → build classpath
   d. If nothing found → return `None` (empty classpath, log warning)
4. Return the constructed settings JSON:
   ```json
   {"groovy": {"classpath": ["/detected/path/lib/*"]}}
   ```

**New helper function: `detect_groovy_classpath() -> Option<Vec<String>>`**

```rust
fn detect_groovy_classpath() -> Option<Vec<String>> {
    // 1. Try GROOVY_HOME
    if let Ok(home) = std::env::var("GROOVY_HOME") {
        let lib_path = format!("{}/lib/*", home.trim_end_matches('/'));
        if std::path::Path::new(&lib_path.trim_end_matches('*')).exists() {
            return Some(vec![lib_path]);
        }
    }

    // 2. Try `which groovy`
    if let Ok(output) = std::process::Command::new("which").arg("groovy").output() {
        if output.status.success() {
            let groovy_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            // Resolve symlink and derive lib path
            if let Ok(real) = std::fs::canonicalize(&groovy_path) {
                // binary is usually in bin/, lib is a sibling
                if let Some(parent) = real.parent() {
                    let lib_path = format!("{}/lib/*", parent.parent()?.display());
                    if std::path::Path::new(&lib_path.trim_end_matches('*')).exists() {
                        return Some(vec![lib_path]);
                    }
                }
            }
        }
    }

    // 3. Try common paths
    let home = std::env::var("HOME").ok()?;
    let candidates = [
        format!("{}/.local/share/mise/installs/groovy/*/lib/*", home),
        format!("{}/.sdkman/candidates/groovy/current/lib/*", home),
        "/usr/local/opt/groovy/libexec/lib/*".to_string(),
        "/opt/homebrew/opt/groovy/libexec/lib/*".to_string(),
    ];

    // Note: glob matching would be needed for the `*` in version dir
    // For simplicity, use glob crate or iterate directory entries
    None
}
```

**Updated `language_server_workspace_configuration()`:**

```rust
fn language_server_workspace_configuration(
    &mut self,
    language_server_id: &LanguageServerId,
    worktree: &Worktree,
) -> zed::Result<Option<zed::serde_json::Value>> {
    let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)
        .ok()
        .and_then(|lsp_settings| lsp_settings.settings);

    // If user provided explicit classpath, pass through
    if let Some(ref s) = settings {
        if let Some(groovy) = s.get("groovy") {
            if groovy.get("classpath").is_some() {
                return Ok(settings);
            }
        }
    }

    // Auto-detect
    match detect_groovy_classpath() {
        Some(classpath) => Ok(Some(serde_json::json!({
            "groovy": { "classpath": classpath }
        }))),
        None => {
            eprintln!("[groovy-lsp] Warning: Could not auto-detect Groovy installation. Set GROOVY_HOME or configure lsp.groovy.settings.groovy.classpath in settings.json.");
            Ok(settings)
        }
    }
}
```

### File: `.zed/settings.json`

Replace explicit JAR list with wildcard:

```jsonc
{
  "lsp": {
    "groovy": {
      "settings": {
        "groovy": {
          // Auto-detected by extension — no manual classpath needed.
          // Uncomment to override:
          // "classpath": ["/path/to/groovy/lib/*"]
        }
      }
    }
  }
}
```

### Dependencies

**No new crate dependencies needed.** The implementation uses:
- `std::env::var()` — for GROOVY_HOME
- `std::process::Command` — for `which groovy`
- `std::path::Path` — for path manipulation
- `std::fs` — for directory existence checks

For glob matching of versioned paths like `~/.local/share/mise/installs/groovy/*/lib/*`, we can use simple directory iteration with `std::fs::read_dir()` instead of adding a `glob` crate.

## Files Modified

| File | Change |
|------|--------|
| `src/lib.rs` | Add `detect_groovy_classpath()` helper, update `language_server_workspace_configuration()` |
| `.zed/settings.json` | Remove explicit classpath (auto-detected) or replace with wildcard `lib/*` |

## Testing

1. **With GROOVY_HOME set**: Extension should auto-detect and pass `$GROOVY_HOME/lib/*` to LSP.
2. **Without GROOVY_HOME**: Extension should fall back to `which groovy`, then common paths.
3. **With explicit settings.json classpath**: Extension should use user-provided path, not auto-detect.
4. **With no Groovy installed**: Extension should log warning and pass empty classpath (no crash).
5. **Wildcard verification**: Confirm `groovy-language-server` correctly resolves all JARs from `lib/*` — check that `groovy.transform.Canonical`, `com.fasterxml.jackson.databind.ObjectMapper`, and `groovy.json.JsonSlurper` all resolve.
