use include_dir::{Dir, include_dir};

static ASSETS: Dir = include_dir!("$CARGO_MANIFEST_DIR/assets");

/// Embedded default models configuration.
pub fn default_models() -> &'static str {
    ASSETS
        .get_file("models.ron")
        .and_then(|f| f.contents_utf8())
        .unwrap_or("")
}

/// On first boot, seed `~/.crabot/` with compiled-in default assets.
pub fn ensure_default_files() {
    let crabot_dir = home::home_dir().unwrap_or_default().join(".crabot");

    let preamble_dir = crabot_dir.join("preamble");
    if !preamble_dir.is_dir() {
        let _ = std::fs::create_dir_all(&preamble_dir);
        if let Some(file) = ASSETS.get_file("preamble.md") {
            let _ = std::fs::write(preamble_dir.join("crabot.md"), file.contents());
        }
    }

    let rules_dir = crabot_dir.join("rules");
    if !rules_dir.is_dir() {
        let _ = std::fs::create_dir_all(&rules_dir);
        if let Some(rules) = ASSETS.get_dir("rules") {
            let _ = rules.extract(&crabot_dir);
        }
    }
}
