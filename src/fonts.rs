use fontdb::Family;
use iced::advanced::graphics::text;

pub fn load_system_fonts() {
    let mut fs = text::font_system().write().expect("Write font system");

    // System fonts are already loaded by cosmic-text's `FontSystem::new_with_fonts`,
    // so we only need to pick a CJK family and set it as the sans-serif default.
    // This overrides cosmic-text's built-in "Open Sans" default (usually not installed),
    // and a CJK font provides much broader Unicode coverage than Western UI fonts.
    const CJK_NAMES: &[&str] = &[
        "Microsoft YaHei UI",
        "Microsoft YaHei",
        "PingFang SC",
        "PingFang TC",
        "STHeiti",
        "STXihei",
        "Noto Sans CJK SC",
        "Noto Sans SC",
        "WenQuanYi Micro Hei",
        "WenQuanYi Zen Hei",
        "Droid Sans Fallback",
    ];

    let cjk_name: Option<&str> = {
        // `db_mut()` takes `&mut self` but `query` only needs `&Database`
        let db = fs.raw().db_mut();
        let mut found = None;
        for &name in CJK_NAMES {
            let q = fontdb::Query {
                families: &[Family::Name(name)],
                ..Default::default()
            };
            if db.query(&q).is_some() {
                found = Some(name);
                break;
            }
        }
        found
    };

    if let Some(name) = cjk_name {
        fs.raw().db_mut().set_sans_serif_family(name);
    }
}
