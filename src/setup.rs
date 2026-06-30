pub const MODELS: &str = include_str!("../assets/models.ron");
pub const PREAMBLE: &str = include_str!("../assets/preamble.md");

/// On first boot, seed `~/.crabot/` with compiled-in default assets.
pub fn ensure_default_files() {
    let crabot_dir = home::home_dir().unwrap_or_default().join(".crabot");
    let preamble_dir = crabot_dir.join("preamble");
    if !preamble_dir.is_dir() {
        let _ = std::fs::create_dir_all(&preamble_dir);
        let _ = std::fs::write(preamble_dir.join("crabot.md"), PREAMBLE);
    }
}
