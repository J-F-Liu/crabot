use std::path::PathBuf;

use include_dir::{Dir, include_dir};

pub static ASSETS: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets");

/// Embedded default models configuration.
pub fn default_models() -> &'static str {
    ASSETS
        .get_file("models.ron")
        .and_then(|f| f.contents_utf8())
        .unwrap_or("")
}

/// The default workspace path (`~/.crabot`) used when no workspace is set.
pub fn default_workspace_path() -> PathBuf {
    home::home_dir().unwrap_or_default().join(".crabot")
}

/// On first boot, seed `~/.crabot/` with compiled-in default assets.
pub fn ensure_default_files() {
    let crabot_dir = default_workspace_path();
    let _ = std::fs::create_dir_all(&crabot_dir);

    // Seed any missing bundled preamble files (`crabot.md` plus the task-tool
    // mode preambles) — never overwrite user edits.
    let preamble_dir = crabot_dir.join("preamble");
    let _ = std::fs::create_dir_all(&preamble_dir);
    if let Some(dir) = ASSETS.get_dir("preamble") {
        for file in dir.files() {
            if let Some(name) = file.path().file_name() {
                let dest = preamble_dir.join(name);
                if !dest.is_file() {
                    let _ = std::fs::write(&dest, file.contents());
                }
            }
        }
    }

    let rules_dir = crabot_dir.join("rules");
    if !rules_dir.is_dir() {
        let _ = std::fs::create_dir(&rules_dir);
        if let Some(rules) = ASSETS.get_dir("rules") {
            let _ = rules.extract(&crabot_dir);
        }
    }

    let tools_file = crabot_dir.join("tools.ron");
    if !tools_file.is_file()
        && let Some(file) = ASSETS.get_file("tools.ron")
    {
        let _ = std::fs::write(&tools_file, file.contents());
    }

    let mcp_file = crabot_dir.join("mcp.ron");
    if !mcp_file.is_file()
        && let Some(file) = ASSETS.get_file("mcp.ron")
    {
        let _ = std::fs::write(&mcp_file, file.contents());
    }

    let settings_file = crabot_dir.join("settings.ron");
    if !settings_file.is_file()
        && let Some(file) = ASSETS.get_file("settings.ron")
    {
        let _ = std::fs::write(&settings_file, file.contents());
    }
}
