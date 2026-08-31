use std::fs;
use std::path::PathBuf;
use std::process::Command;

use zed_extension_api::{
    self as zed, current_platform, download_file, latest_github_release,
    lsp::{Completion, CompletionKind},
    make_file_executable, register_extension, set_language_server_installation_status,
    settings::LspSettings,
    CodeLabel, CodeLabelSpan, DownloadedFileType, Extension, GithubReleaseOptions,
    LanguageServerId, LanguageServerInstallationStatus, Os, Worktree,
};

struct GroovyExtension {
    cached_binary_path: Option<String>,
}

impl GroovyExtension {
    fn language_server_binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
    ) -> zed::Result<String> {
        if let Some(path) = &self.cached_binary_path {
            if fs::metadata(path).is_ok_and(|stat| stat.is_file()) {
                return Ok(path.clone());
            }
        }

        set_language_server_installation_status(
            language_server_id,
            &LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let release = latest_github_release(
            "valentinegb/groovy-language-server",
            GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;
        let (platform, _arch) = current_platform();
        let asset_name = format!(
            "groovy-language-server-{os}",
            os = match platform {
                Os::Mac => "macOS",
                Os::Linux => "Linux",
                Os::Windows => "Windows",
            },
        );
        let asset_file = format!("{asset_name}.zip");
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_file)
            .ok_or_else(|| format!("no asset found matching {asset_file:?}"))?;
        let version_dir = format!("groovy-language-server-{}", release.version);
        let binary_path = format!("{version_dir}/{asset_name}/groovy_language_server_wrapper");

        if !fs::metadata(&binary_path).is_ok_and(|stat| stat.is_file()) {
            set_language_server_installation_status(
                language_server_id,
                &LanguageServerInstallationStatus::Downloading,
            );
            download_file(&asset.download_url, &version_dir, DownloadedFileType::Zip)
                .map_err(|e| format!("failed to download file: {e}"))?;
            make_file_executable(&binary_path)?;

            let entries =
                fs::read_dir(".").map_err(|e| format!("failed to list working directory {e}"))?;

            for entry in entries {
                let entry = entry.map_err(|e| format!("failed to load directory entry {e}"))?;

                if entry.file_name().to_str() != Some(&version_dir) {
                    fs::remove_dir_all(entry.path()).ok();
                }
            }
        }

        self.cached_binary_path = Some(binary_path.clone());

        Ok(binary_path)
    }
}

impl Extension for GroovyExtension {
    fn new() -> Self
    where
        Self: Sized,
    {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        _worktree: &Worktree,
    ) -> zed::Result<zed::Command> {
        Ok(zed::Command {
            command: self.language_server_binary_path(language_server_id)?,
            args: Vec::new(),
            env: Vec::new(),
        })
    }

    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> zed::Result<Option<zed::serde_json::Value>> {
        let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)
            .ok()
            .and_then(|lsp_settings| lsp_settings.settings);

        // If user provided explicit classpath, pass through as-is
        if let Some(ref s) = settings {
            if let Some(groovy) = s.get("groovy") {
                if groovy.get("classpath").is_some() {
                    return Ok(settings);
                }
            }
        }

        // Auto-detect Groovy installation and build classpath with wildcard
        match detect_groovy_classpath() {
            Some(classpath) => Ok(Some(zed::serde_json::json!({
                "groovy": { "classpath": classpath }
            }))),
            None => {
                eprintln!(
                    "[groovy-lsp] Could not auto-detect Groovy installation. \
                     Set GROOVY_HOME or configure lsp.groovy.settings.groovy.classpath in settings.json."
                );
                Ok(settings)
            }
        }
    }

    fn label_for_completion(
        &self,
        _language_server_id: &LanguageServerId,
        completion: Completion,
    ) -> Option<CodeLabel> {
        match completion.kind? {
            CompletionKind::Class | CompletionKind::Enum | CompletionKind::Interface => {
                Some(CodeLabel {
                    code: format!("{} variable", completion.label),
                    spans: vec![
                        CodeLabelSpan::code_range(0..completion.label.len()),
                        CodeLabelSpan::literal(format!(" (import {})", completion.detail?), None),
                    ],
                    filter_range: (0..completion.label.len()).into(),
                })
            }
            CompletionKind::Method => {
                let code = format!("{}()", completion.label);

                Some(CodeLabel {
                    spans: vec![CodeLabelSpan::code_range(0..code.len())],
                    code,
                    filter_range: (0..completion.label.len()).into(),
                })
            }
            CompletionKind::Variable => {
                let def = "def ";
                let code = format!("{def}{}", completion.label);

                Some(CodeLabel {
                    spans: vec![CodeLabelSpan::code_range(def.len()..code.len())],
                    code,
                    filter_range: (0..completion.label.len()).into(),
                })
            }
            _ => None,
        }
    }
}

/// Auto-detect the Groovy installation and return classpath with wildcard.
///
/// Fallback chain:
/// 1. GROOVY_HOME env var
/// 2. `which groovy` command
/// 3. Common installation paths (mise, sdkman, homebrew)
fn detect_groovy_classpath() -> Option<Vec<String>> {
    // 1. Try GROOVY_HOME
    if let Ok(home) = std::env::var("GROOVY_HOME") {
        let lib = PathBuf::from(home.trim_end_matches('/')).join("lib");
        if lib.exists() {
            return Some(vec![format!("{}/*", lib.display())]);
        }
    }

    // 2. Try `which groovy`
    if let Ok(output) = Command::new("which").arg("groovy").output() {
        if output.status.success() {
            let groovy_bin = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Ok(real_path) = fs::canonicalize(&groovy_bin) {
                // groovy binary is in bin/, lib/ is a sibling
                if let Some(parent) = real_path.parent() {
                    if let Some(parent) = parent.parent() {
                        let lib = parent.join("lib");
                        if lib.exists() {
                            return Some(vec![format!("{}/*", lib.display())]);
                        }
                    }
                }
            }
        }
    }

    // 3. Try common installation paths
    if let Ok(home) = std::env::var("HOME") {
        let candidates = [
            // mise (versioned)
            PathBuf::from(&home).join(".local/share/mise/installs/groovy"),
            // SDKMAN
            PathBuf::from(&home).join(".sdkman/candidates/groovy/current"),
            // Homebrew Intel
            PathBuf::from("/usr/local/opt/groovy/libexec"),
            // Homebrew Apple Silicon
            PathBuf::from("/opt/homebrew/opt/groovy/libexec"),
        ];

        for candidate in &candidates {
            if candidate.join("lib").exists() {
                return Some(vec![format!("{}/*", candidate.join("lib").display())]);
            }
            // For mise: check versioned subdirectories
            if candidate.exists() && candidate.join("lib").join("..").exists() {
                if let Ok(entries) = fs::read_dir(candidate) {
                    for entry in entries.flatten() {
                        let lib = entry.path().join("lib");
                        if lib.exists() {
                            return Some(vec![format!("{}/*", lib.display())]);
                        }
                    }
                }
            }
        }
    }

    None
}

register_extension!(GroovyExtension);
