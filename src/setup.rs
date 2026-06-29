pub const MODELS: &str = include_str!("../assets/models.ron");
pub const PREAMBLE: &str = include_str!("../assets/preamble.md");

/// On first boot, seed `~/.crabot/` with compiled-in default assets.
pub fn ensure_default_files() {
    let crabot_dir = home::home_dir().unwrap_or_default().join(".crabot");
    if crabot_dir.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(crabot_dir.join("preamble"));
    let _ = std::fs::write(crabot_dir.join("preamble").join("crabot.md"), PREAMBLE);
}
