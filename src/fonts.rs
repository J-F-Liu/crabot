use fontdb::Family;
use iced::advanced::graphics::text;

pub fn load_system_fonts() {
    let mut fs = text::font_system().write().expect("Write font system");

    // System fonts are already loaded by cosmic-text's `FontSystem::new_with_fonts`,
    // so we only need to pick a CJK family and set it as the sans-serif default.
    // This overrides cosmic-text's built-in "Open Sans" default (usually not installed),
    // and a CJK font provides much broader Unicode coverage than Western UI fonts.
    const CJK_SANS_NAMES: &[&str] = &[
        "Microsoft YaHei UI",
        "Microsoft YaHei",
        "PingFang SC",
        "STHeiti",
        "STXihei",
        "Noto Sans CJK SC",
        "Noto Sans SC",
        "WenQuanYi Micro Hei",
        "WenQuanYi Zen Hei",
        "Droid Sans Fallback",
    ];

    const CJK_MONO_NAMES: &[&str] = &[
        "Maple Mono",
        "Cascadia Code",
        "Sarasa Mono SC",
        "Noto Sans Mono CJK SC",
        "LXGW WenKai Mono",
        "Source Han Mono",
    ];

    /// Western monospace fonts. Used as fallback when no CJK mono font is installed.
    /// These provide proper English monospace glyphs; CJK characters will fall back
    /// to the sans-serif family or system fonts.
    const WESTERN_MONO_NAMES: &[&str] = &[
        "JetBrains Mono",
        "Fira Code",
        "Source Code Pro",
        "Consolas",
        "Monaco",
        "Menlo",
        "DejaVu Sans Mono",
        "Ubuntu Mono",
        "Courier New",
    ];

    fn find_first(db: &fontdb::Database, names: &[&'static str]) -> Option<&'static str> {
        for &name in names {
            let q = fontdb::Query {
                families: &[Family::Name(name)],
                ..Default::default()
            };
            if db.query(&q).is_some() {
                return Some(name);
            }
        }
        None
    }

    let db = fs.raw().db_mut();

    if let Some(name) = find_first(db, CJK_SANS_NAMES) {
        db.set_sans_serif_family(name);
    }

    // Monospace priority: ① CJK mono (best: English + CJK in one font)
    //                    ② Western mono (good English; CJK falls back to system)
    //                    ③ CJK sans   (last resort: at least CJK renders)
    if let Some(name) = find_first(db, CJK_MONO_NAMES)
        .or_else(|| find_first(db, WESTERN_MONO_NAMES))
        .or_else(|| find_first(db, CJK_SANS_NAMES))
    {
        db.set_monospace_family(name);
    }
}
